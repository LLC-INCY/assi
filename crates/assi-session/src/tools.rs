//! Bounded per-session text tools. Clipboard permission is independent of input.
use crate::{wire::Control, Error, Inner, Result, Role, SessionEngine, SessionState};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub const MAX_CLIPBOARD_BYTES: usize = 4096;
pub const MAX_CHAT_BYTES: usize = 2048;
pub const MAX_CHAT_MESSAGES: usize = 100;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatAuthor {
    Host,
    Viewer,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatMessage {
    pub sequence: u64,
    pub author: ChatAuthor,
    pub text: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct ChatSnapshot {
    pub request_id: String,
    pub messages: Vec<ChatMessage>,
}
#[derive(Debug, Clone, Serialize)]
pub struct ClipboardBatch {
    pub request_id: String,
    pub epoch: u64,
    pub enabled: bool,
    pub text: Option<String>,
}
#[derive(Default)]
pub(crate) struct TextTools {
    clipboard: Option<String>,
    chat: VecDeque<ChatMessage>,
    sequence: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ToolMessage {
    ClipboardStatus { epoch: u64, enabled: bool },
    Clipboard { epoch: u64, text: String },
    Chat { text: String },
    ChatEvent { message: ChatMessage },
}
fn text_ok(text: &str, limit: usize) -> bool {
    text.len() <= limit
        && !text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
}
pub(crate) fn active(i: &Inner, request_id: &str) -> Result<()> {
    if i.snapshot.state != SessionState::Active
        || i.snapshot.request_id.as_deref() != Some(request_id)
    {
        return Err(Error::InvalidState);
    }
    Ok(())
}
pub(crate) fn send(i: &Inner, control: Control) -> Result<()> {
    let len = serde_json::to_vec(&control)
        .map_err(|_| Error::Protocol)?
        .len();
    if len + 1 > crate::wire::MAX_CONTROL {
        return Err(Error::Protocol);
    }
    i.annotation_outbound
        .as_ref()
        .ok_or(Error::InvalidState)?
        .try_send(control)
        .map_err(|_| Error::Backpressure)
}
impl SessionEngine {
    pub async fn set_clipboard_enabled(&self, request_id: String, enabled: bool) -> Result<()> {
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Host {
            return Err(Error::PermissionDenied);
        }
        if i.snapshot.clipboard_enabled == enabled {
            return Ok(());
        }
        let epoch = i
            .snapshot
            .clipboard_epoch
            .checked_add(1)
            .ok_or(Error::Protocol)?;
        if !enabled {
            // Revocation is immediate even when a slow peer fills the control queue.
            i.snapshot.clipboard_epoch = epoch;
            i.snapshot.clipboard_enabled = false;
            i.tools.clipboard = None;
        }
        let result = send(
            &i,
            Control::Tool {
                message: ToolMessage::ClipboardStatus { epoch, enabled },
            },
        );
        if result.is_err() && !enabled {
            if let Some(cancel) = &i.cancel {
                let _ = cancel.send(true);
            }
        }
        result?;
        i.snapshot.clipboard_epoch = epoch;
        i.snapshot.clipboard_enabled = enabled;
        i.tools.clipboard = None;
        Ok(())
    }
    pub async fn send_clipboard(&self, request_id: String, text: String) -> Result<()> {
        if !text_ok(&text, MAX_CLIPBOARD_BYTES) {
            return Err(Error::InvalidInput);
        }
        let i = self.lock()?;
        active(&i, &request_id)?;
        if !i.snapshot.clipboard_enabled {
            return Err(Error::PermissionDenied);
        }
        send(
            &i,
            Control::Tool {
                message: ToolMessage::Clipboard {
                    epoch: i.snapshot.clipboard_epoch,
                    text,
                },
            },
        )
    }
    pub fn take_clipboard(&self, request_id: String) -> Result<ClipboardBatch> {
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        Ok(ClipboardBatch {
            request_id,
            epoch: i.snapshot.clipboard_epoch,
            enabled: i.snapshot.clipboard_enabled,
            text: i.tools.clipboard.take(),
        })
    }
    pub fn chat(&self, request_id: String) -> Result<ChatSnapshot> {
        let i = self.lock()?;
        active(&i, &request_id)?;
        Ok(ChatSnapshot {
            request_id,
            messages: i.tools.chat.iter().cloned().collect(),
        })
    }
    pub async fn send_chat(&self, request_id: String, text: String) -> Result<()> {
        if text.trim().is_empty() || !text_ok(&text, MAX_CHAT_BYTES) {
            return Err(Error::InvalidInput);
        }
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role == Role::Host {
            Self::append_chat(&mut i, ChatAuthor::Host, text)
        } else {
            send(
                &i,
                Control::Tool {
                    message: ToolMessage::Chat { text },
                },
            )
        }
    }
    fn append_chat(i: &mut Inner, author: ChatAuthor, text: String) -> Result<()> {
        if text.trim().is_empty() || !text_ok(&text, MAX_CHAT_BYTES) {
            return Err(Error::InvalidInput);
        }
        let message = ChatMessage {
            sequence: i.tools.sequence.checked_add(1).ok_or(Error::Protocol)?,
            author,
            text,
        };
        send(
            i,
            Control::Tool {
                message: ToolMessage::ChatEvent {
                    message: message.clone(),
                },
            },
        )?;
        Self::store_chat(i, message);
        Ok(())
    }
    fn store_chat(i: &mut Inner, message: ChatMessage) {
        i.tools.sequence = message.sequence;
        if i.tools.chat.len() == MAX_CHAT_MESSAGES {
            i.tools.chat.pop_front();
        }
        i.tools.chat.push_back(message);
    }
    pub(crate) fn receive_tool(&self, generation: u64, message: ToolMessage) -> Result<()> {
        let mut i = self.lock()?;
        if i.generation != generation || i.snapshot.state != SessionState::Active {
            return Err(Error::InvalidState);
        }
        match message {
            ToolMessage::ClipboardStatus { epoch, enabled } if i.snapshot.role == Role::Viewer => {
                if epoch <= i.snapshot.clipboard_epoch {
                    return Err(Error::Protocol);
                }
                i.snapshot.clipboard_epoch = epoch;
                i.snapshot.clipboard_enabled = enabled;
                i.tools.clipboard = None;
                Ok(())
            }
            ToolMessage::Clipboard { epoch, text } => {
                // Revocation may cross an already transmitted clipboard message. Discard it.
                if epoch < i.snapshot.clipboard_epoch {
                    return Ok(());
                }
                if !i.snapshot.clipboard_enabled || epoch != i.snapshot.clipboard_epoch {
                    return Err(Error::PermissionDenied);
                }
                if !text_ok(&text, MAX_CLIPBOARD_BYTES) {
                    return Err(Error::InvalidInput);
                }
                i.tools.clipboard = Some(text);
                Ok(())
            }
            ToolMessage::Chat { text } if i.snapshot.role == Role::Host => {
                Self::append_chat(&mut i, ChatAuthor::Viewer, text)
            }
            ToolMessage::ChatEvent { message } if i.snapshot.role == Role::Viewer => {
                if message.sequence != i.tools.sequence + 1
                    || message.text.trim().is_empty()
                    || !text_ok(&message.text, MAX_CHAT_BYTES)
                {
                    return Err(Error::Protocol);
                }
                Self::store_chat(&mut i, message);
                Ok(())
            }
            _ => Err(Error::PermissionDenied),
        }
    }
}
