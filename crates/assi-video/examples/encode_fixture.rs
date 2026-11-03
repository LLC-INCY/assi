//! Synthetic encoder output for decoder interoperability checks; no screen capture.
use assi_video::{EncoderOptions, VideoCodec, VideoEncoder};
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pixels: Vec<u8> = (0..64 * 48)
        .flat_map(|pixel| [(pixel % 251) as u8, 80, 130, 255])
        .collect();
    let frame = VideoEncoder::new().encode_rgba(
        64,
        48,
        &pixels,
        0,
        EncoderOptions {
            codec: VideoCodec::H264,
            ..Default::default()
        },
        true,
    )?;
    if frame.codec != VideoCodec::H264 || !frame.keyframe {
        return Err("H.264 encoder unavailable".into());
    }
    std::io::stdout().write_all(&frame.bytes)?;
    Ok(())
}
