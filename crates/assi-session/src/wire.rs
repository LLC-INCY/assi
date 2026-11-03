use crate::{Error, InputEvent, Result};
use image::ImageDecoder;
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_FRAME: usize = 4 * 1024 * 1024;
pub(crate) const MAX_CONTROL: usize = 8192;
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Control {
    Hello {
        token: String,
        name: String,
    },
    Waiting {
        request_id: String,
        name: String,
    },
    Accepted {
        control: bool,
    },
    Rejected,
    System {
        event: crate::system_actions::SystemEvent,
    },
    Terminal {
        event: crate::terminal::TerminalEvent,
    },
    Tool {
        message: crate::tools::ToolMessage,
    },
    Display {
        message: crate::displays::DisplayMessage,
    },
    Video {
        message: crate::video::VideoMessage,
    },
    AudioStatus {
        epoch: u64,
        enabled: bool,
    },
    Input {
        screen_epoch: u64,
        event: InputEvent,
    },
    AnnotationCommand {
        screen_epoch: u64,
        command: crate::AnnotationCommand,
    },
    AnnotationEvent {
        event: crate::annotations::AnnotationEvent,
    },
    FileOffer {
        id: String,
        name: String,
        size: u64,
    },
    FileAccept {
        id: String,
    },
    FileReject {
        id: String,
    },
    FileCancel {
        id: String,
    },
    FilePause {
        id: String,
    },
    FilePaused {
        id: String,
        offset: u64,
        sha256: String,
    },
    FileResumeRequest {
        id: String,
    },
    FileResume {
        id: String,
        offset: u64,
        sha256: String,
    },
    FileResumed {
        id: String,
        offset: u64,
        sha256: String,
    },
    FileDone {
        id: String,
        sha256: String,
    },
    FileComplete {
        id: String,
    },
    Close,
    Ping,
}
pub(crate) enum Packet {
    Control(Control),
    Frame(Vec<u8>),
    Video(crate::VideoFrame),
    Audio(crate::audio::AudioEnvelope),
    FileChunk {
        id: String,
        offset: u64,
        bytes: Vec<u8>,
    },
}

pub(crate) fn validate_frame(bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_FRAME
        || bytes.len() < 4
        || !bytes.starts_with(&[0xff, 0xd8])
        || !bytes.ends_with(&[0xff, 0xd9])
    {
        return Err(Error::InvalidFrame);
    }
    let decoder = image::codecs::jpeg::JpegDecoder::new(Cursor::new(bytes))
        .map_err(|_| Error::InvalidFrame)?;
    let (width, height) = decoder.dimensions();
    if width == 0
        || height == 0
        || width > 4096
        || height > 4096
        || u64::from(width) * u64::from(height) > 8_388_608
    {
        return Err(Error::InvalidFrame);
    }
    Ok(())
}
pub(crate) async fn read(reader: &mut (impl AsyncRead + Unpin)) -> Result<Packet> {
    let length = reader.read_u32().await? as usize;
    if !(2..=MAX_FRAME + 35).contains(&length) {
        return Err(Error::Protocol);
    }
    let kind = reader.read_u8().await?;
    if !(1..=5).contains(&kind)
        || kind == 1 && length > MAX_CONTROL
        || kind == 3 && !(26..=crate::files::FILE_CHUNK + 25).contains(&length)
        || kind == 4 && !matches!(length, 1938 | 3858)
        || kind == 2 && length > MAX_FRAME + 1
        || kind == 5 && length < 36
    {
        return Err(Error::Protocol);
    }
    let mut body = vec![0; length - 1];
    reader.read_exact(&mut body).await?;
    match kind {
        1 => Ok(Packet::Control(
            serde_json::from_slice(&body).map_err(|_| Error::Protocol)?,
        )),
        2 => {
            validate_frame(&body)?;
            Ok(Packet::Frame(body))
        }
        3 => {
            let id = uuid::Uuid::from_slice(&body[..16])
                .map_err(|_| Error::Protocol)?
                .to_string();
            let offset = u64::from_be_bytes(body[16..24].try_into().map_err(|_| Error::Protocol)?);
            Ok(Packet::FileChunk {
                id,
                offset,
                bytes: body[24..].to_vec(),
            })
        }
        4 => {
            let epoch = u64::from_be_bytes(body[..8].try_into().map_err(|_| Error::Protocol)?);
            let sequence = u64::from_be_bytes(body[8..16].try_into().map_err(|_| Error::Protocol)?);
            let packet = crate::AudioPacket {
                sample_rate: crate::AUDIO_SAMPLE_RATE,
                channels: body[16],
                pcm: body[17..].to_vec(),
            };
            packet.validate()?;
            Ok(Packet::Audio(crate::audio::AudioEnvelope {
                epoch,
                sequence,
                packet,
            }))
        }
        5 => {
            let codec = match body[0] {
                0 => crate::VideoCodec::Jpeg,
                1 => crate::VideoCodec::H264,
                _ => return Err(Error::Protocol),
            };
            let keyframe = match body[1] {
                0 => false,
                1 => true,
                _ => return Err(Error::Protocol),
            };
            let frame = crate::VideoFrame {
                codec,
                keyframe,
                width: u32::from_be_bytes(body[2..6].try_into().map_err(|_| Error::Protocol)?),
                height: u32::from_be_bytes(body[6..10].try_into().map_err(|_| Error::Protocol)?),
                timestamp_us: u64::from_be_bytes(
                    body[10..18].try_into().map_err(|_| Error::Protocol)?,
                ),
                sequence: u64::from_be_bytes(body[18..26].try_into().map_err(|_| Error::Protocol)?),
                screen_epoch: u64::from_be_bytes(
                    body[26..34].try_into().map_err(|_| Error::Protocol)?,
                ),
                bytes: body[34..].to_vec(),
            };
            frame.validate()?;
            Ok(Packet::Video(frame))
        }
        _ => Err(Error::Protocol),
    }
}
pub(crate) async fn control(writer: &mut (impl AsyncWrite + Unpin), value: &Control) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|_| Error::Protocol)?;
    if bytes.len() + 1 > MAX_CONTROL {
        return Err(Error::Protocol);
    }
    writer.write_u32((bytes.len() + 1) as u32).await?;
    writer.write_u8(1).await?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}
