//! Session-policy foundation only; not an authentication or transport implementation.
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    pub view: bool,
    pub control: bool,
    pub files: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Pending,
    Accepted(Permissions),
    Active(Permissions),
    Closed,
}
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConsentError {
    #[error("The session cannot transition from its current state")]
    InvalidTransition,
    #[error("Granted permissions must be a nonempty subset of requested permissions")]
    InvalidPermissions,
}
pub struct Session {
    requested: Permissions,
    state: State,
}
impl Session {
    pub fn new(requested: Permissions) -> Self {
        Self {
            requested,
            state: State::Pending,
        }
    }
    pub fn state(&self) -> State {
        self.state
    }
    /// The future adapter must call this only after authenticated local-owner consent.
    pub fn accept(&mut self, granted: Permissions) -> Result<(), ConsentError> {
        if self.state != State::Pending {
            return Err(ConsentError::InvalidTransition);
        }
        let nonempty = granted.view || granted.files;
        let control_requires_view = !granted.control || granted.view;
        let subset = (!granted.view || self.requested.view)
            && (!granted.control || self.requested.control)
            && (!granted.files || self.requested.files);
        if !nonempty || !control_requires_view || !subset {
            return Err(ConsentError::InvalidPermissions);
        }
        self.state = State::Accepted(granted);
        Ok(())
    }
    /// Called only when the authenticated transport is ready, after consent.
    pub fn activate(&mut self) -> Result<(), ConsentError> {
        if let State::Accepted(granted) = self.state {
            self.state = State::Active(granted);
            Ok(())
        } else {
            Err(ConsentError::InvalidTransition)
        }
    }
    pub fn close(&mut self) {
        self.state = State::Closed;
    }
    pub fn allows_control(&self) -> bool {
        matches!(self.state, State::Active(Permissions { control: true, .. }))
    }
}
