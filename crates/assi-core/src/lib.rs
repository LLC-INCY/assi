//! Local application domain. No network, capture or remote-input capabilities yet.
pub mod consent;
mod store;
pub use store::{Device, DeviceInput, Group, Os, Settings, Snapshot, Store, StoreError, Theme};
