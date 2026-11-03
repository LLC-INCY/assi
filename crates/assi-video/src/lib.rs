//! Bounded screen encoder. H.264 is software OpenH264, never advertised as hardware.
//! No device is opened here. Callers supply already-approved RGBA screen pixels.
use image::{codecs::jpeg::JpegEncoder, DynamicImage, RgbaImage};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_ENCODED_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_SOURCE_PIXELS: u64 = 33_554_432;
pub const MAX_TIMESTAMP_US: u64 = 9_007_199_254_740_991;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodec {
    #[default]
    Jpeg,
    H264,
}

#[derive(Debug, Clone, Serialize)]
pub struct EncodedFrame {
    pub codec: VideoCodec,
    pub keyframe: bool,
    pub width: u32,
    pub height: u32,
    pub timestamp_us: u64,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderOptions {
    pub codec: VideoCodec,
    pub max_edge: u32,
    pub jpeg_quality: u8,
    pub fps: u8,
    pub bitrate: u32,
}
impl Default for EncoderOptions {
    fn default() -> Self {
        Self {
            codec: VideoCodec::Jpeg,
            max_edge: 1920,
            jpeg_quality: 70,
            fps: 24,
            bitrate: 4_000_000,
        }
    }
}
impl EncoderOptions {
    fn validate(self) -> Result<(), VideoError> {
        if !(16..=2560).contains(&self.max_edge)
            || !(1..=100).contains(&self.jpeg_quality)
            || !(1..=30).contains(&self.fps)
            || !(128_000..=12_000_000).contains(&self.bitrate)
        {
            return Err(VideoError::InvalidOptions);
        }
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum VideoError {
    #[error("Недопустимые размеры или данные экрана")]
    InvalidSource,
    #[error("Недопустимые настройки видео")]
    InvalidOptions,
    #[error("Не удалось закодировать экран")]
    Encode,
    #[error("Кадр превышает допустимый размер")]
    TooLarge,
    /// Rate control intentionally skipped this capture; the caller should keep
    /// the previous image and schedule the next capture, not terminate a session.
    #[error("Кадр пропущен для соблюдения битрейта")]
    Skipped,
}

#[derive(Default)]
pub struct VideoEncoder {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    encoder: Option<openh264::encoder::Encoder>,
    options: Option<EncoderOptions>,
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    dimensions: Option<(u32, u32)>,
    h264_failed: bool,
    last_timestamp: Option<u64>,
}
impl VideoEncoder {
    pub fn new() -> Self {
        Self::default()
    }
    /// Clear codec references between sessions, monitors or capability changes.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn h264_supported() -> bool {
        cfg!(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux"
        ))
    }
    pub fn h264_failed(&self) -> bool {
        self.h264_failed
    }

    /// Encodes an approved screen. A failed H.264 backend falls back to independent
    /// JPEG frames for the current configuration; the returned codec is authoritative.
    pub fn encode_rgba(
        &mut self,
        width: u32,
        height: u32,
        rgba: &[u8],
        timestamp_us: u64,
        options: EncoderOptions,
        force_keyframe: bool,
    ) -> Result<EncodedFrame, VideoError> {
        options.validate()?;
        let pixels = u64::from(width) * u64::from(height);
        if width == 0
            || height == 0
            || width > 16_384
            || height > 16_384
            || pixels > MAX_SOURCE_PIXELS
            || pixels * 4 != rgba.len() as u64
            || timestamp_us > MAX_TIMESTAMP_US
        {
            return Err(VideoError::InvalidSource);
        }
        if self.options != Some(options) {
            self.reset();
            self.options = Some(options);
        }
        if self
            .last_timestamp
            .is_some_and(|previous| timestamp_us <= previous)
        {
            return Err(VideoError::InvalidSource);
        }
        let image =
            RgbaImage::from_raw(width, height, rgba.to_vec()).ok_or(VideoError::InvalidSource)?;
        let image = DynamicImage::ImageRgba8(image);
        let image = if width.max(height) > options.max_edge {
            image.resize(
                options.max_edge,
                options.max_edge,
                image::imageops::FilterType::Triangle,
            )
        } else {
            image
        };
        if options.codec == VideoCodec::H264 && !self.h264_failed && Self::h264_supported() {
            // I420 requires even dimensions. Resample the complete image, never crop
            // a strip: the same normalized coordinates must still address its edges.
            let w = (image.width() & !1).max(2);
            let h = (image.height() & !1).max(2);
            if w >= 16 && h >= 16 {
                let image = image
                    .resize_exact(w, h, image::imageops::FilterType::Triangle)
                    .to_rgba8();
                match self.h264(&image, timestamp_us, options, force_keyframe) {
                    Ok(frame) => {
                        self.last_timestamp = Some(timestamp_us);
                        return Ok(frame);
                    }
                    Err(VideoError::Skipped) => {
                        self.last_timestamp = Some(timestamp_us);
                        return Err(VideoError::Skipped);
                    }
                    Err(_) => {}
                }
                self.h264_failed = true;
                #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
                {
                    self.encoder = None;
                }
                return self.jpeg(
                    &DynamicImage::ImageRgba8(image),
                    timestamp_us,
                    options.jpeg_quality,
                );
            }
        }
        self.jpeg(&image, timestamp_us, options.jpeg_quality)
    }