pub(crate) async fn frame(writer: &mut (impl AsyncWrite + Unpin), bytes: &[u8]) -> Result<()> {
    writer.write_u32((bytes.len() + 1) as u32).await?;
    writer.write_u8(2).await?;
    writer.write_all(bytes).await?;
    writer.flush().await?;
    Ok(())
}

pub(crate) async fn packet(writer: &mut (impl AsyncWrite + Unpin), packet: Packet) -> Result<()> {
    match packet {
        Packet::Control(message) => control(writer, &message).await,
        Packet::Frame(bytes) => frame(writer, &bytes).await,
        Packet::Video(frame) => {
            frame.validate()?;
            writer.write_u32((frame.bytes.len() + 35) as u32).await?;
            writer.write_u8(5).await?;
            writer
                .write_u8(match frame.codec {
                    crate::VideoCodec::Jpeg => 0,
                    crate::VideoCodec::H264 => 1,
                })
                .await?;
            writer.write_u8(u8::from(frame.keyframe)).await?;
            writer.write_u32(frame.width).await?;
            writer.write_u32(frame.height).await?;
            writer.write_u64(frame.timestamp_us).await?;
            writer.write_u64(frame.sequence).await?;
            writer.write_u64(frame.screen_epoch).await?;
            writer.write_all(&frame.bytes).await?;
            writer.flush().await?;
            Ok(())
        }
        Packet::Audio(envelope) => {
            envelope.packet.validate()?;
            writer
                .write_u32((envelope.packet.pcm.len() + 18) as u32)
                .await?;
            writer.write_u8(4).await?;
            writer.write_u64(envelope.epoch).await?;
            writer.write_u64(envelope.sequence).await?;
            writer.write_u8(envelope.packet.channels).await?;
            writer.write_all(&envelope.packet.pcm).await?;
            writer.flush().await?;
            Ok(())
        }
        Packet::FileChunk { id, offset, bytes } => {
            if bytes.is_empty() || bytes.len() > crate::files::FILE_CHUNK {
                return Err(Error::InvalidFile);
            }
            let id = uuid::Uuid::parse_str(&id).map_err(|_| Error::InvalidFile)?;
            writer.write_u32((bytes.len() + 25) as u32).await?;
            writer.write_u8(3).await?;
            writer.write_all(id.as_bytes()).await?;
            writer.write_u64(offset).await?;
            writer.write_all(&bytes).await?;
            writer.flush().await?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn oversized_packet_rejected_before_payload_allocation() {
        let bytes = ((MAX_FRAME + 36) as u32).to_be_bytes();
        assert!(matches!(
            read(&mut bytes.as_slice()).await,
            Err(Error::Protocol)
        ));
    }
    #[tokio::test]
    async fn unknown_packet_type_rejected() {
        assert!(matches!(
            read(&mut [0, 0, 0, 2, 99, 0].as_slice()).await,
            Err(Error::Protocol)
        ));
    }
    #[tokio::test]
    async fn audio_length_is_rejected_before_reading_body() {
        for length in [2u32, 1937, 1939, 3857, 3859, 65_536] {
            let mut header = length.to_be_bytes().to_vec();
            header.push(4);
            assert!(matches!(
                read(&mut header.as_slice()).await,
                Err(Error::Protocol)
            ));
        }
    }
    #[tokio::test]
    async fn audio_wire_roundtrips_and_rejects_channel_length_mismatch() {
        for channels in [1, 2] {
            let expected = crate::AudioPacket {
                sample_rate: 48_000,
                channels,
                pcm: vec![123; 1920 * usize::from(channels)],
            };
            let mut bytes = Vec::new();
            packet(
                &mut bytes,
                Packet::Audio(crate::audio::AudioEnvelope {
                    epoch: 2,
                    sequence: 7,
                    packet: expected.clone(),
                }),
            )
            .await
            .unwrap();
            match read(&mut bytes.as_slice()).await.unwrap() {
                Packet::Audio(envelope) => {
                    assert_eq!(envelope.epoch, 2);
                    assert_eq!(envelope.sequence, 7);
                    assert_eq!(envelope.packet, expected);
                }
                _ => panic!("wrong packet"),
            }
            bytes[21] = 3 - channels;
            assert!(matches!(
                read(&mut bytes.as_slice()).await,
                Err(Error::InvalidAudio)
            ));
        }
    }
}
