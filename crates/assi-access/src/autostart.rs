#[cfg(any(not(windows), test))]
use crate::Error;
use crate::Result;
use std::path::Path;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::{fs, io::Write, path::PathBuf};

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn registration_path() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        return Ok(
            PathBuf::from(std::env::var_os("HOME").ok_or(Error::Autostart)?)
                .join("Library/LaunchAgents/rf.assi.client.plist"),
        );
    }
    #[cfg(target_os = "linux")]
    {
        let base = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(path) if Path::new(&path).is_absolute() => PathBuf::from(path),
            _ => PathBuf::from(std::env::var_os("HOME").ok_or(Error::Autostart)?).join(".config"),
        };
        return Ok(base.join("autostart/rf.assi.client.desktop"));
    }
    #[allow(unreachable_code)]
    Err(Error::Unsupported)
}
/// Reports registration, not whether the operating system has disabled the item.
pub fn autostart_enabled() -> Result<bool> {
    #[cfg(windows)]
    {
        crate::windows::autostart_enabled()
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        match fs::symlink_metadata(registration_path()?) {
            Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(true),
            Ok(_) => Err(Error::Autostart),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err(Error::Autostart),
        }
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        Err(Error::Unsupported)
    }
}
/// Registers only at the current user's login. Never starts a process immediately.
/// The executable argument comes from current_exe, not from untrusted remote data.
pub fn set_autostart(enabled: bool, executable: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        crate::windows::set_autostart(enabled, executable)
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let path = registration_path()?;
        if !enabled {
            return match fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(_) => Err(Error::Autostart),
            };
        }
        let executable = executable.canonicalize().map_err(|_| Error::Autostart)?;
        if !executable.is_file() {
            return Err(Error::Autostart);
        }
        let text = executable.to_str().ok_or(Error::Autostart)?;
        #[cfg(target_os = "macos")]
        let body = mac_registration(text)?;
        #[cfg(target_os = "linux")]
        let body = linux_registration(text)?;
        let dir = path.parent().ok_or(Error::Autostart)?;
        fs::create_dir_all(dir).map_err(|_| Error::Autostart)?;
        let mut file = tempfile::NamedTempFile::new_in(dir).map_err(|_| Error::Autostart)?;
        file.write_all(body.as_bytes())
            .map_err(|_| Error::Autostart)?;
        file.as_file().sync_all().map_err(|_| Error::Autostart)?;
        file.persist(path).map_err(|_| Error::Autostart)?;
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = (enabled, executable);
        Err(Error::Unsupported)
    }
}
#[cfg(any(target_os = "macos", test))]
fn mac_registration(executable: &str) -> Result<String> {
    if executable.chars().any(char::is_control) || !executable.starts_with('/') {
        return Err(Error::Autostart);
    }
    let escaped = executable
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;");
    Ok(format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>rf.assi.client</string><key>ProgramArguments</key><array><string>{escaped}</string></array><key>RunAtLoad</key><true/><key>ProcessType</key><string>Interactive</string></dict></plist>\n"))
}
#[cfg(any(target_os = "linux", test))]
fn linux_registration(executable: &str) -> Result<String> {
    if executable.chars().any(char::is_control)
        || !executable.starts_with('/')
        || executable.contains('=')
    {
        return Err(Error::Autostart);
    }
    // Desktop Entry has two escaping passes: quoted argv, then string values.
    let quoted = executable
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$")
        .replace('%', "%%")
        .replace('\\', "\\\\");
    Ok(format!("[Desktop Entry]\nType=Application\nName=Асси\nExec=\"{quoted}\"\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn launchagent_arguments_are_xml_not_shell() {
        let text = mac_registration("/Applications/A & B <test> \"app\"/Асси").unwrap();
        assert!(text.contains("A &amp; B &lt;test&gt; &quot;app&quot;"));
        assert!(!text.contains("/bin/sh"));
        assert!(!text.contains("KeepAlive"));
        assert!(mac_registration("/tmp/x\n<key>RunAtLoad</key>").is_err());
    }
    #[test]
    fn desktop_exec_escapes_field_codes_and_metacharacters() {
        let text = linux_registration("/tmp/a $b `c` %f \\ d\"e").unwrap();
        assert!(text.contains("%%f"));
        assert!(text.contains("\\\\$b"));
        assert!(text.contains("\\\\`c\\\\`"));
        assert!(text.contains("\\\\\\\\ d\\\\\"e"));
        assert!(linux_registration("/tmp/a=b").is_err());
        assert!(linux_registration("/tmp/a\nExec=bad").is_err());
    }
}
