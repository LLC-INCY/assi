//! Typed OS actions require one explicit local-owner decision per request.
//! This module transports requests and results; it never runs OS commands.
use crate::{
    tools::{active, send},
    wire::Control,
    Error, Result, Role, SessionEngine, SessionState,
};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemAction {
    Lock,
    Restart,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemCapabilities {
    pub lock: bool,
    pub restart: bool,
}
impl SystemCapabilities {
    fn allows(self, action: SystemAction) -> bool {
        match action {
            SystemAction::Lock => self.lock,
            SystemAction::Restart => self.restart,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemRequestState {
    Requested,
    Approved,
    Rejected,
    Dispatched,
    Failed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemRequest {
    pub id: String,
    pub action: SystemAction,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemResult {
    pub id: String,
    pub action: SystemAction,
    pub state: SystemRequestState,
}
#[derive(Debug, Clone, Serialize)]
pub struct SystemSnapshot {
    pub request_id: String,
    pub supported: SystemCapabilities,
    pub pending: Option<SystemRequest>,
    pub latest: Option<SystemResult>,
}
#[derive(Default)]
pub(super) struct SystemState {
    capabilities: SystemCapabilities,
    advertised: bool,
    pending: Option<SystemRequest>,
    approved: bool,
    latest: Option<SystemResult>,
    seen: std::collections::HashSet<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum SystemEvent {
    Capabilities { supported: SystemCapabilities },
    Request { request: SystemRequest },
    Decision { id: String, approved: bool },
    Result { id: String, success: bool },
}
fn message(event: SystemEvent) -> Control {
    Control::System { event }
}
impl SessionEngine {
    pub fn set_system_capabilities(
        &self,
        request_id: String,
        supported: SystemCapabilities,
    ) -> Result<()> {
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Host {
            return Err(Error::PermissionDenied);
        }
        if i.system.advertised && i.system.capabilities == supported {
            return Ok(());
        }
        send(&i, message(SystemEvent::Capabilities { supported }))?;
        i.system.capabilities = supported;
        i.system.advertised = true;
        Ok(())
    }
    pub fn system_snapshot(&self, request_id: String) -> Result<SystemSnapshot> {
        let i = self.lock()?;
        active(&i, &request_id)?;
        Ok(SystemSnapshot {
            request_id,
            supported: i.system.capabilities,
            pending: if i.system.approved {
                None
            } else {
                i.system.pending.clone()
            },
            latest: i.system.latest.clone(),
        })
    }
    pub fn request_system_action(
        &self,
        request_id: String,
        action: SystemAction,
    ) -> Result<String> {
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Viewer || !i.system.capabilities.allows(action) {
            return Err(Error::PermissionDenied);
        }
        if i.system.pending.is_some() {
            return Err(Error::InvalidState);
        }
        let request = SystemRequest {
            id: uuid::Uuid::new_v4().to_string(),
            action,
        };
        send(
            &i,
            message(SystemEvent::Request {
                request: request.clone(),
            }),
        )?;
        i.system.latest = Some(SystemResult {
            id: request.id.clone(),
            action,
            state: SystemRequestState::Requested,
        });
        i.system.pending = Some(request.clone());
        i.system.approved = false;
        Ok(request.id)
    }
    /// Only a local host UI decision may call this. Approved action is returned
    /// to the native caller, which must still hold its session lifecycle lock.
    pub fn decide_system_action(
        &self,
        request_id: String,
        id: String,
        approved: bool,
    ) -> Result<Option<SystemAction>> {
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Host {
            return Err(Error::PermissionDenied);
        }
        let request = i.system.pending.clone().ok_or(Error::InvalidState)?;
        if request.id != id || i.system.approved {
            return Err(Error::InvalidState);
        }
        if approved && !i.system.capabilities.allows(request.action) {
            return Err(Error::PermissionDenied);
        }
        send(
            &i,
            message(SystemEvent::Decision {
                id: id.clone(),
                approved,
            }),
        )?;
        i.system.latest = Some(SystemResult {
            id,
            action: request.action,
            state: if approved {
                SystemRequestState::Approved
            } else {
                SystemRequestState::Rejected
            },
        });
        i.system.approved = approved;
        if !approved {
            i.system.pending = None;
        }
        Ok(approved.then_some(request.action))
    }
    pub fn complete_system_action(
        &self,
        request_id: String,
        id: String,
        success: bool,
    ) -> Result<()> {
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Host {
            return Err(Error::PermissionDenied);
        }
        let request = i.system.pending.clone().ok_or(Error::InvalidState)?;
        if request.id != id || !i.system.approved {
            return Err(Error::InvalidState);
        }
        send(
            &i,
            message(SystemEvent::Result {
                id: id.clone(),
                success,
            }),
        )?;
        i.system.latest = Some(SystemResult {
            id,
            action: request.action,
            state: if success {
                SystemRequestState::Dispatched
            } else {
                SystemRequestState::Failed
            },
        });
        i.system.pending = None;
        i.system.approved = false;
        Ok(())
    }
    pub(super) fn receive_system(&self, generation: u64, event: SystemEvent) -> Result<()> {
        let mut i = self.lock()?;
        if i.generation != generation || i.snapshot.state != SessionState::Active {
            return Err(Error::InvalidState);
        }
        match event {
            SystemEvent::Capabilities { supported } if i.snapshot.role == Role::Viewer => {
                i.system.capabilities = supported;
                i.system.advertised = true;
            }
            SystemEvent::Request { request } if i.snapshot.role == Role::Host => {
                if uuid::Uuid::parse_str(&request.id).is_err()
                    || i.system.pending.is_some()
                    || i.system.seen.contains(&request.id)
                    || i.system.seen.len() >= 128
                {
                    return Err(Error::Protocol);
                }
                if !i.system.capabilities.allows(request.action) {
                    return Err(Error::PermissionDenied);
                }
                i.system.seen.insert(request.id.clone());
                i.system.latest = Some(SystemResult {
                    id: request.id.clone(),
                    action: request.action,
                    state: SystemRequestState::Requested,
                });
                i.system.pending = Some(request);
                i.system.approved = false;
            }
            SystemEvent::Decision { id, approved } if i.snapshot.role == Role::Viewer => {
                let request = i.system.pending.clone().ok_or(Error::Protocol)?;
                if request.id != id || i.system.approved {
                    return Err(Error::Protocol);
                }
                i.system.latest = Some(SystemResult {
                    id,
                    action: request.action,
                    state: if approved {
                        SystemRequestState::Approved
                    } else {
                        SystemRequestState::Rejected
                    },
                });
                i.system.approved = approved;
                if !approved {
                    i.system.pending = None;
                }
            }
            SystemEvent::Result { id, success } if i.snapshot.role == Role::Viewer => {
                let request = i.system.pending.clone().ok_or(Error::Protocol)?;
                if request.id != id || !i.system.approved {
                    return Err(Error::Protocol);
                }
                i.system.latest = Some(SystemResult {
                    id,
                    action: request.action,
                    state: if success {
                        SystemRequestState::Dispatched
                    } else {
                        SystemRequestState::Failed
                    },
                });
                i.system.pending = None;
                i.system.approved = false;
            }
            _ => return Err(Error::PermissionDenied),
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;
    fn engine(role: Role) -> (std::sync::Arc<SessionEngine>, mpsc::Receiver<Control>) {
        let engine = SessionEngine::new();
        let (tx, rx) = mpsc::channel(32);
        {
            let mut i = engine.lock().unwrap();
            i.snapshot.role = role;
            i.snapshot.state = SessionState::Active;
            i.snapshot.request_id = Some("session".into());
            i.annotation_outbound = Some(tx);
            i.system.capabilities = SystemCapabilities {
                lock: true,
                restart: true,
            };
        }
        (engine, rx)
    }
    #[test]
    fn approval_is_host_only_bound_to_request_and_single_use() {
        let (host, _rx) = engine(Role::Host);
        let id = uuid::Uuid::new_v4().to_string();
        host.receive_system(
            0,
            SystemEvent::Request {
                request: SystemRequest {
                    id: id.clone(),
                    action: SystemAction::Lock,
                },
            },
        )
        .unwrap();
        assert!(host
            .decide_system_action("stale-session".into(), id.clone(), true)
            .is_err());
        assert!(host
            .decide_system_action("session".into(), uuid::Uuid::new_v4().to_string(), true)
            .is_err());
        assert!(host
            .complete_system_action("session".into(), id.clone(), true)
            .is_err());
        assert_eq!(
            host.decide_system_action("session".into(), id.clone(), true)
                .unwrap(),
            Some(SystemAction::Lock)
        );
        assert!(host
            .decide_system_action("session".into(), id.clone(), true)
            .is_err());
        host.complete_system_action("session".into(), id.clone(), true)
            .unwrap();
        assert!(host
            .complete_system_action("session".into(), id, true)
            .is_err());
        let (viewer, _rx) = engine(Role::Viewer);
        assert!(viewer
            .decide_system_action("session".into(), "x".into(), true)
            .is_err());
    }
    #[test]
    fn completed_request_cannot_be_replayed_into_a_stale_approval() {
        let (host, _rx) = engine(Role::Host);
        let id = uuid::Uuid::new_v4().to_string();
        let event = SystemEvent::Request {
            request: SystemRequest {
                id: id.clone(),
                action: SystemAction::Lock,
            },
        };
        host.receive_system(0, event.clone()).unwrap();
        host.decide_system_action("session".into(), id, false)
            .unwrap();
        assert!(host.receive_system(0, event).is_err());
        assert!(host
            .system_snapshot("session".into())
            .unwrap()
            .pending
            .is_none());
    }
    #[test]
    fn unsupported_and_forged_peer_events_fail_closed() {
        let (host, _rx) = engine(Role::Host);
        host.lock().unwrap().system.capabilities = SystemCapabilities::default();
        assert!(host
            .receive_system(
                0,
                SystemEvent::Request {
                    request: SystemRequest {
                        id: uuid::Uuid::new_v4().to_string(),
                        action: SystemAction::Restart
                    }
                }
            )
            .is_err());
        assert!(host
            .receive_system(
                0,
                SystemEvent::Decision {
                    id: "x".into(),
                    approved: true
                }
            )
            .is_err());
        let (viewer, _rx) = engine(Role::Viewer);
        assert!(viewer
            .receive_system(
                0,
                SystemEvent::Result {
                    id: "x".into(),
                    success: true
                }
            )
            .is_err());
        assert!(serde_json::from_str::<SystemAction>("\"shutdown -h now\"").is_err());
    }
    #[tokio::test]
    async fn consent_roundtrip_over_tls_does_not_execute_any_system_command() {
        use std::time::Duration;
        async fn until(mut check: impl FnMut() -> bool) {
            tokio::time::timeout(Duration::from_secs(3), async {
                while !check() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
        }
        let host = SessionEngine::new();
        let viewer = SessionEngine::new();
        let ip = "127.0.0.1".parse().unwrap();
        let invite = host
            .start_host(std::net::SocketAddr::new(ip, 0), ip, "Owner".into())
            .await
            .unwrap();
        viewer.connect(invite, "Viewer".into()).await.unwrap();
        until(|| {
            host.snapshot().state == SessionState::Pending
                && viewer.snapshot().state == SessionState::Pending
        })
        .await;
        let request = host.snapshot().request_id.unwrap();
        host.accept(request.clone(), false).await.unwrap();
        until(|| {
            host.snapshot().state == SessionState::Active
                && viewer.snapshot().state == SessionState::Active
        })
        .await;
        host.set_system_capabilities(
            request.clone(),
            SystemCapabilities {
                lock: true,
                restart: false,
            },
        )
        .unwrap();
        until(|| {
            viewer
                .system_snapshot(request.clone())
                .unwrap()
                .supported
                .lock
        })
        .await;
        assert!(viewer
            .request_system_action(request.clone(), SystemAction::Restart)
            .is_err());
        let id = viewer
            .request_system_action(request.clone(), SystemAction::Lock)
            .unwrap();
        until(|| {
            host.system_snapshot(request.clone())
                .unwrap()
                .pending
                .is_some()
        })
        .await;
        assert_eq!(
            host.system_snapshot(request.clone())
                .unwrap()
                .latest
                .unwrap()
                .state,
            SystemRequestState::Requested
        );
        assert_eq!(
            host.decide_system_action(request.clone(), id.clone(), true)
                .unwrap(),
            Some(SystemAction::Lock)
        );
        host.complete_system_action(request.clone(), id, true)
            .unwrap();
        until(|| {
            viewer
                .system_snapshot(request.clone())
                .unwrap()
                .latest
                .is_some_and(|value| value.state == SystemRequestState::Dispatched)
        })
        .await;
        host.stop().await.unwrap();
        viewer.stop().await.unwrap();
        assert!(host.system_snapshot(request).is_err());
    }
}
