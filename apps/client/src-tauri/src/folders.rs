//! Folder transfer is an explicitly selected mode. An extension never authorizes
//! extraction. Local picker paths and staging paths remain outside webview data.
use super::*;
#[cfg(any(not(target_os = "android"), test))]
use assi_session::TransferDirection;
use assi_session::TransferState;
use std::path::PathBuf;

#[derive(Clone, Serialize)]
pub struct FolderStatus {
    request_id: String,
    state: &'static str,
    transfer_id: Option<String>,
    name: Option<String>,
    error: Option<String>,
}
impl FolderStatus {
    fn idle(request: &str) -> Self {
        Self {
            request_id: request.into(),
            state: "idle",
            transfer_id: None,
            name: None,
            error: None,
        }
    }
}
struct ReceivingFolder {
    request: String,
    transfer: String,
    staging: Arc<tempfile::TempDir>,
    destination: PathBuf,
    cancel: assi_folders::CancellationToken,
    task: Option<
        tokio::task::JoinHandle<
            Result<assi_folders::PreparedExtraction, assi_folders::FolderError>,
        >,
    >,
}
impl ReceivingFolder {
    fn payload(&self) -> PathBuf {
        self.staging.path().join("payload.assi-folder")
    }
}
#[derive(Default)]
pub(super) struct FolderJobs {
    preparing: Option<(String, assi_folders::CancellationToken)>,
    receiving: Option<ReceivingFolder>,
    status: Option<FolderStatus>,
}
impl FolderJobs {
    pub(super) fn cancel(&mut self) {
        if let Some((_, token)) = &self.preparing {
            token.cancel();
        }
        if let Some(job) = &self.receiving {
            job.cancel.cancel();
        }
        if let Some(status) = self.status.as_mut() {
            if matches!(status.state, "receiving" | "extracting") {
                status.state = "cancelled";
            }
        }
    }
    /// Call only after the core closes its file handles. A cancelled blocking
    /// worker retains its own staging guard until it returns and drops output.
    pub(super) fn clear_after_core(&mut self) {
        self.cancel();
        self.preparing = None;
        self.receiving = None;
        self.status = None;
    }
    pub(super) fn clear_except(&mut self, request: Option<&str>) {
        if self
            .preparing
            .as_ref()
            .is_some_and(|(id, _)| Some(id.as_str()) != request)
        {
            if let Some((_, token)) = self.preparing.take() {
                token.cancel();
            }
        }
        if self
            .receiving
            .as_ref()
            .is_some_and(|job| Some(job.request.as_str()) != request)
        {
            if let Some(job) = self.receiving.take() {
                job.cancel.cancel();
            }
        }
        if self
            .status
            .as_ref()
            .is_some_and(|status| Some(status.request_id.as_str()) != request)
        {
            self.status = None;
        }
    }
    fn snapshot(&self, request: &str) -> FolderStatus {
        self.status
            .as_ref()
            .filter(|s| s.request_id == request)
            .cloned()
            .unwrap_or_else(|| FolderStatus::idle(request))
    }
    #[cfg(not(target_os = "android"))]
    fn busy(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|s| matches!(s.state, "receiving" | "extracting"))
    }
    fn cancel_extraction(&mut self, request: &str, id: &str) -> bool {
        let matched = self
            .receiving
            .as_ref()
            .is_some_and(|job| job.request == request && job.transfer == id && job.task.is_some());
        if matched {
            if let Some(job) = self.receiving.take() {
                job.cancel.cancel();
            }
            if let Some(status) = self.status.as_mut() {
                status.state = "cancelled";
            }
        }
        matched
    }
}
#[tauri::command]
pub async fn session_file_offer_folder(
    app: tauri::AppHandle,
    remote: crate::sessions::RemoteSession,
    request_id: String,
) -> Result<bool, String> {
    #[cfg(target_os = "android")]
    {
        let _ = (app, remote, request_id);
        Err("Выбор папок Android ещё не подключён".into())
    }
    #[cfg(not(target_os = "android"))]
    {
        use tauri_plugin_dialog::DialogExt;
        let _dialog = remote
            .file_dialog
            .try_lock()
            .map_err(|_| "Окно выбора файла уже открыто")?;
        require_file_session(&remote, &request_id)?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let owner = app
            .get_webview_window(remote.label())
            .ok_or("Окно сеанса закрыто")?;
        app.dialog()
            .file()
            .set_parent(&owner)
            .set_title("Выберите папку для отправки")
            .pick_folder(move |path| {
                let _ = tx.send(path);
            });
        let Some(path) = rx.await.map_err(|_| "Выбор папки прерван")? else {
            return Ok(false);
        };
        require_file_session(&remote, &request_id)?;
        let source = path
            .into_path()
            .map_err(|_| "Это место пока не поддерживается")?;
        let cancel = assi_folders::CancellationToken::default();
        {
            let _guard = remote.lifecycle.lock().await;
            require_file_session(&remote, &request_id)?;
            let mut jobs = remote
                .folder_jobs
                .lock()
                .map_err(|_| "Ошибка передачи папки")?;
            jobs.preparing = Some((request_id.clone(), cancel.clone()));
        }
        let worker_cancel = cancel.clone();
        let prepared = tauri::async_runtime::spawn_blocking(move || {
            assi_folders::pack_with_cancel(&source, &worker_cancel)
        })
        .await;
        {
            let mut jobs = remote
                .folder_jobs
                .lock()
                .map_err(|_| "Ошибка передачи папки")?;
            jobs.preparing = None;
        }
        let bundle = prepared
            .map_err(|_| "Подготовка папки прервана")?
            .map_err(|e| e.to_string())?;
        let _guard = remote.lifecycle.lock().await;
        require_file_session(&remote, &request_id)?;
        if cancel.is_cancelled() {
            return Err("Подготовка папки отменена".into());
        }
        remote
            .file_queue
            .lock()
            .map_err(|_| "Ошибка очереди")?
            .enqueue_folder(&request_id, bundle)?;
        Ok(true)
    }
}
#[tauri::command]
pub async fn session_file_accept_folder(
    app: tauri::AppHandle,
    remote: crate::sessions::RemoteSession,
    request_id: String,
    transfer_id: String,
) -> Result<bool, String> {
    #[cfg(target_os = "android")]
    {
        let _ = (app, remote, request_id, transfer_id);
        Err("Сохранение папок Android ещё не подключено".into())
    }
    #[cfg(not(target_os = "android"))]
    {
        use tauri_plugin_dialog::DialogExt;
        let _dialog = remote
            .file_dialog
            .try_lock()
            .map_err(|_| "Окно выбора файла уже открыто")?;
        let snapshot = require_file_session(&remote, &request_id)?;
        let incoming = |snapshot: &Snapshot| {
            snapshot
                .transfer
                .as_ref()
                .filter(|t| {
                    t.id == transfer_id
                        && t.direction == TransferDirection::Receiving
                        && t.state == TransferState::Offered
                })
                .cloned()
                .ok_or("Предложение папки уже не актуально")
        };
        incoming(&snapshot)?;
        if remote
            .folder_jobs
            .lock()
            .map_err(|_| "Ошибка передачи папки")?
            .busy()
        {
            return Err("Сначала завершите текущую передачу папки".into());
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let owner = app
            .get_webview_window(remote.label())
            .ok_or("Окно сеанса закрыто")?;
        app.dialog()
            .file()
            .set_parent(&owner)
            .set_title("Принять и распаковать папку — выберите место сохранения")
            .pick_folder(move |path| {
                let _ = tx.send(path);
            });
        let Some(path) = rx.await.map_err(|_| "Выбор места сохранения прерван")?
        else {
            return Ok(false);
        };
        let destination = path
            .into_path()
            .map_err(|_| "Это место пока не поддерживается")?;
        let staging = Arc::new(
            tempfile::Builder::new()
                .prefix("assi-folder-receive-")
                .tempdir()
                .map_err(|_| "Не удалось подготовить приём папки")?,
        );
        let _guard = remote.lifecycle.lock().await;
        let transfer = incoming(&require_file_session(&remote, &request_id)?)?;
        if remote
            .folder_jobs
            .lock()
            .map_err(|_| "Ошибка передачи папки")?
            .busy()
        {
            return Err("Сначала завершите текущую передачу папки".into());
        }
        let job = ReceivingFolder {
            request: request_id.clone(),
            transfer: transfer_id.clone(),
            staging,
            destination,
            cancel: assi_folders::CancellationToken::default(),
            task: None,
        };
        remote
            .engine
            .accept_file(request_id.clone(), transfer_id.clone(), job.payload())
            .await
            .map_err(|e| e.to_string())?;
        let mut jobs = remote
            .folder_jobs
            .lock()
            .map_err(|_| "Ошибка передачи папки")?;
        jobs.receiving = Some(job);
        jobs.status = Some(FolderStatus {
            request_id,
            state: "receiving",
            transfer_id: Some(transfer_id),
            name: Some(transfer.name),
            error: None,
        });
        Ok(true)
    }
}
#[tauri::command]
pub fn session_file_folder_status(
    remote: crate::sessions::RemoteSession,
    request_id: String,
) -> Result<FolderStatus, String> {
    let snapshot = remote.engine.snapshot();
    if snapshot.state != SessionState::Active || snapshot.request_id.as_deref() != Some(&request_id)
    {
        return Err("Сеанс изменился".into());
    }
    Ok(remote
        .folder_jobs
        .lock()
        .map_err(|_| "Ошибка передачи папки")?
        .snapshot(&request_id))
}
#[tauri::command]
pub async fn session_file_pause(
    remote: crate::sessions::RemoteSession,
    request_id: String,
    transfer_id: String,
) -> Result<(), String> {
    remote
        .engine
        .pause_file(request_id, transfer_id)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn session_file_resume(
    remote: crate::sessions::RemoteSession,
    request_id: String,
    transfer_id: String,
) -> Result<(), String> {
    remote
        .engine
        .resume_file(request_id, transfer_id)
        .await
        .map_err(|e| e.to_string())
}
impl Remote {
    /// Called with lifecycle held. Heavy disk work runs in a worker; only the
    /// no-clobber publication happens here after rechecking session and cancel.
    pub(super) async fn folder_tick(&self, snapshot: &Snapshot) {
        let finished = {
            let Ok(mut jobs) = self.folder_jobs.lock() else {
                return;
            };
            let Some(job) = jobs.receiving.as_mut() else {
                return;
            };
            if snapshot.state != SessionState::Active {
                jobs.cancel();
                return;
            }
            if snapshot.request_id.as_deref() != Some(&job.request) {
                // Every new active request starts after the core's old file
                // cleanup barrier, including persistent host reconnections.
                jobs.clear_after_core();
                return;
            }
            if job.cancel.is_cancelled() {
                return;
            }
            if job.task.is_none() {
                let transfer = snapshot.transfer.as_ref().filter(|t| t.id == job.transfer);
                if transfer.is_some_and(|t| {
                    matches!(
                        t.state,
                        TransferState::Cancelled | TransferState::Rejected | TransferState::Error
                    )
                }) {
                    job.cancel.cancel();
                    jobs.receiving = None;
                    if let Some(status) = jobs.status.as_mut() {
                        status.state = "cancelled";
                    }
                    return;
                }
                // This private final path is created only by FileManager's
                // SHA-256-verified no-clobber commit, never from an offer name.
                if !job.payload().is_file() {
                    if snapshot
                        .transfer
                        .as_ref()
                        .is_some_and(|transfer| transfer.id != job.transfer)
                    {
                        job.cancel.cancel();
                        jobs.receiving = None;
                        if let Some(status) = jobs.status.as_mut() {
                            status.state = "error";
                            status.error = Some("Передача папки не завершена".into());
                        }
                    }
                    return;
                }
                let staging = job.staging.clone();
                let parent = job.destination.clone();
                let cancel = job.cancel.clone();
                job.task = Some(tokio::task::spawn_blocking(move || {
                    assi_folders::prepare_unpack(
                        &staging.path().join("payload.assi-folder"),
                        &parent,
                        &cancel,
                    )
                }));
                if let Some(status) = jobs.status.as_mut() {
                    status.state = "extracting";
                }
            }
            jobs.receiving.as_mut().and_then(|job| {
                if job.task.as_ref().is_some_and(|task| task.is_finished()) {
                    job.task.take()
                } else {
                    None
                }
            })
        };
        let Some(task) = finished else {
            return;
        };
        let prepared = task
            .await
            .map_err(|_| "Распаковка папки прервана".to_owned())
            .and_then(|result| result.map_err(|e| e.to_string()));
        let Ok(mut jobs) = self.folder_jobs.lock() else {
            return;
        };
        let Some(job) = jobs.receiving.as_ref() else {
            return;
        };
        let now = self.engine.snapshot();
        if now.state != SessionState::Active
            || now.request_id.as_deref() != Some(&job.request)
            || job.cancel.is_cancelled()
        {
            jobs.cancel();
            return;
        }
        let result = prepared.and_then(|prepared| prepared.publish().map_err(|e| e.to_string()));
        if let Some(status) = jobs.status.as_mut() {
            match result {
                Ok(path) => {
                    status.state = "completed";
                    status.name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                }
                Err(error) => {
                    status.state = "error";
                    status.error = Some(error);
                }
            }
        }
        jobs.receiving = None;
    }
    pub(super) fn cancel_folder_extraction(&self, request: &str, id: &str) -> bool {
        self.folder_jobs
            .lock()
            .is_ok_and(|mut jobs| jobs.cancel_extraction(request, id))
    }
    pub(super) fn has_file_work(&self) -> bool {
        self.folder_jobs
            .lock()
            .is_ok_and(|jobs| jobs.preparing.is_some() || jobs.receiving.is_some())
            || self
                .file_queue
                .lock()
                .is_ok_and(|queue| queue.owns_temporary_files())
    }
    pub(super) fn cancel_folder_jobs(&self) {
        if let Ok(mut jobs) = self.folder_jobs.lock() {
            jobs.cancel();
        }
    }
    pub(super) fn finish_inactive_file_cleanup(&self) {
        let now = self.engine.snapshot();
        let active = (now.state == SessionState::Active)
            .then_some(now.request_id.as_deref())
            .flatten();
        if let Ok(mut jobs) = self.folder_jobs.lock() {
            jobs.clear_except(active);
        }
        if let Ok(mut queue) = self.file_queue.lock() {
            match active {
                Some(id) => queue.cleanup_previous(id),
                None => queue.finish_cleanup(),
            }
        }
    }
    pub(super) fn finish_file_cleanup(&self) {
        if let Ok(mut jobs) = self.folder_jobs.lock() {
            jobs.clear_after_core();
        }
        if let Ok(mut queue) = self.file_queue.lock() {
            queue.finish_cleanup();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn until(mut condition: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(4), async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    async fn pair() -> (Arc<SessionEngine>, Arc<Remote>, String) {
        let host = SessionEngine::new();
        let viewer = Remote::new(None);
        let ip = "127.0.0.1".parse().unwrap();
        let invitation = host
            .start_host(SocketAddr::new(ip, 0), ip, "Host".into())
            .await
            .unwrap();
        viewer
            .engine
            .connect(invitation, "Viewer".into())
            .await
            .unwrap();
        until(|| {
            host.snapshot().state == SessionState::Pending
                && viewer.engine.snapshot().state == SessionState::Pending
        })
        .await;
        let id = host.snapshot().request_id.unwrap();
        host.accept(id.clone(), false).await.unwrap();
        until(|| {
            host.snapshot().state == SessionState::Active
                && viewer.engine.snapshot().state == SessionState::Active
        })
        .await;
        (host, viewer, id)
    }
    fn stage(
        viewer: &Remote,
        request: &str,
        bundle: &std::path::Path,
        parent: &std::path::Path,
    ) -> (PathBuf, assi_folders::CancellationToken) {
        let staging = Arc::new(tempfile::tempdir().unwrap());
        let path = staging.path().to_owned();
        std::fs::copy(bundle, staging.path().join("payload.assi-folder")).unwrap();
        let cancel = assi_folders::CancellationToken::default();
        let mut jobs = viewer.folder_jobs.lock().unwrap();
        jobs.receiving = Some(ReceivingFolder {
            request: request.into(),
            transfer: "test-transfer".into(),
            staging,
            destination: parent.into(),
            cancel: cancel.clone(),
            task: None,
        });
        jobs.status = Some(FolderStatus {
            request_id: request.into(),
            state: "receiving",
            transfer_id: Some("test-transfer".into()),
            name: Some("folder".into()),
            error: None,
        });
        (path, cancel)
    }
    fn source() -> (tempfile::TempDir, assi_folders::PreparedFolder) {
        let source = tempfile::tempdir().unwrap();
        let directory = source.path().join("example");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("hello.txt"), "hello").unwrap();
        let bundle = assi_folders::pack(&directory).unwrap();
        (source, bundle)
    }
    #[tokio::test]
    async fn explicit_folder_mode_publishes_only_after_worker_validation() {
        let (host, viewer, id) = pair().await;
        let (_source, bundle) = source();
        let destination = tempfile::tempdir().unwrap();
        let (staging, _cancel) = stage(&viewer, &id, bundle.path(), destination.path());
        viewer.folder_tick(&viewer.engine.snapshot()).await;
        until(|| {
            viewer
                .folder_jobs
                .lock()
                .unwrap()
                .receiving
                .as_ref()
                .and_then(|job| job.task.as_ref())
                .is_some_and(|task| task.is_finished())
        })
        .await;
        assert!(!destination.path().join("example").exists());
        viewer.folder_tick(&viewer.engine.snapshot()).await;
        assert_eq!(
            std::fs::read_to_string(destination.path().join("example/hello.txt")).unwrap(),
            "hello"
        );
        assert!(!staging.exists());
        assert_eq!(
            viewer.folder_jobs.lock().unwrap().snapshot(&id).state,
            "completed"
        );
        viewer.terminate().await.unwrap();
        host.stop().await.unwrap();
    }
    #[tokio::test]
    async fn cancellation_discards_prepared_folder_before_publication() {
        let (host, viewer, id) = pair().await;
        let (_source, bundle) = source();
        let destination = tempfile::tempdir().unwrap();
        let (_staging, cancel) = stage(&viewer, &id, bundle.path(), destination.path());
        viewer.folder_tick(&viewer.engine.snapshot()).await;
        until(|| {
            viewer
                .folder_jobs
                .lock()
                .unwrap()
                .receiving
                .as_ref()
                .and_then(|job| job.task.as_ref())
                .is_some_and(|task| task.is_finished())
        })
        .await;
        assert!(viewer.cancel_folder_extraction(&id, "test-transfer"));
        assert!(cancel.is_cancelled());
        viewer.folder_tick(&viewer.engine.snapshot()).await;
        assert_eq!(std::fs::read_dir(destination.path()).unwrap().count(), 0);
        viewer.terminate().await.unwrap();
        host.stop().await.unwrap();
    }
    #[tokio::test]
    async fn ordinary_bundle_file_is_never_automatically_extracted() {
        let (host, viewer, id) = pair().await;
        let (_source, bundle) = source();
        let destination = tempfile::tempdir().unwrap();
        let transfer = host
            .offer_file(id.clone(), bundle.path().into())
            .await
            .unwrap();
        until(|| {
            viewer
                .engine
                .snapshot()
                .transfer
                .as_ref()
                .is_some_and(|value| value.id == transfer)
        })
        .await;
        let target = destination.path().join("ordinary.assi-folder");
        viewer
            .engine
            .accept_file(id, transfer, target.clone())
            .await
            .unwrap();
        until(|| {
            viewer
                .engine
                .snapshot()
                .transfer
                .as_ref()
                .is_some_and(|value| value.state == TransferState::Completed)
        })
        .await;
        viewer.folder_tick(&viewer.engine.snapshot()).await;
        assert!(target.is_file());
        assert_eq!(std::fs::read_dir(destination.path()).unwrap().count(), 1);
        viewer.terminate().await.unwrap();
        host.stop().await.unwrap();
    }
    #[tokio::test]
    async fn passive_viewer_disconnect_cancels_work_and_removes_staging() {
        let (host, viewer, id) = pair().await;
        let destination = tempfile::tempdir().unwrap();
        let staging = Arc::new(tempfile::tempdir().unwrap());
        let path = staging.path().to_owned();
        let cancel = assi_folders::CancellationToken::default();
        {
            let mut jobs = viewer.folder_jobs.lock().unwrap();
            jobs.receiving = Some(ReceivingFolder {
                request: id.clone(),
                transfer: "pending".into(),
                staging,
                destination: destination.path().into(),
                cancel: cancel.clone(),
                task: None,
            });
            jobs.status = Some(FolderStatus {
                request_id: id,
                state: "receiving",
                transfer_id: Some("pending".into()),
                name: Some("folder".into()),
                error: None,
            });
        }
        let pump = tokio::spawn(viewer.clone().pump());
        tokio::time::sleep(Duration::from_millis(60)).await;
        host.stop().await.unwrap();
        until(|| cancel.is_cancelled() && !path.exists()).await;
        viewer.terminate().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), pump)
            .await
            .unwrap()
            .unwrap();
    }
}
