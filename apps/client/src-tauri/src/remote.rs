//! IPC boundary: only local UI may grant consent; native media stays dormant until then.
#[path = "folders.rs"]
mod folders;
#[path = "remote_media.rs"]
mod media;
#[path = "remote_tools.rs"]
mod tools;
use assi_platform::Capabilities;
#[cfg(not(target_os = "android"))]
use assi_platform::Platform;
use assi_session::{InputEvent, Role, SessionEngine, SessionState, Snapshot};
pub use folders::*;
pub use media::*;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::{
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{Manager, State};
pub use tools::*;

pub struct Remote {
    pub(crate) engine: Arc<SessionEngine>,
    pub(crate) window_label: String,
    pub(crate) closed: AtomicBool,
    #[cfg(not(target_os = "android"))]
    platform: Arc<Mutex<Platform>>,
    app: Option<tauri::AppHandle>,
    overlay_enabled: AtomicBool,
    fullscreen_restore: Mutex<Option<bool>>,
    audio: Mutex<assi_audio::AudioCapture>,
    terminal: Mutex<assi_terminal::TerminalManager>,
    file_queue: Mutex<crate::file_queue::FileQueue>,
    folder_jobs: Mutex<folders::FolderJobs>,
    #[cfg(not(target_os = "android"))]
    preview: Mutex<Option<media::Preview>>,
    #[cfg(not(target_os = "android"))]
    encoder: Arc<Mutex<assi_video::VideoEncoder>>,
    audio_error: Mutex<Option<String>>,
    capture_fps: AtomicU64,
    overlay_display_supported: AtomicBool,
    #[cfg(not(target_os = "android"))]
    file_dialog: tokio::sync::Mutex<()>,
    error: Arc<Mutex<Option<String>>>,
    pub(crate) lifecycle: tokio::sync::Mutex<()>,
}
#[derive(Serialize)]
pub struct RemoteSnapshot {
    session: Snapshot,
    capabilities: Capabilities,
    media_error: Option<String>,
    files_supported: bool,
    overlay_supported: bool,
    overlay_enabled: bool,
    audio_supported: bool,
    terminal_supported: bool,
    audio_error: Option<String>,
}
impl Remote {
    pub fn new(app: Option<tauri::AppHandle>) -> Arc<Self> {
        Self::new_for_window(app, "main".into())
    }
    pub fn new_for_window(_app: Option<tauri::AppHandle>, window_label: String) -> Arc<Self> {
        Arc::new(Self {
            engine: SessionEngine::new(),
            window_label,
            closed: AtomicBool::new(false),
            #[cfg(not(target_os = "android"))]
            platform: Arc::new(Mutex::new(Platform::new())),
            app: _app,
            overlay_enabled: AtomicBool::new(true),
            fullscreen_restore: Mutex::new(None),
            audio: Mutex::new(assi_audio::AudioCapture::new()),
            terminal: Mutex::new(assi_terminal::TerminalManager::default()),
            file_queue: Mutex::new(crate::file_queue::FileQueue::default()),
            folder_jobs: Mutex::new(folders::FolderJobs::default()),
            #[cfg(not(target_os = "android"))]
            preview: Mutex::new(None),
            #[cfg(not(target_os = "android"))]
            encoder: Arc::new(Mutex::new(assi_video::VideoEncoder::new())),
            audio_error: Mutex::new(None),
            capture_fps: AtomicU64::new(8),
            overlay_display_supported: AtomicBool::new(true),
            #[cfg(not(target_os = "android"))]
            file_dialog: tokio::sync::Mutex::new(()),
            error: Arc::new(Mutex::new(None)),
            lifecycle: tokio::sync::Mutex::new(()),
        })
    }
    pub(crate) fn is_main(&self) -> bool {
        self.window_label == "main"
    }
    pub async fn terminate(&self) -> Result<(), String> {
        self.closed.store(true, Ordering::SeqCst);
        self.shutdown().await
    }
    pub async fn pump(self: Arc<Self>) {
        let mut previous = None;
        let mut previous_session = None;
        let mut cleanup_needed = false;
        let video = self.clone();
        tauri::async_runtime::spawn(video.pump_video());
        while !self.closed.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if self.closed.load(Ordering::SeqCst) {
                break;
            }
            // Serialize native work with start/stop: no old-session input can cross a restart.
            let _guard = self.lifecycle.lock().await;
            let snapshot = self.engine.snapshot();
            if snapshot.state == SessionState::Active {
                previous_session = snapshot.request_id.clone();
            } else if previous_session.take().is_some() || self.has_file_work() {
                self.cancel_folder_jobs();
                if let Ok(mut queue) = self.file_queue.lock() {
                    queue.clear();
                }
                cleanup_needed = true;
            }
            if cleanup_needed && self.engine.wait_file_cleanup().await.is_ok() {
                self.finish_inactive_file_cleanup();
                cleanup_needed = false;
            }
            if snapshot.state == SessionState::Active {
                self.folder_tick(&snapshot).await;
                self.file_queue_tick(&snapshot).await;
            }
            let active = snapshot.role == Role::Host && snapshot.state == SessionState::Active;
            if !active {
                self.stop_audio();
                if let Some(app) = self.app.as_ref().filter(|_| self.is_main()) {
                    let _ = crate::overlay::hide(app);
                }
                if snapshot.state != SessionState::Active {
                    self.cancel_folder_jobs();
                    if let Ok(mut queue) = self.file_queue.lock() {
                        queue.clear();
                    }
                    let _ = self.restore_fullscreen();
                }
                if previous.take().is_some() {
                    let _ = self.release().await;
                }
                continue;
            }
            if previous.is_some() && previous != snapshot.request_id {
                let _ = self.release().await;
            }
            previous = snapshot.request_id.clone();
            self.audio_tick(&snapshot).await;
            if let Some(id) = snapshot.request_id.as_deref() {
                let _ = crate::access::publish_system_capabilities(&self.engine, id);
            }
            let outcome = self.input_tick(snapshot.request_id.clone()).await;
            self.terminal_tick(&snapshot).await;
            if outcome.is_ok() {
                if let Some(app) = self.app.as_ref().filter(|_| self.is_main()) {
                    let now = self.engine.snapshot();
                    let visible = now.state == SessionState::Active
                        && now.role == Role::Host
                        && now.request_id == snapshot.request_id
                        && self.overlay_enabled.load(Ordering::SeqCst)
                        && self.overlay_display_supported.load(Ordering::SeqCst)
                        && !self.engine.annotations().strokes.is_empty();
                    if let Err(error) = crate::overlay::sync(app, visible) {
                        self.overlay_enabled.store(false, Ordering::SeqCst);
                        *self.error.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(format!("Пометки поверх окон недоступны: {error}"));
                    }
                }
            }
            if let Err(error) = outcome {
                *self.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
                let (stop_result, _) = tokio::join!(self.engine.stop(), self.release());
                if let Err(error) = stop_result {
                    *self.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(error.to_string());
                }
                previous = None;
            }
        }
    }
    fn restore_fullscreen(&self) -> Result<(), String> {
        if let Some(was) = self
            .fullscreen_restore
            .lock()
            .map_err(|_| "Ошибка окна")?
            .take()
        {
            if let Some(window) = self
                .app
                .as_ref()
                .and_then(|app| app.get_webview_window(&self.window_label))
            {
                #[cfg(not(target_os = "android"))]
                window.set_fullscreen(was).map_err(|e| e.to_string())?;
                #[cfg(target_os = "android")]
                let _ = (window, was);
            }
        }
        Ok(())
    }
    pub(crate) async fn release_held_inputs(&self) -> Result<(), String> {
        self.engine.take_input();
        #[cfg(not(target_os = "android"))]
        {
            let platform = self.platform.clone();
            tauri::async_runtime::spawn_blocking(move || {
                platform
                    .lock()
                    .map_err(|_| "Ошибка ввода".to_string())?
                    .release_inputs()
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|_| "Завершение ввода прервано".to_string())?
        }
        #[cfg(target_os = "android")]
        {
            Ok(())
        }
    }
    pub(crate) fn recording_snapshot(&self) -> Snapshot {
        self.engine.snapshot()
    }
    pub(crate) fn report_error(&self, error: String) {
        *self.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
    }
    pub async fn shutdown(&self) -> Result<(), String> {
        self.cancel_folder_jobs();
        self.stop_audio();
        if let Ok(mut queue) = self.file_queue.lock() {
            queue.clear();
        }
        if let Ok(mut terminal) = self.terminal.lock() {
            terminal.stop();
        }
        #[cfg(not(target_os = "android"))]
        if let Ok(mut preview) = self.preview.lock() {
            *preview = None;
        }
        if let Some(app) = self.app.as_ref().filter(|_| self.is_main()) {
            let _ = crate::overlay::hide(app);
        }
        let _ = self.restore_fullscreen();
        // Revoke immediately, even while an OS capture is running. Native cleanup
        // then waits for that work; checks in the pump prevent subsequent dispatch.
        let early_stop = self.engine.stop();
        let cleanup = async {
            let _guard = self.lifecycle.lock().await;
            // Disk cleanup may be slow; release native keys/projection in parallel
            // with network/file shutdown, after waiting for in-flight native work.
            let (stop, release) = tokio::join!(self.engine.stop(), self.release());
            stop.map_err(|e| e.to_string())?;
            release
        };
        let (stopped, cleaned) = tokio::join!(early_stop, cleanup);
        self.finish_file_cleanup();
        stopped.map_err(|e| e.to_string())?;
        cleaned?;
        // Stop is immediate; let the PTY worker kill and reap its process group before app exit.
        for _ in 0..200 {
            let stopped = self.terminal.lock().map(|t| t.is_stopped()).unwrap_or(true);
            if stopped {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Err("Терминал ещё завершает процесс".into())
    }

    #[cfg(not(target_os = "android"))]
    async fn release(&self) -> Result<(), String> {
        self.cancel_folder_jobs();
        self.stop_audio();
        if let Ok(mut queue) = self.file_queue.lock() {
            queue.clear();
        }
        if let Ok(mut terminal) = self.terminal.lock() {
            terminal.stop();
        }
        #[cfg(not(target_os = "android"))]
        if let Ok(mut preview) = self.preview.lock() {
            *preview = None;
        }
        if let Some(app) = self.app.as_ref().filter(|_| self.is_main()) {
            let _ = crate::overlay::hide(app);
        }
        let platform = self.platform.clone();
        tauri::async_runtime::spawn_blocking(move || {
            platform
                .lock()
                .map_err(|_| "Ошибка состояния ввода".to_string())?
                .release_inputs()
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|_| "Не удалось завершить системный ввод".to_string())?
    }
    #[cfg(target_os = "android")]
    async fn release(&self) -> Result<(), String> {
        self.cancel_folder_jobs();
        self.stop_audio();
        if let Ok(mut queue) = self.file_queue.lock() {
            queue.clear();
        }
        if let Ok(mut terminal) = self.terminal.lock() {
            terminal.stop();
        }
        #[cfg(not(target_os = "android"))]
        if let Ok(mut preview) = self.preview.lock() {
            *preview = None;
        }
        if let Some(app) = self.app.as_ref().filter(|_| self.is_main()) {
            assi_capture::stop(app).await?;
        }
        Ok(())
    }
    fn stop_audio(&self) {
        if let Ok(mut audio) = self.audio.lock() {
            audio.stop();
        }
    }
    async fn audio_tick(&self, snapshot: &Snapshot) {
        let Some(id) = snapshot.request_id.clone() else {
            return;
        };
        if !snapshot.audio_enabled {
            self.stop_audio();
            return;
        }
        let (frames, error) = match self.audio.lock() {
            Ok(mut audio) => (audio.drain(), audio.take_error()),
            Err(_) => (Vec::new(), Some("Ошибка состояния звука".into())),
        };
        if let Some(error) = error {
            *self.audio_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
            let _ = self.engine.set_audio_enabled(id, false).await;
            self.stop_audio();
            return;
        }
        for pcm in frames {
            if self
                .engine
                .publish_audio(
                    id.clone(),
                    assi_session::AudioPacket {
                        sample_rate: 48_000,
                        channels: 2,
                        pcm,
                    },
                )
                .is_err()
            {
                break;
            }
        }
    }
    pub(crate) async fn reset(&self) -> Result<(), String> {
        if self.closed.load(Ordering::SeqCst) {
            return Err("Окно сеанса уже закрыто".into());
        }
        self.overlay_enabled.store(true, Ordering::SeqCst);
        let _ = self.restore_fullscreen();
        let (stop_result, release_result) = tokio::join!(self.engine.stop(), self.release());
        self.finish_file_cleanup();
        stop_result.map_err(|e| e.to_string())?;
        release_result?;
        #[cfg(not(target_os = "android"))]
        {
            *self
                .platform
                .lock()
                .map_err(|_| "Ошибка системного адаптера")? = Platform::new();
        }
        self.capture_fps.store(8, Ordering::Relaxed);
        self.overlay_display_supported.store(true, Ordering::SeqCst);
        *self.audio_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.error.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(())
    }
}
fn capture_capabilities() -> Capabilities {
    #[cfg(target_os = "android")]
    {
        Capabilities {
            capture: true,
            control: false,
            reason: Some("Android поддерживает показ экрана без удалённого управления".into()),
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        Platform::capabilities()
    }
}
#[tauri::command]
pub fn session_snapshot(remote: crate::sessions::RemoteSession) -> RemoteSnapshot {
    RemoteSnapshot {
        session: remote.engine.snapshot(),
        capabilities: capture_capabilities(),
        files_supported: !cfg!(target_os = "android"),
        overlay_supported: remote.is_main()
            && crate::overlay::supported()
            && remote.overlay_display_supported.load(Ordering::SeqCst),
        audio_supported: assi_audio::AudioCapture::supported(),
        terminal_supported: assi_terminal::TerminalManager::supported(),
        audio_error: remote
            .audio_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
        overlay_enabled: remote.overlay_enabled.load(Ordering::SeqCst),
        media_error: remote
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
    }
}
#[tauri::command]
pub async fn session_host(
    remote: crate::sessions::RemoteSession,
    store: State<'_, crate::AppStore>,
    ip: String,
) -> Result<String, String> {
    if !capture_capabilities().capture {
        return Err("На этой платформе доступен только просмотр других устройств".into());
    }
    let ip: IpAddr = ip
        .trim()
        .parse()
        .map_err(|_| "Укажите корректный IP этого устройства")?;
    if ip.is_unspecified() || ip.is_multicast() {
        return Err("Укажите конкретный IP этого устройства".into());
    }
    let name = crate::with_store(store, |s| Ok(s.snapshot()?.settings.device_name)).await?;
    let _guard = remote.lifecycle.lock().await;
    remote.reset().await?;
    remote
        .engine
        .start_host(SocketAddr::new(ip, 0), ip, name)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn session_connect(
    remote: crate::sessions::RemoteSession,
    store: State<'_, crate::AppStore>,
    invitation: String,
) -> Result<(), String> {
    let name = crate::with_store(store, |s| Ok(s.snapshot()?.settings.device_name)).await?;
    let _guard = remote.lifecycle.lock().await;
    remote.reset().await?;
    remote
        .engine
        .connect(invitation, name)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn session_accept(
    remote: crate::sessions::RemoteSession,
    request_id: String,
    control: bool,
) -> Result<(), String> {
    if control && !capture_capabilities().control {
        return Err("Управление недоступно на этой платформе".into());
    }
    #[cfg(target_os = "android")]
    {
        let current = remote.engine.snapshot();
        if current.state != SessionState::Pending
            || current.role != Role::Host
            || current.request_id.as_deref() != Some(&request_id)
        {
            return Err("Запрос больше не актуален".into());
        }
        let app = remote.app.as_ref().ok_or("Нет Android runtime")?;
        // Never hold lifecycle while the system asks the owner for consent.
        if let Err(error) = assi_capture::request_capture(app, request_id.clone()).await {
            let _ = assi_capture::stop_request(app, &request_id).await;
            return Err(error);
        }
    }
    let _guard = remote.lifecycle.lock().await;
    let result = remote
        .engine
        .accept(request_id.clone(), control)
        .await
        .map_err(|e| e.to_string());
    #[cfg(target_os = "android")]
    if result.is_err() {
        if let Some(app) = remote.app.as_ref().filter(|_| remote.is_main()) {
            let _ = assi_capture::stop_request(app, &request_id).await;
        }
    }
    result
}
#[tauri::command]
pub async fn session_reject(
    remote: crate::sessions::RemoteSession,
    request_id: String,
) -> Result<(), String> {
    remote
        .engine
        .reject(request_id)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn session_stop(remote: crate::sessions::RemoteSession) -> Result<(), String> {
    remote.shutdown().await
}
#[tauri::command]
pub async fn session_frame(
    remote: crate::sessions::RemoteSession,
) -> Result<tauri::ipc::Response, String> {
    Ok(tauri::ipc::Response::new(remote.preview_jpeg().await?))
}
#[tauri::command]
pub async fn session_input(
    remote: crate::sessions::RemoteSession,
    input: InputEvent,
    request_id: String,
    screen_epoch: u64,
) -> Result<(), String> {
    let _guard = remote.lifecycle.lock().await;
    if remote.engine.snapshot().request_id.as_deref() != Some(&request_id) {
        return Err("Сеанс уже изменился".into());
    }
    remote
        .engine
        .send_input_at(request_id, screen_epoch, input)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn stop_revokes_before_waiting_for_busy_media() {
        // No capture or input: holding the media lock models a slow OS call.
        let remote = Remote::new(None);
        remote
            .engine
            .start_host(
                "127.0.0.1:0".parse().unwrap(),
                "127.0.0.1".parse().unwrap(),
                "Test".into(),
            )
            .await
            .unwrap();
        let busy = remote.lifecycle.lock().await;
        let clone = remote.clone();
        let stopping = tokio::spawn(async move { clone.shutdown().await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while remote.engine.snapshot().state != SessionState::Closed {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            !stopping.is_finished(),
            "native cleanup still waits for the media operation"
        );
        assert!(!remote.engine.snapshot().control);
        drop(busy);
        stopping.await.unwrap().unwrap();
    }
}

#[tauri::command]
pub async fn session_host_relay(
    remote: crate::sessions::RemoteSession,
    store: State<'_, crate::AppStore>,
    config: String,
) -> Result<String, String> {
    if !capture_capabilities().capture {
        return Err("Показ экрана недоступен".into());
    }
    let config = assi_relay::RelayConfig::decode(config.trim())
        .map_err(|_| "Некорректная конфигурация сервера")?;
    let name = crate::with_store(store, |s| Ok(s.snapshot()?.settings.device_name)).await?;
    let _guard = remote.lifecycle.lock().await;
    remote.reset().await?;
    remote
        .engine
        .start_host_relay(config, name)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(not(target_os = "android"))]
fn require_file_session(remote: &Remote, request_id: &str) -> Result<Snapshot, String> {
    let snapshot = remote.engine.snapshot();
    if snapshot.state != SessionState::Active || snapshot.request_id.as_deref() != Some(request_id)
    {
        return Err("Сеанс уже завершён или изменился".into());
    }
    Ok(snapshot)
}

/// Paths are never accepted from JS: a local owner chooses a file with the OS picker.
#[tauri::command]
pub async fn session_file_offer(
    app: tauri::AppHandle,
    remote: crate::sessions::RemoteSession,
    request_id: String,
) -> Result<bool, String> {
    #[cfg(target_os = "android")]
    {
        let _ = (app, remote, request_id);
        Err("Выбор файлов Android ещё не подключён".into())
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
            .set_title("Выберите файл для отправки")
            .pick_files(move |path| {
                let _ = tx.send(path);
            });
        let Some(path) = rx.await.map_err(|_| "Выбор файла прерван")? else {
            return Ok(false);
        };
        require_file_session(&remote, &request_id)?;
        let paths = path
            .into_iter()
            .map(|p| {
                p.into_path()
                    .map_err(|_| "Неподдерживаемый источник файла".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        remote
            .file_queue
            .lock()
            .map_err(|_| "Ошибка очереди")?
            .enqueue(&request_id, paths)?;
        Ok(true)
    }
}
#[tauri::command]
pub async fn session_file_accept(
    app: tauri::AppHandle,
    remote: crate::sessions::RemoteSession,
    request_id: String,
    transfer_id: String,
) -> Result<bool, String> {
    #[cfg(target_os = "android")]
    {
        let _ = (app, remote, request_id, transfer_id);
        Err("Сохранение файлов Android ещё не подключено".into())
    }
    #[cfg(not(target_os = "android"))]
    {
        use tauri_plugin_dialog::DialogExt;
        let _dialog = remote
            .file_dialog
            .try_lock()
            .map_err(|_| "Окно выбора файла уже открыто")?;
        let snapshot = require_file_session(&remote, &request_id)?;
        let transfer = snapshot
            .transfer
            .as_ref()
            .filter(|t| t.id == transfer_id)
            .ok_or("Предложение файла уже не актуально")?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let owner = app
            .get_webview_window(remote.label())
            .ok_or("Окно сеанса закрыто")?;
        app.dialog()
            .file()
            .set_parent(&owner)
            .set_title("Сохранить входящий файл под новым именем")
            .set_file_name(&transfer.name)
            .save_file(move |path| {
                let _ = tx.send(path);
            });
        let Some(path) = rx.await.map_err(|_| "Выбор места сохранения прерван")?
        else {
            return Ok(false);
        };
        let path = path
            .into_path()
            .map_err(|_| "Это место сохранения пока не поддерживается")?;
        remote
            .engine
            .accept_file(request_id, transfer_id, path)
            .await
            .map_err(|e| e.to_string())?;
        Ok(true)
    }
}
#[tauri::command]
pub async fn session_file_reject(
    remote: crate::sessions::RemoteSession,
    request_id: String,
    transfer_id: String,
) -> Result<(), String> {
    remote
        .engine
        .reject_file(request_id, transfer_id)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn session_file_cancel(
    remote: crate::sessions::RemoteSession,
    request_id: String,
    transfer_id: String,
) -> Result<(), String> {
    let _guard = remote.lifecycle.lock().await;
    if remote.cancel_folder_extraction(&request_id, &transfer_id) {
        return Ok(());
    }
    remote
        .engine
        .cancel_file(request_id, transfer_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn session_annotations(
    remote: crate::sessions::RemoteSession,
    request_id: String,
) -> Result<assi_session::AnnotationSnapshot, String> {
    let board = remote.engine.annotations();
    if board.request_id.as_deref() != Some(&request_id) {
        return Err("Сеанс уже изменился".into());
    }
    Ok(board)
}
#[tauri::command]
pub async fn session_annotate(
    remote: crate::sessions::RemoteSession,
    request_id: String,
    command: assi_session::AnnotationCommand,
    screen_epoch: Option<u64>,
) -> Result<(), String> {
    let epoch = screen_epoch.unwrap_or_else(|| remote.engine.snapshot().screen_epoch);
    remote
        .engine
        .annotate_at(request_id, epoch, command)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub fn session_overlay_snapshot(
    window: tauri::WebviewWindow,
    remote: crate::sessions::RemoteSession,
) -> Option<assi_session::AnnotationSnapshot> {
    if !remote.is_main()
        || window.label() != crate::overlay::LABEL
        || !remote.overlay_enabled.load(Ordering::SeqCst)
    {
        return None;
    }
    let state = remote.engine.snapshot();
    if state.state != SessionState::Active || state.role != Role::Host {
        return None;
    }
    let board = remote.engine.annotations();
    (board.request_id == state.request_id).then_some(board)
}
#[tauri::command]
pub async fn session_overlay_enable(
    remote: crate::sessions::RemoteSession,
    request_id: String,
    enabled: bool,
) -> Result<(), String> {
    let _guard = remote.lifecycle.lock().await;
    let state = remote.engine.snapshot();
    if !remote.is_main()
        || !crate::overlay::supported()
        || !remote.overlay_display_supported.load(Ordering::SeqCst)
        || state.state != SessionState::Active
        || state.role != Role::Host
        || state.request_id.as_deref() != Some(&request_id)
    {
        return Err("Пометки поверх рабочего стола недоступны".into());
    }
    remote.overlay_enabled.store(enabled, Ordering::SeqCst);
    if !enabled {
        if let Some(app) = remote.app.as_ref().filter(|_| remote.is_main()) {
            crate::overlay::hide(app)?;
        }
    }
    Ok(())
}
#[tauri::command]
pub async fn session_fullscreen(
    window: tauri::WebviewWindow,
    remote: crate::sessions::RemoteSession,
    enabled: bool,
    request_id: String,
) -> Result<(), String> {
    let _guard = remote.lifecycle.lock().await;
    if window.label() != remote.window_label {
        return Err("Недоступное окно".into());
    }
    let snapshot = remote.engine.snapshot();
    if !enabled {
        if snapshot.state == SessionState::Active
            && snapshot.request_id.as_deref() != Some(&request_id)
        {
            return Err("Сеанс уже изменился".into());
        }
        return remote.restore_fullscreen();
    }
    if snapshot.state != SessionState::Active || snapshot.request_id.as_deref() != Some(&request_id)
    {
        return Err("Сеанс не активен".into());
    }
    let mut restore = remote
        .fullscreen_restore
        .lock()
        .map_err(|_| "Ошибка окна")?;
    if restore.is_none() {
        let was = window.is_fullscreen().map_err(|e| e.to_string())?;
        #[cfg(not(target_os = "android"))]
        window.set_fullscreen(true).map_err(|e| e.to_string())?;
        *restore = Some(was);
    }
    Ok(())
}

#[tauri::command]
pub async fn session_audio_enable(
    remote: crate::sessions::RemoteSession,
    request_id: String,
    enabled: bool,
) -> Result<(), String> {
    let _guard = remote.lifecycle.lock().await;
    let snapshot = remote.engine.snapshot();
    if snapshot.state != SessionState::Active
        || snapshot.role != Role::Host
        || snapshot.request_id.as_deref() != Some(&request_id)
    {
        return Err("Нет активного показа экрана".into());
    }
    if enabled {
        remote
            .audio
            .lock()
            .map_err(|_| "Ошибка состояния звука")?
            .start()?;
        if let Err(error) = remote.engine.set_audio_enabled(request_id, true).await {
            remote.stop_audio();
            return Err(error.to_string());
        }
        *remote.audio_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
    } else {
        remote
            .engine
            .set_audio_enabled(request_id, false)
            .await
            .map_err(|e| e.to_string())?;
        remote.stop_audio();
    }
    Ok(())
}
#[tauri::command]
pub fn session_audio(
    remote: crate::sessions::RemoteSession,
    request_id: String,
) -> Result<assi_session::AudioBatch, String> {
    remote
        .engine
        .take_audio_batch(request_id)
        .map_err(|e| e.to_string())
}
#[derive(Serialize)]
pub struct CaptureOptions {
    displays: Vec<assi_platform::DisplayInfo>,
    selected: Option<String>,
    quality: assi_platform::CaptureQuality,
}
fn require_host(remote: &Remote, id: &str) -> Result<(), String> {
    let snapshot = remote.engine.snapshot();
    if snapshot.state != SessionState::Active
        || snapshot.role != Role::Host
        || snapshot.request_id.as_deref() != Some(id)
    {
        return Err("Настройки экрана доступны его владельцу во время сеанса".into());
    }
    Ok(())
}
#[tauri::command]
pub async fn session_capture_options(
    remote: crate::sessions::RemoteSession,
    request_id: String,
) -> Result<CaptureOptions, String> {
    let _guard = remote.lifecycle.lock().await;
    require_host(&remote, &request_id)?;
    #[cfg(not(target_os = "android"))]
    {
        let platform = remote.platform.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let adapter = platform.lock().map_err(|_| "Ошибка экрана")?;
            Ok(CaptureOptions {
                displays: Platform::displays().map_err(|e| e.to_string())?,
                selected: adapter.selected_display(),
                quality: adapter.quality(),
            })
        })
        .await
        .map_err(|e| e.to_string())?
    }
    #[cfg(target_os = "android")]
    {
        Err("На Android экран выбирается в системном запросе показа".into())
    }
}
#[tauri::command]
pub async fn session_capture_set(
    remote: crate::sessions::RemoteSession,
    request_id: String,
    display: String,
    quality: assi_platform::CaptureQuality,
) -> Result<(), String> {
    let _guard = remote.lifecycle.lock().await;
    require_host(&remote, &request_id)?;
    #[cfg(not(target_os = "android"))]
    {
        let platform = remote.platform.clone();
        let engine = remote.engine.clone();
        let request = request_id.clone();
        let outcome =
            tauri::async_runtime::spawn_blocking(move || -> Result<(bool, bool), String> {
                let mut adapter = platform.lock().map_err(|_| "Ошибка экрана")?;
                let displays = Platform::displays().map_err(|e| e.to_string())?;
                let chosen = displays
                    .iter()
                    .find(|d| d.id == display)
                    .ok_or("Монитор недоступен")?;
                let current = adapter
                    .selected_display()
                    .or_else(|| displays.iter().find(|d| d.primary).map(|d| d.id.clone()));
                let changed = current.as_deref() != Some(&display);
                if changed {
                    if engine.snapshot().displays.is_empty() {
                        engine
                            .set_displays(
                                request.clone(),
                                displays
                                    .iter()
                                    .map(|d| assi_session::RemoteDisplay {
                                        id: d.id.clone(),
                                        name: d.name.clone(),
                                        width: d.width,
                                        height: d.height,
                                        primary: d.primary,
                                    })
                                    .collect(),
                                current.clone().ok_or("Монитор не найден")?,
                            )
                            .map_err(|e| e.to_string())?;
                    }
                    adapter.release_inputs().map_err(|e| e.to_string())?;
                    adapter
                        .select_display(display.clone())
                        .map_err(|e| e.to_string())?;
                    engine
                        .commit_display(request, display)
                        .map_err(|e| e.to_string())?;
                }
                adapter.set_quality(quality);
                Ok((chosen.primary, changed))
            })
            .await
            .map_err(|e| e.to_string())?;
        let (primary, changed) = match outcome {
            Ok(value) => value,
            Err(error) => {
                let _ = remote.engine.stop().await;
                let _ = remote.release().await;
                return Err(error);
            }
        };
        remote
            .capture_fps
            .store(u64::from(quality.max_fps()), Ordering::Relaxed);
        remote
            .overlay_display_supported
            .store(primary, Ordering::SeqCst);
        if !primary {
            if let Some(app) = remote.app.as_ref().filter(|_| remote.is_main()) {
                crate::overlay::hide(app)?;
            }
        }
        if changed {
            remote.engine.take_input();
            remote
                .engine
                .annotate(request_id, assi_session::AnnotationCommand::Clear)
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    #[cfg(target_os = "android")]
    {
        let _ = (display, quality);
        Err("На Android экран выбирается в системном запросе показа".into())
    }
}
#[tauri::command]
pub async fn session_save_image(
    app: tauri::AppHandle,
    remote: crate::sessions::RemoteSession,
    request_id: String,
    bytes: Vec<u8>,
) -> Result<bool, String> {
    if bytes.len() < 33
        || bytes.len() > 12 * 1024 * 1024
        || !bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || &bytes[12..16] != b"IHDR"
    {
        return Err("Некорректный PNG кадр".into());
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().map_err(|_| "Некорректный кадр")?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().map_err(|_| "Некорректный кадр")?);
    if width == 0
        || height == 0
        || width > 4096
        || height > 4096
        || u64::from(width) * u64::from(height) > 8_388_608
    {
        return Err("Размер кадра превышает ограничение".into());
    }
    #[cfg(not(target_os = "android"))]
    require_file_session(&remote, &request_id)?;
    #[cfg(not(target_os = "android"))]
    {
        use tauri_plugin_dialog::DialogExt;
        let _dialog = remote
            .file_dialog
            .try_lock()
            .map_err(|_| "Окно выбора файла уже открыто")?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let owner = app
            .get_webview_window(remote.label())
            .ok_or("Окно сеанса закрыто")?;
        app.dialog()
            .file()
            .set_parent(&owner)
            .set_title("Сохранить кадр под новым именем")
            .set_file_name("Асси-кадр.png")
            .add_filter("PNG", &["png"])
            .save_file(move |path| {
                let _ = tx.send(path);
            });
        let Some(path) = rx.await.map_err(|_| "Выбор места сохранения прерван")?
        else {
            return Ok(false);
        };
        require_file_session(&remote, &request_id)?;
        let path = path
            .into_path()
            .map_err(|_| "Неподдерживаемое место сохранения")?;
        tauri::async_runtime::spawn_blocking(move || -> Result<bool, String> {
            use std::io::Write;
            let parent = path.parent().ok_or("Не удалось определить папку")?;
            let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
            tmp.write_all(&bytes).map_err(|e| e.to_string())?;
            tmp.as_file().sync_all().map_err(|e| e.to_string())?;
            tmp.persist_noclobber(path).map_err(|e| e.to_string())?;
            Ok(true)
        })
        .await
        .map_err(|e| e.to_string())?
    }
    #[cfg(target_os = "android")]
    {
        let _ = (app, remote, request_id);
        Err("Сохранение кадра на Android пока недоступно".into())
    }
}
