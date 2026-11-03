//! Terminal transport is independent of keyboard permission; only host grants it.
use crate::{wire::Control, Error, Inner, Result, Role, SessionEngine, SessionState};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
pub const TERMINAL_CHUNK: usize = 1024;
const BUFFER: usize = 64 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TerminalEvent {
    Request,
    Permission { epoch: u64, enabled: bool },
    Input { epoch: u64, bytes: Vec<u8> },
    Resize { epoch: u64, cols: u16, rows: u16 },
    Output { epoch: u64, bytes: Vec<u8> },
    Close { epoch: u64 },
}
#[derive(Clone, Debug)]
pub enum TerminalCommand {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Close,
}
#[derive(Default)]
pub(super) struct TerminalState {
    requested: bool,
    enabled: bool,
    epoch: u64,
    commands: VecDeque<TerminalCommand>,
    output: VecDeque<u8>,
}
#[derive(Clone, Debug, Serialize)]
pub struct TerminalSnapshot {
    pub request_id: String,
    pub requested: bool,
    pub enabled: bool,
    pub epoch: u64,
    pub output: Vec<u8>,
}
fn active(i: &Inner, request: &str) -> Result<()> {
    if i.snapshot.state != SessionState::Active || i.snapshot.request_id.as_deref() != Some(request)
    {
        return Err(Error::InvalidState);
    }
    Ok(())
}
fn send(i: &Inner, event: TerminalEvent) -> Result<()> {
    i.annotation_outbound
        .as_ref()
        .ok_or(Error::InvalidState)?
        .try_send(Control::Terminal { event })
        .map_err(|_| Error::Backpressure)
}
fn bytes_ok(bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() || bytes.len() > TERMINAL_CHUNK {
        return Err(Error::Protocol);
    }
    Ok(())
}
fn size_ok(cols: u16, rows: u16) -> Result<()> {
    if !(20..=300).contains(&cols) || !(5..=120).contains(&rows) {
        return Err(Error::Protocol);
    }
    Ok(())
}
impl SessionEngine {
    pub fn terminal_request(&self, request_id: String) -> Result<()> {
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Viewer {
            return Err(Error::PermissionDenied);
        }
        if i.terminal.enabled || i.terminal.requested {
            return Ok(());
        }
        send(&i, TerminalEvent::Request)?;
        i.terminal.requested = true;
        Ok(())
    }
    /// Called only from local host IPC after starting/stopping its PTY.
    pub fn terminal_allow(&self, request_id: String, enabled: bool) -> Result<()> {
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Host || (enabled && !i.terminal.requested) {
            return Err(Error::PermissionDenied);
        }
        let epoch = i.terminal.epoch.checked_add(1).ok_or(Error::Protocol)?;
        // Revoke locally even if the outbound queue has filled.
        if !enabled {
            i.terminal.enabled = false;
            i.terminal.commands.clear();
            i.terminal.output.clear();
        }
        send(&i, TerminalEvent::Permission { epoch, enabled })?;
        i.terminal = TerminalState {
            epoch,
            enabled,
            ..Default::default()
        };
        Ok(())
    }
    pub fn terminal_input(&self, request_id: String, bytes: Vec<u8>) -> Result<()> {
        bytes_ok(&bytes)?;
        let i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Viewer || !i.terminal.enabled {
            return Err(Error::PermissionDenied);
        }
        send(
            &i,
            TerminalEvent::Input {
                epoch: i.terminal.epoch,
                bytes,
            },
        )
    }
    pub fn terminal_resize(&self, request_id: String, cols: u16, rows: u16) -> Result<()> {
        size_ok(cols, rows)?;
        let i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Viewer || !i.terminal.enabled {
            return Err(Error::PermissionDenied);
        }
        send(
            &i,
            TerminalEvent::Resize {
                epoch: i.terminal.epoch,
                cols,
                rows,
            },
        )
    }
    pub fn terminal_publish(&self, request_id: String, bytes: Vec<u8>) -> Result<()> {
        bytes_ok(&bytes)?;
        let i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Host || !i.terminal.enabled {
            return Err(Error::PermissionDenied);
        }
        send(
            &i,
            TerminalEvent::Output {
                epoch: i.terminal.epoch,
                bytes,
            },
        )
    }
    pub fn terminal_close(&self, request_id: String) -> Result<()> {
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        let result = send(
            &i,
            TerminalEvent::Close {
                epoch: i.terminal.epoch,
            },
        );
        i.terminal.enabled = false;
        i.terminal.requested = false;
        i.terminal.commands.clear();
        if i.snapshot.role == Role::Host {
            i.terminal.commands.push_back(TerminalCommand::Close);
        }
        result
    }
    pub fn take_terminal_commands(&self, request_id: String) -> Result<Vec<TerminalCommand>> {
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Host {
            return Err(Error::PermissionDenied);
        }
        Ok(i.terminal.commands.drain(..).collect())
    }
    /// Drains received output. UI renders text, never interprets output as HTML/URLs.
    pub fn terminal_snapshot(&self, request_id: String) -> Result<TerminalSnapshot> {
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        Ok(TerminalSnapshot {
            request_id,
            requested: i.terminal.requested,
            enabled: i.terminal.enabled,
            epoch: i.terminal.epoch,
            output: i.terminal.output.drain(..).collect(),
        })
    }
    pub(super) fn receive_terminal(&self, generation: u64, event: TerminalEvent) -> Result<()> {
        let mut i = self.lock()?;
        if i.generation != generation || i.snapshot.state != SessionState::Active {
            return Err(Error::InvalidState);
        }
        match event {
            TerminalEvent::Request if i.snapshot.role == Role::Host => {
                if !i.terminal.enabled {
                    i.terminal.requested = true;
                }
            }
            TerminalEvent::Permission { epoch, enabled } if i.snapshot.role == Role::Viewer => {
                if epoch <= i.terminal.epoch {
                    return Err(Error::Protocol);
                }
                i.terminal = TerminalState {
                    epoch,
                    enabled,
                    ..Default::default()
                };
            }
            TerminalEvent::Input { epoch, bytes } if i.snapshot.role == Role::Host => {
                bytes_ok(&bytes)?;
                if epoch != i.terminal.epoch || !i.terminal.enabled {
                    return Ok(());
                }
                if i.terminal.commands.len() >= 32 {
                    return Err(Error::Backpressure);
                }
                i.terminal.commands.push_back(TerminalCommand::Input(bytes));
            }
            TerminalEvent::Resize { epoch, cols, rows } if i.snapshot.role == Role::Host => {
                size_ok(cols, rows)?;
                if epoch != i.terminal.epoch || !i.terminal.enabled {
                    return Ok(());
                }
                if i.terminal.commands.len() >= 32 {
                    return Err(Error::Backpressure);
                }
                i.terminal
                    .commands
                    .push_back(TerminalCommand::Resize { cols, rows });
            }
            TerminalEvent::Output { epoch, bytes } if i.snapshot.role == Role::Viewer => {
                bytes_ok(&bytes)?;
                if epoch != i.terminal.epoch || !i.terminal.enabled {
                    return Ok(());
                }
                if i.terminal.output.len() + bytes.len() > BUFFER {
                    return Err(Error::Backpressure);
                }
                i.terminal.output.extend(bytes);
            }
            TerminalEvent::Close { epoch } => {
                if epoch != i.terminal.epoch {
                    return Ok(());
                }
                i.terminal.enabled = false;
                i.terminal.requested = false;
                i.terminal.commands.clear();
                if i.snapshot.role == Role::Host {
                    i.terminal.commands.push_back(TerminalCommand::Close);
                }
            }
            _ => return Err(Error::PermissionDenied),
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn engine(
        role: Role,
    ) -> (
        std::sync::Arc<SessionEngine>,
        tokio::sync::mpsc::Receiver<Control>,
    ) {
        let e = SessionEngine::new();
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        {
            let mut i = e.lock().unwrap();
            i.snapshot.role = role;
            i.snapshot.state = SessionState::Active;
            i.snapshot.request_id = Some("session".into());
            i.annotation_outbound = Some(tx);
        }
        (e, rx)
    }
    #[test]
    fn input_permission_does_not_authorize_terminal() {
        let (e, _rx) = engine(Role::Host);
        e.lock().unwrap().snapshot.control = true;
        e.receive_terminal(
            0,
            TerminalEvent::Input {
                epoch: 0,
                bytes: b"whoami\n".to_vec(),
            },
        )
        .unwrap();
        assert!(e
            .take_terminal_commands("session".into())
            .unwrap()
            .is_empty());
        assert!(e.terminal_allow("session".into(), true).is_err());
        e.receive_terminal(0, TerminalEvent::Request).unwrap();
        e.terminal_allow("session".into(), true).unwrap();
        e.receive_terminal(
            0,
            TerminalEvent::Input {
                epoch: 1,
                bytes: b"pwd\n".to_vec(),
            },
        )
        .unwrap();
        assert_eq!(e.take_terminal_commands("session".into()).unwrap().len(), 1);
        e.terminal_allow("session".into(), false).unwrap();
        e.receive_terminal(
            0,
            TerminalEvent::Input {
                epoch: 1,
                bytes: b"pwd\n".to_vec(),
            },
        )
        .unwrap();
        assert!(e
            .take_terminal_commands("session".into())
            .unwrap()
            .is_empty());
    }
    #[test]
    fn malicious_role_stale_session_and_limits_rejected() {
        let (e, _rx) = engine(Role::Host);
        assert!(e
            .receive_terminal(
                0,
                TerminalEvent::Permission {
                    epoch: 1,
                    enabled: true
                }
            )
            .is_err());
        assert!(e.receive_terminal(1, TerminalEvent::Request).is_err());
        assert!(e.terminal_snapshot("old".into()).is_err());
        assert!(bytes_ok(&vec![0; TERMINAL_CHUNK + 1]).is_err());
        assert!(size_ok(0, 24).is_err());
    }
    #[test]
    fn output_bounded_and_drained() {
        let (e, _rx) = engine(Role::Viewer);
        e.receive_terminal(
            0,
            TerminalEvent::Permission {
                epoch: 1,
                enabled: true,
            },
        )
        .unwrap();
        for _ in 0..64 {
            e.receive_terminal(
                0,
                TerminalEvent::Output {
                    epoch: 1,
                    bytes: vec![65; 1024],
                },
            )
            .unwrap();
        }
        assert!(e
            .receive_terminal(
                0,
                TerminalEvent::Output {
                    epoch: 1,
                    bytes: vec![65]
                }
            )
            .is_err());
        assert_eq!(
            e.terminal_snapshot("session".into()).unwrap().output.len(),
            BUFFER
        );
        assert!(e
            .terminal_snapshot("session".into())
            .unwrap()
            .output
            .is_empty());
    }
}
