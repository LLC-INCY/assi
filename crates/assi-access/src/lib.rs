//! Explicitly enabled per-user persistent access. No startup registration or
//! OS command runs during construction. This is not a pre-login system service.
mod actions;
mod autostart;
#[cfg(windows)]
mod windows;
pub use actions::{execute_system_action, system_action_supported, SystemAction};
pub use autostart::{autostart_enabled, set_autostart};

use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
};
use thiserror::Error;

const MAX_STORED: u64 = 32 * 1024;
const FILE_NAME: &str = "trusted-access.bin";

#[derive(Debug, Error)]
pub enum Error {
    #[error("Постоянный доступ на этой платформе пока недоступен")]
    Unsupported,
    #[error("Укажите корректный адрес, постоянный порт и имя устройства")]
    InvalidConfig,
    #[error("Не удалось создать защищённый ключ доступа")]
    Crypto,
    #[error("Не удалось безопасно прочитать или сохранить постоянный доступ")]
    Storage,
    #[error("Файл постоянного доступа повреждён или имеет небезопасные права")]
    UnsafeStorage,
    #[error("Не удалось изменить автозапуск")]
    Autostart,
    #[error("Системное действие недоступно или не выполнено")]
    SystemAction,
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessConfig {
    pub bind: SocketAddr,
    pub advertised_ip: IpAddr,
    pub name: String,
    pub allow_control: bool,
}
impl AccessConfig {
    pub fn validate(&self) -> Result<()> {
        let valid_ip = |ip: IpAddr| {
            !ip.is_unspecified()
                && !ip.is_multicast()
                && !matches!(ip, IpAddr::V4(ip) if ip.is_broadcast())
        };
        if self.bind.port() == 0
            || !valid_ip(self.advertised_ip)
            || !valid_ip(self.bind.ip())
            || self.bind.ip() != self.advertised_ip
            || !(1..=60).contains(&self.name.trim().chars().count())
            || self.name.chars().any(char::is_control)
        {
            return Err(Error::InvalidConfig);
        }
        Ok(())
    }
    pub fn endpoint(&self) -> SocketAddr {
        SocketAddr::new(self.advertised_ip, self.bind.port())
    }
}

// Deliberately no Debug or Serialize: the private key and bearer token must never
// be included in status IPC or diagnostic output. Only StoredIdentity is encoded.
pub struct AccessIdentity {
    pub config: AccessConfig,
    pub certificate_der: Vec<u8>,
    pub private_key_pkcs8: Vec<u8>,
    pub token: [u8; 32],
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredIdentity {
    version: u8,
    config: AccessConfig,
    certificate_der: Vec<u8>,
    private_key_pkcs8: Vec<u8>,
    token: [u8; 32],
}
impl StoredIdentity {
    fn validate(&self) -> Result<()> {
        self.config.validate()?;
        if self.version != 1
            || self.certificate_der.is_empty()
            || self.certificate_der.len() > 4096
            || self.private_key_pkcs8.is_empty()
            || self.private_key_pkcs8.len() > 4096
            || self.token == [0; 32]
        {
            return Err(Error::UnsafeStorage);
        }
        Ok(())
    }
    fn into_identity(self) -> AccessIdentity {
        AccessIdentity {
            config: self.config,
            certificate_der: self.certificate_der,
            private_key_pkcs8: self.private_key_pkcs8,
            token: self.token,
        }
    }
}

pub struct AccessStore {
    dir: PathBuf,
}
impl AccessStore {
    /// `dir` must be a dedicated private subdirectory of application data.
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
    pub fn enable(&self, config: AccessConfig) -> Result<AccessIdentity> {
        if cfg!(target_os = "android") {
            return Err(Error::Unsupported);
        }
        config.validate()?;
        let key = rcgen::generate_simple_self_signed(vec!["assi.local".into()])
            .map_err(|_| Error::Crypto)?;
        let mut token = [0; 32];
        SystemRandom::new()
            .fill(&mut token)
            .map_err(|_| Error::Crypto)?;
        let stored = StoredIdentity {
            version: 1,
            config,
            certificate_der: key.cert.der().to_vec(),
            private_key_pkcs8: key.signing_key.serialize_der(),
            token,
        };
        self.write(&stored)?;
        Ok(stored.into_identity())
    }
    pub fn load(&self) -> Result<Option<AccessIdentity>> {
        if !self.dir.try_exists().map_err(|_| Error::Storage)? {
            return Ok(None);
        }
        check_private(&self.dir, true)?;
        let path = self.dir.join(FILE_NAME);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(Error::Storage),
        };
        if metadata.len() > MAX_STORED {
            return Err(Error::UnsafeStorage);
        }
        check_private(&path, false)?;
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(path).map_err(|_| Error::Storage)?;
        check_metadata(&file.metadata().map_err(|_| Error::Storage)?, false)?;
        let mut bytes = Vec::new();
        file.take(MAX_STORED + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Storage)?;
        if bytes.len() as u64 > MAX_STORED {
            return Err(Error::UnsafeStorage);
        }
        #[cfg(windows)]
        let bytes = windows::unprotect(&bytes)?;
        let stored: StoredIdentity =
            serde_json::from_slice(&bytes).map_err(|_| Error::UnsafeStorage)?;
        stored.validate()?;
        Ok(Some(stored.into_identity()))
    }
    /// Removing disk trust must be accompanied by stopping the active listener.
    pub fn revoke(&self) -> Result<()> {
        if !self.dir.try_exists().map_err(|_| Error::Storage)? {
            return Ok(());
        }
        check_private(&self.dir, true)?;
        // remove_file removes a link itself, never its target.
        match fs::remove_file(self.dir.join(FILE_NAME)) {
            Ok(()) => sync_dir(&self.dir),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(Error::Storage),
        }
    }
    fn write(&self, stored: &StoredIdentity) -> Result<()> {
        stored.validate()?;
        if !self.dir.try_exists().map_err(|_| Error::Storage)? {
            #[cfg(unix)]
            let mut builder = fs::DirBuilder::new();
            #[cfg(not(unix))]
            let builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            // Parent app-data directory is created by the application, not recursively here.
            builder.create(&self.dir).map_err(|_| Error::Storage)?;
        }
        check_private(&self.dir, true)?;
        let bytes = serde_json::to_vec(stored).map_err(|_| Error::Storage)?;
        #[cfg(windows)]
        let bytes = windows::protect(&bytes)?;
        if bytes.len() as u64 > MAX_STORED {
            return Err(Error::UnsafeStorage);
        }
        let mut temporary =
            tempfile::NamedTempFile::new_in(&self.dir).map_err(|_| Error::Storage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|_| Error::Storage)?;
        }
        temporary.write_all(&bytes).map_err(|_| Error::Storage)?;
        temporary.as_file().sync_all().map_err(|_| Error::Storage)?;
        temporary
            .persist(self.dir.join(FILE_NAME))
            .map_err(|_| Error::Storage)?;
        sync_dir(&self.dir)
    }
}
fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        fs::File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(|_| Error::Storage)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn check_private(path: &Path, directory: bool) -> Result<()> {
    check_metadata(
        &fs::symlink_metadata(path).map_err(|_| Error::Storage)?,
        directory,
    )
}
fn check_metadata(meta: &fs::Metadata, directory: bool) -> Result<()> {
    if (directory && !meta.is_dir())
        || (!directory && !meta.is_file())
        || meta.file_type().is_symlink()
    {
        return Err(Error::UnsafeStorage);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions and does not mutate process state.
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            return Err(Error::UnsafeStorage);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Reject all reparse points, including junctions (not only symbolic links).
        if meta.file_attributes() & 0x400 != 0 {
            return Err(Error::UnsafeStorage);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample_config() -> AccessConfig {
        AccessConfig {
            bind: "127.0.0.1:4789".parse().unwrap(),
            advertised_ip: "127.0.0.1".parse().unwrap(),
            name: "Test".into(),
            allow_control: false,
        }
    }
    #[test]
    fn identity_persists_rotates_and_revokes() {
        let temp = tempfile::tempdir().unwrap();
        let store = AccessStore::new(temp.path().join("access"));
        assert!(store.load().unwrap().is_none());
        let first = store.enable(sample_config()).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(first.token, loaded.token);
        assert_eq!(first.private_key_pkcs8, loaded.private_key_pkcs8);
        let second = store.enable(sample_config()).unwrap();
        assert_ne!(first.token, second.token);
        assert_ne!(first.certificate_der, second.certificate_der);
        store.revoke().unwrap();
        assert!(store.load().unwrap().is_none());
        store.revoke().unwrap();
    }
    #[test]
    fn config_rejects_unreachable_identity_and_ephemeral_port() {
        let mut config = sample_config();
        config.bind.set_port(0);
        assert!(config.validate().is_err());
        config = sample_config();
        config.advertised_ip = "0.0.0.0".parse().unwrap();
        assert!(config.validate().is_err());
        config = sample_config();
        config.name = "x\ny".into();
        assert!(config.validate().is_err());
    }
    #[cfg(unix)]
    #[test]
    fn insecure_and_linked_store_is_rejected_without_reading_target() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("access");
        let store = AccessStore::new(dir.clone());
        store.enable(sample_config()).unwrap();
        assert_eq!(
            fs::metadata(dir.join(FILE_NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::set_permissions(dir.join(FILE_NAME), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(store.load(), Err(Error::UnsafeStorage)));
        fs::remove_file(dir.join(FILE_NAME)).unwrap();
        symlink(temp.path().join("missing"), dir.join(FILE_NAME)).unwrap();
        assert!(matches!(store.load(), Err(Error::UnsafeStorage)));
        store.revoke().unwrap();
        assert!(dir.join(FILE_NAME).symlink_metadata().is_err());
    }
    #[test]
    fn bounded_corrupt_storage_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("access");
        let store = AccessStore::new(dir.clone());
        store.enable(sample_config()).unwrap();
        fs::write(dir.join(FILE_NAME), vec![0; MAX_STORED as usize + 1]).unwrap();
        assert!(matches!(store.load(), Err(Error::UnsafeStorage)));
    }
}