    fn jpeg(
        &mut self,
        image: &DynamicImage,
        timestamp_us: u64,
        quality: u8,
    ) -> Result<EncodedFrame, VideoError> {
        let mut bytes = Vec::new();
        JpegEncoder::new_with_quality(&mut bytes, quality)
            .encode_image(&image.to_rgb8())
            .map_err(|_| VideoError::Encode)?;
        if bytes.len() > MAX_ENCODED_BYTES {
            return Err(VideoError::TooLarge);
        }
        self.last_timestamp = Some(timestamp_us);
        Ok(EncodedFrame {
            codec: VideoCodec::Jpeg,
            keyframe: true,
            width: image.width(),
            height: image.height(),
            timestamp_us,
            bytes,
        })
    }

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    fn h264(
        &mut self,
        image: &RgbaImage,
        timestamp_us: u64,
        options: EncoderOptions,
        force_keyframe: bool,
    ) -> Result<EncodedFrame, VideoError> {
        use openh264::{
            encoder::{
                BitRate, Complexity, Encoder, EncoderConfig, FrameRate, FrameType,
                IntraFramePeriod, Level, Profile, RateControlMode, UsageType,
            },
            formats::{RgbaSliceU8, YUVBuffer},
            OpenH264API, Timestamp,
        };
        let dimensions = (image.width(), image.height());
        if self.encoder.is_none() || self.dimensions != Some(dimensions) {
            let config = EncoderConfig::new()
                .usage_type(UsageType::ScreenContentRealTime)
                .profile(Profile::Baseline)
                .level(Level::Level_5_0)
                .complexity(Complexity::Low)
                .adaptive_quantization(false)
                .background_detection(false)
                .rate_control_mode(RateControlMode::Bitrate)
                .bitrate(BitRate::from_bps(options.bitrate))
                .max_frame_rate(FrameRate::from_hz(f32::from(options.fps)))
                .intra_frame_period(IntraFramePeriod::from_num_frames(
                    u32::from(options.fps) * 2,
                ))
                .skip_frames(true)
                .num_threads(2);
            self.encoder = Some(
                Encoder::with_api_config(OpenH264API::from_source(), config)
                    .map_err(|_| VideoError::Encode)?,
            );
            self.dimensions = Some(dimensions);
        }
        let encoder = self.encoder.as_mut().ok_or(VideoError::Encode)?;
        if force_keyframe {
            encoder.force_intra_frame();
        }
        let source = RgbaSliceU8::new(
            image.as_raw(),
            (dimensions.0 as usize, dimensions.1 as usize),
        );
        let yuv = YUVBuffer::from_rgba8_source(source);
        let encoded = encoder
            .encode_at(&yuv, Timestamp::from_millis(timestamp_us / 1000))
            .map_err(|_| VideoError::Encode)?;
        let keyframe = matches!(encoded.frame_type(), FrameType::IDR);
        if matches!(encoded.frame_type(), FrameType::Skip) {
            return Err(VideoError::Skipped);
        }
        let bytes = encoded.to_vec();
        if bytes.is_empty() {
            return Err(VideoError::Encode);
        }
        if bytes.len() > MAX_ENCODED_BYTES {
            return Err(VideoError::TooLarge);
        }
        Ok(EncodedFrame {
            codec: VideoCodec::H264,
            keyframe,
            width: dimensions.0,
            height: dimensions.1,
            timestamp_us,
            bytes,
        })
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    fn h264(
        &mut self,
        _: &RgbaImage,
        _: u64,
        _: EncoderOptions,
        _: bool,
    ) -> Result<EncodedFrame, VideoError> {
        Err(VideoError::Encode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pixels(w: u32, h: u32) -> Vec<u8> {
        (0..w * h)
            .flat_map(|n| [(n % 251) as u8, 80, 130, 255])
            .collect()
    }
    #[test]
    fn invalid_dimensions_lengths_and_timestamps_are_rejected_before_codec() {
        let mut encoder = VideoEncoder::new();
        for (w, h, data, timestamp) in [
            (0, 2, vec![], 0),
            (2, 2, vec![0; 15], 0),
            (16_385, 1, vec![], 0),
            (32, 32, pixels(32, 32), MAX_TIMESTAMP_US + 1),
        ] {
            assert_eq!(
                encoder
                    .encode_rgba(w, h, &data, timestamp, EncoderOptions::default(), false)
                    .unwrap_err(),
                VideoError::InvalidSource
            );
        }
    }
    #[test]
    fn jpeg_fallback_preserves_geometry_and_bounds() {
        let mut encoder = VideoEncoder::new();
        let frame = encoder
            .encode_rgba(
                100,
                50,
                &pixels(100, 50),
                0,
                EncoderOptions {
                    max_edge: 64,
                    ..EncoderOptions::default()
                },
                false,
            )
            .unwrap();
        assert_eq!(frame.codec, VideoCodec::Jpeg);
        assert!(frame.keyframe);
        assert_eq!((frame.width, frame.height), (64, 32));
        let decoded = image::load_from_memory(&frame.bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (64, 32));
    }
    #[test]
    fn timestamp_is_monotonic_and_reset_accepts_new_session_origin() {
        let mut encoder = VideoEncoder::new();
        let source = pixels(32, 32);
        encoder
            .encode_rgba(32, 32, &source, 100, EncoderOptions::default(), false)
            .unwrap();
        assert!(encoder
            .encode_rgba(32, 32, &source, 99, EncoderOptions::default(), false)
            .is_err());
        encoder.reset();
        assert!(encoder
            .encode_rgba(32, 32, &source, 0, EncoderOptions::default(), false)
            .is_ok());
    }
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    #[test]
    fn h264_roundtrip_idr_delta_forced_idr_and_resize() {
        use openh264::{decoder::Decoder, formats::YUVSource};
        let options = EncoderOptions {
            codec: VideoCodec::H264,
            ..EncoderOptions::default()
        };
        let mut encoder = VideoEncoder::new();
        let mut decoder = Decoder::new().unwrap();
        let mut total = 0;
        for (index, (w, h, force)) in [
            (64, 48, false),
            (64, 48, false),
            (64, 48, true),
            (80, 50, false),
        ]
        .into_iter()
        .enumerate()
        {
            let frame = encoder
                .encode_rgba(w, h, &pixels(w, h), index as u64 * 40_000, options, force)
                .unwrap();
            assert_eq!(frame.codec, VideoCodec::H264);
            if index == 0 || force || index == 3 {
                assert!(frame.keyframe);
            }
            if index == 1 {
                assert!(!frame.keyframe);
            }
            let decoded = decoder.decode(&frame.bytes).unwrap().unwrap();
            assert_eq!(decoded.dimensions(), (w as usize, h as usize));
            total += frame.bytes.len();
        }
        assert!(total < 64 * 48 * 4 * 4);
        assert!(!encoder.h264_failed());
    }
    #[test]
    fn options_validate_even_without_h264() {
        let mut encoder = VideoEncoder::new();
        assert_eq!(
            encoder
                .encode_rgba(
                    32,
                    32,
                    &pixels(32, 32),
                    0,
                    EncoderOptions {
                        fps: 0,
                        ..EncoderOptions::default()
                    },
                    false
                )
                .unwrap_err(),
            VideoError::InvalidOptions
        );
    }
}
