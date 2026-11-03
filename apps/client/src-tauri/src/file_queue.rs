//! A local, bounded selection queue. Only OS picker paths enter this module.
use assi_session::{TransferSnapshot, TransferState};
use serde::Serialize;
use std::path::PathBuf;
#[derive(Clone, Serialize)]
pub struct QueueEntry {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub state: &'static str,
    pub kind: &'static str,
    pub error: Option<String>,
    #[serde(skip)]
    path: PathBuf,
    #[serde(skip)]
    transfer: Option<String>,
}
#[derive(Default)]
pub struct FileQueue {
    request: String,
    #[cfg(any(not(target_os = "android"), test))]
    serial: u64,
    entries: Vec<QueueEntry>,
    bundles: std::collections::HashMap<u64, assi_folders::PreparedFolder>,
    retired: Vec<assi_folders::PreparedFolder>,
    retired_request: String,
}
impl FileQueue {
    pub fn owns_temporary_files(&self) -> bool {
        !self.bundles.is_empty() || !self.retired.is_empty()
    }
    pub fn clear(&mut self) {
        if !self.bundles.is_empty() {
            self.retired_request = self.request.clone();
        }
        self.request.clear();
        self.entries.clear();
        self.retired
            .extend(self.bundles.drain().map(|(_, bundle)| bundle));
    }
    #[cfg(any(not(target_os = "android"), test))]
    pub fn enqueue(&mut self, request: &str, paths: Vec<PathBuf>) -> Result<usize, String> {
        if request.is_empty() || paths.is_empty() || paths.len() > 32 {
            return Err("Выберите от 1 до 32 файлов".into());
        }
        if self.request != request {
            self.clear();
        }
        if self.entries.len() + paths.len() > 32 {
            return Err("В очереди не больше 32 файлов; удалите завершённые".into());
        }
        let mut additions = Vec::new();
        for path in paths {
            let info = std::fs::symlink_metadata(&path)
                .map_err(|_| "Не удалось прочитать выбранный файл")?;
            if !info.is_file()
                || info.file_type().is_symlink()
                || info.len() > assi_session::MAX_FILE_SIZE
            {
                return Err(
                    "Нужны обычные файлы до 1 ГиБ; для папки используйте отдельную команду отправки".into(),
                );
            }
            let name = path
                .file_name()
                .and_then(|v| v.to_str())
                .ok_or("Некорректное имя файла")?
                .to_owned();
            if name.chars().any(char::is_control) {
                return Err("Имя файла содержит управляющие символы".into());
            }
            additions.push((path, name, info.len()));
        }
        let count = additions.len();
        self.request = request.to_owned();
        for (path, name, size) in additions {
            self.serial = self.serial.checked_add(1).ok_or("Слишком много файлов")?;
            self.entries.push(QueueEntry {
                id: self.serial,
                name,
                size,
                state: "queued",
                kind: "file",
                error: None,
                path,
                transfer: None,
            });
        }
        Ok(count)
    }
    /// Called after the core has closed all file handles during stop/reset.
    pub fn finish_cleanup(&mut self) {
        self.retired.clear();
        self.retired_request.clear();
    }
    pub fn cleanup_previous(&mut self, request: &str) {
        if self.retired_request != request {
            self.finish_cleanup();
        }
    }
    #[cfg(any(not(target_os = "android"), test))]
    pub fn enqueue_folder(
        &mut self,
        request: &str,
        bundle: assi_folders::PreparedFolder,
    ) -> Result<usize, String> {
        let name = bundle.name.clone();
        self.enqueue(request, vec![bundle.path().to_owned()])?;
        let entry = self.entries.last_mut().ok_or("Ошибка очереди")?;
        entry.kind = "folder";
        entry.name = name;
        self.bundles.insert(entry.id, bundle);
        Ok(1)
    }
    pub fn snapshot(&self, request: &str) -> Vec<QueueEntry> {
        if self.request == request {
            self.entries.clone()
        } else {
            Vec::new()
        }
    }
    pub fn next(
        &mut self,
        request: &str,
        transfer: Option<&TransferSnapshot>,
    ) -> Option<(u64, PathBuf)> {
        if self.request != request {
            self.clear();
            return None;
        }
        if let Some(transfer) = transfer {
            // The peer may start another transfer before our next poll. A missing
            // terminal snapshot must not leave an unremovable active queue row.
            for entry in self.entries.iter_mut().filter(|e| {
                matches!(e.state, "active" | "paused")
                    && e.transfer.as_deref() != Some(&transfer.id)
            }) {
                entry.state = "error";
                entry.error=Some("Результат передачи больше не доступен. Проверьте файл у получателя и удалите строку из очереди".into());
            }
            if let Some(entry) = self
                .entries
                .iter_mut()
                .find(|e| e.transfer.as_deref() == Some(&transfer.id))
            {
                match transfer.state {
                    TransferState::Paused => entry.state = "paused",
                    TransferState::Transferring => entry.state = "active",
                    TransferState::Completed => entry.state = "completed",
                    TransferState::Rejected | TransferState::Cancelled => entry.state = "cancelled",
                    TransferState::Error => {
                        entry.state = "error";
                        entry.error = Some(
                            "Передача не завершена; удалите файл из очереди для продолжения".into(),
                        );
                    }
                    _ => {}
                }
            }
            let finished: Vec<_> = self
                .entries
                .iter()
                .filter(|entry| matches!(entry.state, "completed" | "cancelled" | "error"))
                .map(|entry| entry.id)
                .collect();
            for id in finished {
                self.bundles.remove(&id);
            }
            if matches!(
                transfer.state,
                TransferState::Offered | TransferState::Transferring | TransferState::Paused
            ) {
                return None;
            }
        }
        if self
            .entries
            .iter()
            .any(|e| matches!(e.state, "starting" | "active" | "paused" | "error"))
        {
            return None;
        }
        let entry = self.entries.iter_mut().find(|e| e.state == "queued")?;
        entry.state = "starting";
        Some((entry.id, entry.path.clone()))
    }
    pub fn started(&mut self, id: u64, transfer: String) {
        if let Some(e) = self
            .entries
            .iter_mut()
            .find(|e| e.id == id && e.state == "starting")
        {
            e.state = "active";
            e.transfer = Some(transfer);
        }
    }
    pub fn failed(&mut self, id: u64) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
            e.state = "error";
            e.error =
                Some("Не удалось предложить файл; удалите его из очереди для продолжения".into());
        }
    }
    pub fn remove(&mut self, request: &str, id: u64) -> Result<(), String> {
        if request != self.request {
            return Err("Сеанс изменился".into());
        }
        if self
            .entries
            .iter()
            .any(|e| e.id == id && matches!(e.state, "active" | "paused" | "starting"))
        {
            return Err("Сначала отмените текущую передачу".into());
        }
        self.entries.retain(|e| e.id != id);
        self.bundles.remove(&id);
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn batch_atomic_and_hidden_paths() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "a").unwrap();
        let mut q = FileQueue::default();
        assert!(q
            .enqueue("r", vec![file.clone(), dir.path().into()])
            .is_err());
        assert!(q.snapshot("r").is_empty());
        assert_eq!(q.enqueue("r", vec![file.clone()]).unwrap(), 1);
        let json = serde_json::to_string(&q.snapshot("r")).unwrap();
        assert!(!json.contains(&dir.path().display().to_string()));
        assert!(q.next("r", None).is_some());
        assert!(q.next("r", None).is_none());
        q.failed(1);
        assert!(q.next("r", None).is_none());
        q.remove("r", 1).unwrap();
        assert!(q.snapshot("r").is_empty());
    }
    #[test]
    fn limit_and_session_isolation() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a");
        std::fs::write(&file, "a").unwrap();
        let mut q = FileQueue::default();
        assert!(q.enqueue("r", vec![file.clone(); 33]).is_err());
        q.enqueue("r", vec![file; 32]).unwrap();
        assert!(q.next("new", None).is_none());
        assert!(q.snapshot("r").is_empty());
    }
    #[test]
    fn unrelated_transfer_cannot_complete_queue() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a");
        std::fs::write(&file, "a").unwrap();
        let mut q = FileQueue::default();
        q.enqueue("r", vec![file.clone(), file]).unwrap();
        let (id, _) = q.next("r", None).unwrap();
        q.started(id, "ours".into());
        let mut transfer = TransferSnapshot {
            id: "other".into(),
            name: "other".into(),
            size: 1,
            transferred: 1,
            direction: assi_session::TransferDirection::Receiving,
            state: TransferState::Completed,
            error: None,
            pause_ready: false,
            resume_requested: false,
        };
        assert!(q.next("r", Some(&transfer)).is_none());
        assert_eq!(q.snapshot("r")[0].state, "error");
        transfer.id = "ours".into();
        assert!(q.next("r", Some(&transfer)).is_some());
    }
    #[test]
    fn pause_keeps_queue_busy_and_preserves_folder_bundle() {
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("a"), "data").unwrap();
        let bundle = assi_folders::pack(source.path()).unwrap();
        let path = bundle.path().to_owned();
        let second = source.path().join("b");
        std::fs::write(&second, "next").unwrap();
        let mut queue = FileQueue::default();
        queue.enqueue_folder("r", bundle).unwrap();
        queue.enqueue("r", vec![second]).unwrap();
        let (id, _) = queue.next("r", None).unwrap();
        queue.started(id, "transfer".into());
        let mut transfer = TransferSnapshot {
            id: "transfer".into(),
            name: "folder".into(),
            size: 1,
            transferred: 0,
            direction: assi_session::TransferDirection::Sending,
            state: TransferState::Paused,
            error: None,
            pause_ready: true,
            resume_requested: false,
        };
        assert!(queue.next("r", Some(&transfer)).is_none());
        assert!(path.exists());
        assert_eq!(queue.snapshot("r")[0].state, "paused");
        assert!(queue.remove("r", id).is_err());
        transfer.state = TransferState::Transferring;
        assert!(queue.next("r", Some(&transfer)).is_none());
        assert!(path.exists());
        transfer.state = TransferState::Completed;
        assert!(queue.next("r", Some(&transfer)).is_some());
        assert!(!path.exists());
    }
    #[test]
    fn clear_retains_inflight_bundle_until_core_cleanup_barrier() {
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("a"), "data").unwrap();
        let bundle = assi_folders::pack(source.path()).unwrap();
        let path = bundle.path().to_owned();
        let mut queue = FileQueue::default();
        queue.enqueue_folder("r", bundle).unwrap();
        queue.clear();
        assert!(path.exists());
        queue.cleanup_previous("r");
        assert!(path.exists());
        queue.finish_cleanup();
        assert!(!path.exists());
    }
}
