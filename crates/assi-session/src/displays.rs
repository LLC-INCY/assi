//! Monitor changes invalidate all old-coordinate input until a displayed frame is acknowledged.
use crate::{
    tools::{active, send},
    wire::Control,
    Error, InputEvent, Result, Role, SessionEngine, SessionState,
};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteDisplay {
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}
#[derive(Default)]
pub(crate) struct DisplayState {
    pub request: Option<String>,
    pub acknowledged_epoch: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum DisplayMessage {
    Status {
        displays: Vec<RemoteDisplay>,
        selected: Option<String>,
        epoch: u64,
        pending: bool,
    },
    Request {
        id: String,
    },
    Acknowledge {
        epoch: u64,
    },
}
fn valid(displays: &[RemoteDisplay], selected: &Option<String>) -> bool {
    displays.len() <= 16
        && displays.iter().enumerate().all(|(n, d)| {
            !d.id.is_empty()
                && d.id.len() <= 128
                && d.name.len() <= 160
                && !d.id.chars().any(char::is_control)
                && !d.name.chars().any(char::is_control)
                && d.width > 0
                && d.height > 0
                && d.width <= 32768
                && d.height <= 32768
                && !displays[..n].iter().any(|prior| prior.id == d.id)
        })
        && selected
            .as_ref()
            .is_none_or(|id| displays.iter().any(|d| &d.id == id))
}
fn send_status(i: &crate::Inner) -> Result<()> {
    send(
        i,
        Control::Display {
            message: DisplayMessage::Status {
                displays: i.snapshot.displays.clone(),
                selected: i.snapshot.selected_display.clone(),
                epoch: i.snapshot.screen_epoch,
                pending: i.snapshot.screen_pending,
            },
        },
    )
}
impl SessionEngine {
    pub fn set_displays(
        &self,
        request_id: String,
        displays: Vec<RemoteDisplay>,
        selected: String,
    ) -> Result<()> {
        let selected = Some(selected);
        if !valid(&displays, &selected) {
            return Err(Error::InvalidInput);
        }
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Host {
            return Err(Error::PermissionDenied);
        }
        // This is enumeration, not permission to silently change the captured monitor.
        if i.snapshot.selected_display.is_some() && i.snapshot.selected_display != selected {
            return Err(Error::InvalidState);
        }
        let message = Control::Display {
            message: DisplayMessage::Status {
                displays: displays.clone(),
                selected: selected.clone(),
                epoch: i.snapshot.screen_epoch,
                pending: i.snapshot.screen_pending,
            },
        };
        send(&i, message)?;
        i.snapshot.displays = displays;
        i.snapshot.selected_display = selected;
        Ok(())
    }
    pub async fn request_display(&self, request_id: String, display_id: String) -> Result<()> {
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Viewer {
            return Err(Error::PermissionDenied);
        }
        if i.snapshot.screen_pending || !i.snapshot.displays.iter().any(|d| d.id == display_id) {
            return Err(Error::InvalidState);
        }
        send(
            &i,
            Control::Display {
                message: DisplayMessage::Request { id: display_id },
            },
        )?;
        i.snapshot.screen_pending = true;
        Ok(())
    }
    pub fn take_display_request(&self, request_id: String) -> Result<Option<String>> {
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Host {
            return Err(Error::PermissionDenied);
        }
        Ok(i.display.request.take())
    }
    /// Native host calls only after release_inputs and selecting the new monitor under its lifecycle lock.
    pub fn commit_display(&self, request_id: String, display_id: String) -> Result<u64> {
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Host {
            return Err(Error::PermissionDenied);
        }
        if !i.snapshot.displays.iter().any(|d| d.id == display_id) {
            return Err(Error::InvalidInput);
        }
        let epoch = i
            .snapshot
            .screen_epoch
            .checked_add(1)
            .ok_or(Error::Protocol)?;
        send(
            &i,
            Control::Display {
                message: DisplayMessage::Status {
                    displays: i.snapshot.displays.clone(),
                    selected: Some(display_id.clone()),
                    epoch,
                    pending: true,
                },
            },
        )?;
        i.snapshot.screen_epoch = epoch;
        i.snapshot.screen_pending = true;
        i.snapshot.selected_display = Some(display_id);
        i.display.request = None;
        i.display.acknowledged_epoch = 0;
        i.inputs.clear();
        i.inputs.push_back(InputEvent::ReleaseAll);
        i.video.clear_for_screen();
        i.frame = None;
        if let Some(frames) = &i.frames {
            frames.send_replace(None);
        }
        // A monitor switch changes drawing coordinates as well.
        let revision = i
            .annotations
            .revision
            .checked_add(1)
            .ok_or(Error::Protocol)?;
        let event = crate::annotations::AnnotationEvent {
            revision,
            author: crate::AnnotationAuthor::Host,
            command: crate::AnnotationCommand::Clear,
        };
        crate::tools::send(
            &i,
            Control::AnnotationEvent {
                event: event.clone(),
            },
        )?;
        i.annotations.apply(&event)?;
        Ok(epoch)
    }
    pub fn reject_display(&self, request_id: String) -> Result<()> {
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Host {
            return Err(Error::PermissionDenied);
        }
        i.display.request = None;
        i.snapshot.screen_pending = false;
        i.display.acknowledged_epoch = i.snapshot.screen_epoch;
        send_status(&i)
    }
    pub async fn acknowledge_screen(&self, request_id: String, screen_epoch: u64) -> Result<()> {
        let mut i = self.lock()?;
        active(&i, &request_id)?;
        if i.snapshot.role != Role::Viewer {
            return Err(Error::PermissionDenied);
        }
        if screen_epoch != i.snapshot.screen_epoch || i.video.received_epoch != screen_epoch {
            return Err(Error::InvalidState);
        }
        send(
            &i,
            Control::Display {
                message: DisplayMessage::Acknowledge {
                    epoch: screen_epoch,
                },
            },
        )?;
        i.snapshot.screen_pending = false;
        Ok(())
    }
    pub(crate) fn receive_display(&self, generation: u64, message: DisplayMessage) -> Result<()> {
        let mut i = self.lock()?;
        if i.generation != generation || i.snapshot.state != SessionState::Active {
            return Err(Error::InvalidState);
        }
        match message {
            DisplayMessage::Status {
                displays,
                selected,
                epoch,
                pending,
            } if i.snapshot.role == Role::Viewer => {
                if !valid(&displays, &selected) || epoch < i.snapshot.screen_epoch || epoch == 0 {
                    return Err(Error::Protocol);
                }
                if epoch != i.snapshot.screen_epoch {
                    i.video.clear_for_screen();
                    i.frame = None;
                }
                i.snapshot.displays = displays;
                i.snapshot.selected_display = selected;
                i.snapshot.screen_epoch = epoch;
                i.snapshot.screen_pending = pending;
                Ok(())
            }
            DisplayMessage::Request { id } if i.snapshot.role == Role::Host => {
                if !i.snapshot.displays.iter().any(|d| d.id == id) {
                    return Err(Error::InvalidInput);
                }
                if i.snapshot.screen_pending {
                    return Ok(());
                }
                i.display.request = Some(id);
                i.snapshot.screen_pending = true;
                i.inputs.clear();
                i.inputs.push_back(InputEvent::ReleaseAll);
                send_status(&i)
            }
            DisplayMessage::Acknowledge { epoch } if i.snapshot.role == Role::Host => {
                if epoch < i.snapshot.screen_epoch {
                    return Ok(());
                }
                if epoch != i.snapshot.screen_epoch || i.video.published_epoch != epoch {
                    return Err(Error::InvalidState);
                }
                i.display.acknowledged_epoch = epoch;
                i.snapshot.screen_pending = false;
                send_status(&i)
            }
            _ => Err(Error::PermissionDenied),
        }
    }
}
