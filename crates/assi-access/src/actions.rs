use crate::{Error, Result};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SystemAction {
    Lock,
    Restart,
}
pub fn system_action_supported(action: SystemAction) -> bool {
    match action {
        SystemAction::Lock => cfg!(any(target_os = "windows", target_os = "linux")),
        SystemAction::Restart => cfg!(any(target_os = "windows", target_os = "linux")),
    }
}
/// Execute only after the caller has verified explicit owner consent for this
/// action in this session. This function is never reachable through free text.
pub fn execute_system_action(action: SystemAction) -> Result<()> {
    #[cfg(windows)]
    {
        if action == SystemAction::Lock {
            // SAFETY: LockWorkStation takes no pointers. It operates on this interactive desktop.
            return if unsafe { windows_sys::Win32::System::Shutdown::LockWorkStation() } != 0 {
                Ok(())
            } else {
                Err(Error::SystemAction)
            };
        }
        use std::os::windows::ffi::OsStringExt;
        let mut buffer = [0u16; 32768];
        // SAFETY: buffer is writable for its declared length; API returns UTF-16 length excluding NUL.
        let length = unsafe {
            windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
                buffer.as_mut_ptr(),
                buffer.len() as u32,
            )
        } as usize;
        if length == 0 || length >= buffer.len() {
            return Err(Error::SystemAction);
        }
        let directory = std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length]));
        return run_bounded(directory.join("shutdown.exe"), &["/r", "/t", "0"]);
    }
    #[cfg(target_os = "linux")]
    {
        let (program, args) = match action {
            SystemAction::Lock => ("/usr/bin/loginctl", ["--no-ask-password", "lock-session"]),
            SystemAction::Restart => ("/usr/bin/systemctl", ["--no-ask-password", "reboot"]),
        };
        return run_bounded(program, &args);
    }
    #[allow(unreachable_code)]
    {
        let _ = action;
        Err(Error::Unsupported)
    }
}
#[cfg(any(windows, target_os = "linux"))]
fn run_bounded(program: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Result<()> {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| Error::SystemAction)?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    Ok(())
                } else {
                    Err(Error::SystemAction)
                }
            }
            Ok(None) if started.elapsed() < Duration::from_secs(5) => {
                std::thread::sleep(Duration::from_millis(25))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::SystemAction);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arbitrary_system_commands_are_not_deserializable() {
        assert!(serde_json::from_str::<SystemAction>("\"rm -rf /\"").is_err());
        assert_eq!(
            serde_json::from_str::<SystemAction>("\"lock\"").unwrap(),
            SystemAction::Lock
        );
    }
}
