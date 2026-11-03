//! Opt-in session PTY. The caller grants permission locally before start_approved.
//! No elevated execution, remote executable selection, or persistent shell.
use thiserror::Error;
pub const MAX_CHUNK: usize = 1024;
pub const MAX_BUFFER: usize = 64 * 1024;
#[derive(Debug, Error)]
pub enum Error {
    #[error("Терминал недоступен на этой платформе")]
    Unsupported,
    #[error("Нет активного разрешённого терминала")]
    InvalidState,
    #[error("Некорректный размер терминала или пакет")]
    InvalidInput,
    #[error("Терминал занят или переполнен")]
    Busy,
    #[error("Не удалось запустить или обслужить терминал")]
    Io,
}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Default)]
pub struct TerminalBatch {
    pub bytes: Vec<u8>,
    pub closed: bool,
    pub error: bool,
}
pub fn valid_size(cols: u16, rows: u16) -> bool {
    (20..=300).contains(&cols) && (5..=120).contains(&rows)
}
#[cfg(all(unix, not(target_os = "android")))]
mod native;
#[cfg(all(unix, not(target_os = "android")))]
pub use native::TerminalManager;
#[cfg(not(all(unix, not(target_os = "android"))))]
#[derive(Default)]
pub struct TerminalManager;
#[cfg(not(all(unix, not(target_os = "android"))))]
impl TerminalManager {
    pub fn supported() -> bool {
        false
    }
    pub fn start_approved(&mut self, _: &str, _: u16, _: u16) -> Result<()> {
        Err(Error::Unsupported)
    }
    pub fn write(&self, _: &str, _: Vec<u8>) -> Result<()> {
        Err(Error::Unsupported)
    }
    pub fn resize(&self, _: &str, _: u16, _: u16) -> Result<()> {
        Err(Error::Unsupported)
    }
    pub fn drain(&self, _: &str) -> Result<TerminalBatch> {
        Err(Error::Unsupported)
    }
    pub fn stop(&mut self) {}
    pub fn is_stopped(&self) -> bool {
        true
    }
    pub fn finish_stop(&mut self, _: std::time::Duration) -> bool {
        true
    }
}
