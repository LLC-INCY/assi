//! Fixed-size PCM audio; at most 200ms buffered at each endpoint.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, sync::Arc};
use tokio::sync::Notify;
pub const AUDIO_SAMPLE_RATE: u32 = 48_000;
pub const AUDIO_PACKET_SAMPLES: usize = 960;
pub const MAX_AUDIO_PACKETS: usize = 10;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioPacket {
    pub sample_rate: u32,
    pub channels: u8,
    pub pcm: Vec<u8>,
}
impl AudioPacket {
    pub fn validate(&self) -> Result<()> {
        if self.sample_rate != AUDIO_SAMPLE_RATE
            || !matches!(self.channels, 1 | 2)
            || self.pcm.len() != AUDIO_PACKET_SAMPLES * usize::from(self.channels) * 2
        {
            return Err(Error::InvalidAudio);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AudioBatch {
    pub request_id: String,
    pub audio_epoch: u64,
    pub audio_enabled: bool,
    pub packets: Vec<AudioPacket>,
}

pub(crate) struct AudioEnvelope {
    pub epoch: u64,
    pub sequence: u64,
    pub packet: AudioPacket,
}
#[derive(Default)]
pub(crate) struct AudioState {
    pub outgoing: VecDeque<AudioEnvelope>,
    pub incoming: VecDeque<AudioPacket>,
    pub sequence: u64,
    pub received_sequence: u64,
    pub notify: Arc<Notify>,
}
impl AudioState {
    pub fn clear(&mut self) {
        self.outgoing.clear();
        self.incoming.clear();
        self.sequence = 0;
        self.received_sequence = 0;
    }
    pub fn push(&mut self, epoch: u64, packet: AudioPacket) -> Result<()> {
        self.sequence = self.sequence.checked_add(1).ok_or(Error::Protocol)?;
        if self.outgoing.len() == MAX_AUDIO_PACKETS {
            self.outgoing.pop_front();
        }
        self.outgoing.push_back(AudioEnvelope {
            epoch,
            sequence: self.sequence,
            packet,
        });
        self.notify.notify_one();
        Ok(())
    }
    pub fn receive(&mut self, envelope: AudioEnvelope) -> Result<()> {
        envelope.packet.validate()?;
        if envelope.sequence <= self.received_sequence {
            return Err(Error::Protocol);
        }
        self.received_sequence = envelope.sequence;
        if self.incoming.len() == MAX_AUDIO_PACKETS {
            self.incoming.pop_front();
        }
        self.incoming.push_back(envelope.packet);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packet(value: u8) -> AudioPacket {
        AudioPacket {
            sample_rate: AUDIO_SAMPLE_RATE,
            channels: 1,
            pcm: vec![value; 1920],
        }
    }
    #[test]
    fn queues_drop_oldest_and_replayed_sequence_is_rejected() {
        let mut state = AudioState::default();
        for n in 0..30 {
            state.push(7, packet(n)).unwrap();
        }
        assert_eq!(state.outgoing.len(), MAX_AUDIO_PACKETS);
        assert_eq!(state.outgoing.front().unwrap().packet.pcm[0], 20);
        while let Some(envelope) = state.outgoing.pop_front() {
            state.receive(envelope).unwrap();
        }
        assert_eq!(state.incoming.len(), MAX_AUDIO_PACKETS);
        assert!(state
            .receive(AudioEnvelope {
                epoch: 7,
                sequence: 30,
                packet: packet(99)
            })
            .is_err());
        assert_eq!(state.incoming.back().unwrap().pcm[0], 29);
        state.clear();
        assert!(state.incoming.is_empty() && state.outgoing.is_empty());
        assert_eq!(state.received_sequence, 0);
    }
}
