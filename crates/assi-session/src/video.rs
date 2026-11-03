//! Ordered bounded video transport. H264 decoding is negotiated by the viewer.
use crate::{
    tools::{active, send},
    wire::Control,
    Error, Result, Role, SessionEngine, SessionState,
};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use tokio::sync::mpsc;
pub const MAX_VIDEO_FRAMES: usize = 8;
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodec {
    #[default]
    Jpeg,
    H264,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VideoFrame {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub timestamp_us: u64,
    pub keyframe: bool,
    pub sequence: u64,
    pub screen_epoch: u64,
    pub bytes: Vec<u8>,
}
impl VideoFrame {
    pub fn validate(&self) -> Result<()> {
        if self.width == 0
            || self.height == 0
            || self.width > 4096
            || self.height > 4096
            || u64::from(self.width) * u64::from(self.height) > 8_388_608
            || self.bytes.is_empty()
            || self.bytes.len() > crate::wire::MAX_FRAME
        {
            return Err(Error::InvalidFrame);
        }
        match self.codec {
            VideoCodec::Jpeg => {
                crate::wire::validate_frame(&self.bytes)?;
                use image::ImageDecoder;
                let decoder =
                    image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(&self.bytes))
                        .map_err(|_| Error::InvalidFrame)?;
                if decoder.dimensions() != (self.width, self.height) || !self.keyframe {
                    return Err(Error::InvalidFrame);
                }
            }
            VideoCodec::H264 => {
                if !self.bytes.starts_with(&[0, 0, 0, 1]) && !self.bytes.starts_with(&[0, 0, 1]) {
                    return Err(Error::InvalidFrame);
                }
            }
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum VideoMessage {
    Codec { codec: VideoCodec },
    Keyframe,
}
#[derive(Default)]
pub(crate) struct VideoState {
    pub sender: Option<mpsc::Sender<VideoFrame>>,
    incoming: VecDeque<VideoFrame>,
    last: Option<VideoFrame>,
    sequence: u64,
    received_sequence: u64,
    waiting_keyframe: bool,
    negotiated_h264: bool,
    force_keyframe: bool,
    require_keyframe: bool,
    pub published_epoch: u64,
    pub received_epoch: u64,
}
impl VideoState {
    pub(crate) fn clear_for_screen(&mut self) {
        self.incoming.clear();
        self.last = None;
        self.force_keyframe = true;
        self.require_keyframe = true;
        self.waiting_keyframe = true;
        self.published_epoch = 0;
        self.received_epoch = 0;
    }
}
impl SessionEngine {
    pub async fn set_video_codec(&self, request_id: String, codec: VideoCodec) -> Result<()> {
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Viewer {
            return Err(Error::PermissionDenied);
        }
        send(
            &i,
            Control::Video {
                message: VideoMessage::Codec { codec },
            },
        )?;
        i.snapshot.video_codec = codec;
        i.video.negotiated_h264 |= codec == VideoCodec::H264;
        i.video.clear_for_screen();
        Ok(())
    }
    pub async fn request_video_keyframe(&self, request_id: String) -> Result<()> {
        let i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Viewer {
            return Err(Error::PermissionDenied);
        }
        send(
            &i,
            Control::Video {
                message: VideoMessage::Keyframe,
            },
        )
    }
    pub fn take_video_keyframe_request(&self, request_id: String) -> Result<bool> {
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Host {
            return Err(Error::PermissionDenied);
        }
        Ok(std::mem::take(&mut i.video.force_keyframe))
    }
    /// Queue one frame. Backpressure requests a fresh IDR; the caller must not tear down the session.
    /// Sequence belongs to the engine. screen_epoch=0 means the current host screen under its lifecycle lock.
    pub fn publish_video(&self, request_id: String, mut frame: VideoFrame) -> Result<()> {
        frame.validate()?;
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Host
            || (frame.codec == VideoCodec::H264 && i.snapshot.video_codec != VideoCodec::H264)
        {
            return Err(Error::PermissionDenied);
        }
        if frame.screen_epoch != 0 && frame.screen_epoch != i.snapshot.screen_epoch {
            return Err(Error::InvalidState);
        }
        if i.video.require_keyframe && !frame.keyframe {
            i.video.force_keyframe = true;
            return Err(Error::Backpressure);
        }
        frame.sequence = i.video.sequence.checked_add(1).ok_or(Error::Protocol)?;
        frame.screen_epoch = i.snapshot.screen_epoch;
        match i
            .video
            .sender
            .as_ref()
            .ok_or(Error::InvalidState)?
            .try_send(frame.clone())
        {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                i.video.force_keyframe = true;
                i.video.require_keyframe = true;
                return Err(Error::Backpressure);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => return Err(Error::InvalidState),
        }
        i.video.require_keyframe = false;
        i.video.sequence = frame.sequence;
        i.video.published_epoch = frame.screen_epoch;
        if frame.codec == VideoCodec::Jpeg {
            i.frame = Some(frame.bytes.clone());
        }
        if i.video.incoming.len() == MAX_VIDEO_FRAMES {
            i.video.incoming.pop_front();
        }
        i.video.incoming.push_back(frame.clone());
        i.video.last = Some(frame);
        Ok(())
    }
    /// Return the oldest undecoded frame after sequence. Caller advances after successful presentation.
    pub fn latest_video(
        &self,
        request_id: String,
        after_sequence: u64,
    ) -> Result<Option<VideoFrame>> {
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        while i
            .video
            .incoming
            .front()
            .is_some_and(|f| f.sequence <= after_sequence)
        {
            i.video.incoming.pop_front();
        }
        if i.snapshot.role == Role::Host
            && i.video
                .incoming
                .front()
                .is_some_and(|f| !f.keyframe && f.sequence != after_sequence + 1)
        {
            while i.video.incoming.front().is_some_and(|f| !f.keyframe) {
                i.video.incoming.pop_front();
            }
            if i.video.incoming.is_empty() {
                i.video.force_keyframe = true;
                i.video.require_keyframe = true;
            }
        }
        Ok(i.video.incoming.front().cloned())
    }
    pub(crate) fn receive_video_message(
        &self,
        generation: u64,
        message: VideoMessage,
    ) -> Result<()> {
        let mut i = self.lock()?;
        if i.generation != generation || i.snapshot.state != SessionState::Active {
            return Err(Error::InvalidState);
        }
        if i.snapshot.role != Role::Host {
            return Err(Error::PermissionDenied);
        }
        match message {
            VideoMessage::Codec { codec } => {
                i.snapshot.video_codec = codec;
                i.video.force_keyframe = true;
                i.video.require_keyframe = true;
            }
            VideoMessage::Keyframe => {
                i.video.force_keyframe = true;
                i.video.require_keyframe = true;
            }
        }
        Ok(())
    }
    pub(crate) fn receive_video(&self, generation: u64, frame: VideoFrame) -> Result<()> {
        frame.validate()?;
        let mut i = self.lock()?;
        if i.generation != generation || i.snapshot.state != SessionState::Active {
            return Err(Error::InvalidState);
        }
        if i.snapshot.role != Role::Viewer {
            return Err(Error::PermissionDenied);
        }
        if frame.codec == VideoCodec::H264 && i.snapshot.video_codec != VideoCodec::H264 {
            // A decoder fallback can race bytes already in the transport. They
            // were previously negotiated but must no longer reach the decoder.
            return if i.video.negotiated_h264 {
                Ok(())
            } else {
                Err(Error::PermissionDenied)
            };
        }
        if frame.screen_epoch < i.snapshot.screen_epoch {
            return Ok(());
        }
        if frame.screen_epoch != i.snapshot.screen_epoch
            || frame.sequence <= i.video.received_sequence
        {
            return Err(Error::Protocol);
        }
        i.video.received_sequence = frame.sequence;
        if i.video.incoming.len() >= MAX_VIDEO_FRAMES {
            i.video.incoming.clear();
            i.video.waiting_keyframe = true;
            if !frame.keyframe {
                send(
                    &i,
                    Control::Video {
                        message: VideoMessage::Keyframe,
                    },
                )?;
            }
        }
        if i.video.waiting_keyframe && !frame.keyframe {
            return Ok(());
        }
        i.video.waiting_keyframe = false;
        i.video.received_epoch = frame.screen_epoch;
        if frame.codec == VideoCodec::Jpeg {
            i.frame = Some(frame.bytes.clone());
        }
        i.video.incoming.push_back(frame);
        Ok(())
    }
}
