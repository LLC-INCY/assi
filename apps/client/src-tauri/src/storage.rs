use assi_core::{Store, StoreError};
use std::{path::PathBuf, sync::Mutex};

pub struct Storage {
    path: Result<PathBuf, String>,
    store: Mutex<Option<Store>>,
}
impl Storage {
    pub fn new(path: Result<PathBuf, String>) -> Self {
        Self {
            path,
            store: Mutex::new(None),
        }
    }
    pub fn access<T>(
        &self,
        default_name: &str,
        f: impl FnOnce(&mut Store) -> Result<T, StoreError>,
    ) -> Result<T, String> {
        let mut guard = self
            .store
            .lock()
            .map_err(|_| "Хранилище недоступно. Перезапустите приложение.".to_string())?;
        if guard.is_none() {
            let path = self.path.as_ref().map_err(Clone::clone)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|_| "Не удалось открыть папку данных. Проверьте доступ к ней и повторите попытку.".to_string())?;
            }
            // Keep None on failure: the UI can display the error and retry without destroying data.
            *guard = Some(Store::open(path, default_name).map_err(super::client_error)?);
        }
        let store = guard
            .as_mut()
            .ok_or_else(|| "Хранилище недоступно.".to_string())?;
        f(store).map_err(super::client_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn newer_database_reports_error_and_can_retry_after_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("future.sqlite3");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.pragma_update(None, "user_version", 99).unwrap();
        let storage = Storage::new(Ok(path.clone()));
        assert!(storage
            .access("Тест", |s| s.snapshot())
            .unwrap_err()
            .contains("новой версией"));
        assert_eq!(
            db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            99
        );
        // Simulate a repaired/replaced database; the same backend must be retryable.
        db.pragma_update(None, "user_version", 0).unwrap();
        drop(db);
        assert!(storage.access("Тест", |s| s.snapshot()).is_ok());
    }
    #[test]
    fn corrupt_database_is_preserved_and_error_is_user_visible() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.sqlite3");
        std::fs::write(&path, b"this is not a SQLite database").unwrap();
        let storage = Storage::new(Ok(path.clone()));
        let error = storage.access("Тест", |s| s.snapshot()).unwrap_err();
        assert!(error.contains("данные"));
        assert!(!error.contains(&path.to_string_lossy().to_string()));
        assert_eq!(
            std::fs::read(path).unwrap(),
            b"this is not a SQLite database"
        );
    }
}
