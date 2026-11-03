//! Window identity, never caller-provided JSON, selects a remote session.
use crate::{recording::RecordingState, remote::Remote};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    ops::Deref,
    sync::{Arc, Mutex},
};
use tauri::{
    ipc::{CommandArg, CommandItem, InvokeError},
    Manager, Runtime, State,
};
#[cfg(any(not(target_os = "android"), test))]
const MAX_VIEWERS: usize = 4;
struct Entry {
    remote: Arc<Remote>,
    recording: Arc<RecordingState>,
    closing: bool,
}
#[derive(Default)]
pub struct SessionRegistry {
    inner: Mutex<Registry>,
}
#[derive(Default)]
struct Registry {
    entries: BTreeMap<String, Entry>,
    #[cfg(any(not(target_os = "android"), test))]
    next: u64,
    closing: bool,
}
#[cfg(any(not(target_os = "android"), test))]
impl Registry {
    fn next_label(&mut self) -> Result<String, String> {
        if self.closing {
            return Err("Приложение закрывается".into());
        }
        if self
            .entries
            .keys()
            .filter(|label| label.as_str() != "main")
            .count()
            >= MAX_VIEWERS
        {
            return Err("Можно открыть до четырёх дополнительных сеансов".into());
        }
        self.next = self.next.checked_add(1).ok_or("Достигнут предел окон")?;
        Ok(format!("viewer-{}", self.next))
    }
}
impl SessionRegistry {
    pub fn with_main(remote: Arc<Remote>) -> Self {
        let mut entries = BTreeMap::new();
        entries.insert(
            "main".into(),
            Entry {
                remote,
                recording: Arc::new(RecordingState::default()),
                closing: false,
            },
        );
        Self {
            inner: Mutex::new(Registry {
                entries,
                ..Default::default()
            }),
        }
    }
    fn lookup(&self, label: &str) -> Result<(Arc<Remote>, Arc<RecordingState>), String> {
        let registry = self.inner.lock().map_err(|_| "Ошибка сеансов")?;
        if registry.closing {
            return Err("Приложение закрывается".into());
        }
        let entry = registry
            .entries
            .get(label)
            .filter(|entry| !entry.closing)
            .ok_or("Окно сеанса уже закрыто")?;
        Ok((entry.remote.clone(), entry.recording.clone()))
    }
    #[cfg(not(target_os = "android"))]
    fn reserve(&self, app: tauri::AppHandle) -> Result<String, String> {
        let mut registry = self.inner.lock().map_err(|_| "Ошибка сеансов")?;
        let label = registry.next_label()?;
        let remote = Remote::new_for_window(Some(app), label.clone());
        registry.entries.insert(
            label.clone(),
            Entry {
                remote,
                recording: Arc::new(RecordingState::default()),
                closing: false,
            },
        );
        Ok(label)
    }
    pub async fn close(&self, label: &str) -> Result<(), String> {
        let resources = {
            let mut registry = self.inner.lock().map_err(|_| "Ошибка сеансов")?;
            let Some(entry) = registry.entries.get_mut(label) else {
                return Ok(());
            };
            entry.closing = true;
            (entry.remote.clone(), entry.recording.clone())
        };
        resources.1.cancel();
        let result = resources.0.terminate().await;
        self.inner
            .lock()
            .map_err(|_| "Ошибка сеансов")?
            .entries
            .remove(label);
        result
    }
    pub async fn shutdown_all(&self) {
        let labels = {
            let mut registry = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            registry.closing = true;
            registry.entries.keys().cloned().collect::<Vec<_>>()
        };
        for label in labels {
            let _ = self.close(&label).await;
        }
    }
}
/// Validates the command/window boundary even if a capability is accidentally broadened.
fn route(label: &str, command: &str) -> Result<&'static str, String> {
    if label == "annotations-overlay" {
        return if command == "session_overlay_snapshot" {
            Ok("overlay")
        } else {
            Err("Окно пометок доступно только для чтения".into())
        };
    }
    if label == "main" {
        return if crate::ipc_catalog::ALL_COMMANDS.contains(&command) {
            Ok("main")
        } else {
            Err("Неизвестная команда".into())
        };
    }
    if !label
        .strip_prefix("viewer-")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("Неизвестное окно".into());
    }
    if !crate::ipc_catalog::VIEWER_COMMANDS.contains(&command) {
        return Err("Это действие доступно только в главном окне".into());
    }
    Ok("viewer")
}
#[derive(Clone)]
pub struct RemoteSession {
    remote: Arc<Remote>,
    label: String,
}
impl RemoteSession {
    pub fn inner(&self) -> &Arc<Remote> {
        &self.remote
    }
    pub fn label(&self) -> &str {
        &self.label
    }
}
impl Deref for RemoteSession {
    type Target = Arc<Remote>;
    fn deref(&self) -> &Self::Target {
        &self.remote
    }
}
impl<'de, R: Runtime> CommandArg<'de, R> for RemoteSession {
    fn from_command(command: CommandItem<'de, R>) -> Result<Self, InvokeError> {
        let window = command.message.webview_ref();
        let label = window.label();
        let kind = route(label, command.name).map_err(InvokeError::from)?;
        let key = if kind == "overlay" { "main" } else { label };
        let registry = window
            .try_state::<Arc<SessionRegistry>>()
            .ok_or_else(|| InvokeError::from("Сеансы не инициализированы"))?;
        let (remote, _) = registry.lookup(key).map_err(InvokeError::from)?;
        Ok(Self {
            remote,
            label: key.into(),
        })
    }
}
pub struct RecordingSession(Arc<RecordingState>);
impl RecordingSession {
    pub fn inner(&self) -> &Arc<RecordingState> {
        &self.0
    }
}
impl Deref for RecordingSession {
    type Target = Arc<RecordingState>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<'de, R: Runtime> CommandArg<'de, R> for RecordingSession {
    fn from_command(command: CommandItem<'de, R>) -> Result<Self, InvokeError> {
        let window = command.message.webview_ref();
        route(window.label(), command.name).map_err(InvokeError::from)?;
        let registry = window
            .try_state::<Arc<SessionRegistry>>()
            .ok_or_else(|| InvokeError::from("Сеансы не инициализированы"))?;
        let (_, recording) = registry.lookup(window.label()).map_err(InvokeError::from)?;
        Ok(Self(recording))
    }
}
pub struct MainWindow;
impl<'de, R: Runtime> CommandArg<'de, R> for MainWindow {
    fn from_command(command: CommandItem<'de, R>) -> Result<Self, InvokeError> {
        if command.message.webview_ref().label() != "main" {
            return Err(InvokeError::from(
                "Это действие доступно только в главном окне",
            ));
        }
        let registry = command
            .message
            .webview_ref()
            .try_state::<Arc<SessionRegistry>>()
            .ok_or_else(|| InvokeError::from("Сеансы не инициализированы"))?;
        registry.lookup("main").map_err(InvokeError::from)?;
        Ok(Self)
    }
}
#[derive(Serialize)]
pub struct WindowSession {
    label: String,
    peer_name: Option<String>,
    state: assi_session::SessionState,
    role: assi_session::Role,
    current: bool,
}
#[derive(Serialize)]
pub struct WindowSessions {
    current: String,
    viewer_only: bool,
    supported: bool,
    windows: Vec<WindowSession>,
}
#[tauri::command]
pub fn session_windows(
    remote: RemoteSession,
    registry: State<'_, Arc<SessionRegistry>>,
) -> Result<WindowSessions, String> {
    let entries = registry.inner.lock().map_err(|_| "Ошибка сеансов")?;
    let windows = entries
        .entries
        .iter()
        .filter(|(_, entry)| !entry.closing)
        .map(|(label, entry)| {
            let snapshot = entry.remote.recording_snapshot();
            WindowSession {
                label: label.clone(),
                peer_name: snapshot.peer_name,
                state: snapshot.state,
                role: snapshot.role,
                current: label == remote.label(),
            }
        })
        .collect();
    Ok(WindowSessions {
        current: remote.label().into(),
        viewer_only: remote.label() != "main",
        supported: !cfg!(target_os = "android"),
        windows,
    })
}
#[tauri::command]
pub async fn session_window_open(
    app: tauri::AppHandle,
    _remote: RemoteSession,
    registry: State<'_, Arc<SessionRegistry>>,
) -> Result<String, String> {
    #[cfg(target_os = "android")]
    {
        let _ = (app, registry);
        Err("Несколько окон доступны на компьютере".into())
    }
    #[cfg(not(target_os = "android"))]
    {
        let label = registry.reserve(app.clone())?;
        let result = tauri::WebviewWindowBuilder::new(
            &app,
            &label,
            tauri::WebviewUrl::App("index.html".into()),
        )
        .title("Асси — новый сеанс")
        .inner_size(1100.0, 780.0)
        .resizable(true)
        .build();
        if result.is_err() {
            let _ = registry.close(&label).await;
            return Err("Не удалось открыть окно сеанса".into());
        }
        let (remote, _) = registry.lookup(&label)?;
        tauri::async_runtime::spawn(remote.pump());
        Ok(label)
    }
}
#[tauri::command]
pub fn session_window_focus(
    app: tauri::AppHandle,
    _remote: RemoteSession,
    registry: State<'_, Arc<SessionRegistry>>,
    label: String,
) -> Result<(), String> {
    registry.lookup(&label)?;
    let window = app.get_webview_window(&label).ok_or("Окно уже закрыто")?;
    window.show().map_err(|_| "Не удалось показать окно")?;
    window
        .set_focus()
        .map_err(|_| "Не удалось выбрать окно".into())
}
#[tauri::command]
pub async fn session_window_close(
    app: tauri::AppHandle,
    _remote: RemoteSession,
    registry: State<'_, Arc<SessionRegistry>>,
    label: String,
) -> Result<(), String> {
    if label == "main" {
        return Err("Главное окно закрывается собственной кнопкой".into());
    }
    registry.lookup(&label)?;
    let window = app.get_webview_window(&label).ok_or("Окно уже закрыто")?;
    let cleanup = registry.close(&label).await;
    let destroyed = window
        .destroy()
        .map_err(|_| "Не удалось закрыть окно".to_string());
    cleanup.and(destroyed)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_acl_and_real_ipc_route_each_window_and_reject_host_escalation() {
        use tauri::test::{get_ipc_response, mock_builder, INVOKE_KEY};
        let main_remote = Remote::new(None);
        main_remote.report_error("main-session".into());
        let viewer_remote = Remote::new_for_window(None, "viewer-1".into());
        viewer_remote.report_error("viewer-session".into());
        let registry = Arc::new(SessionRegistry::with_main(main_remote));
        registry.inner.lock().unwrap().entries.insert(
            "viewer-1".into(),
            Entry {
                remote: viewer_remote,
                recording: Arc::new(RecordingState::default()),
                closing: false,
            },
        );
        // Deliberately use generated application ACL, not mock_context's empty ACL.
        let app = mock_builder()
            .manage(registry)
            .invoke_handler(tauri::generate_handler![
                crate::remote::session_snapshot,
                crate::remote::session_chat,
                crate::remote::session_host,
                crate::remote::session_stop
            ])
            .build(crate::app_context())
            .unwrap();
        let main = app.get_webview_window("main").unwrap_or_else(|| {
            tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
                .build()
                .unwrap()
        });
        let viewer = tauri::WebviewWindowBuilder::new(&app, "viewer-1", Default::default())
            .build()
            .unwrap();
        let overlay =
            tauri::WebviewWindowBuilder::new(&app, "annotations-overlay", Default::default())
                .build()
                .unwrap();
        let request = |name: &str, body: serde_json::Value| tauri::webview::InvokeRequest {
            cmd: name.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: if cfg!(any(windows, target_os = "android")) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            }
            .parse()
            .unwrap(),
            body: tauri::ipc::InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.into(),
        };
        let main_snapshot =
            get_ipc_response(&main, request("session_snapshot", serde_json::json!({})))
                .unwrap()
                .deserialize::<serde_json::Value>()
                .unwrap();
        let viewer_snapshot =
            get_ipc_response(&viewer, request("session_snapshot", serde_json::json!({})))
                .unwrap()
                .deserialize::<serde_json::Value>()
                .unwrap();
        assert_eq!(main_snapshot["media_error"], "main-session");
        assert_eq!(viewer_snapshot["media_error"], "viewer-session");
        // New tool command reaches its actual handler and reports inactive session.
        let chat_error = get_ipc_response(
            &main,
            request(
                "session_chat",
                serde_json::json!({"requestId":"not-active"}),
            ),
        )
        .unwrap_err();
        assert!(chat_error.as_str().unwrap().contains("Сеанс"));
        assert!(!chat_error.to_string().contains("not allowed"));
        let denied_host = get_ipc_response(
            &viewer,
            request("session_host", serde_json::json!({"ip":"127.0.0.1"})),
        )
        .unwrap_err();
        assert!(
            denied_host.to_string().contains("not allowed"),
            "{denied_host}"
        );
        let denied_overlay =
            get_ipc_response(&overlay, request("session_stop", serde_json::json!({}))).unwrap_err();
        assert!(
            denied_overlay.to_string().contains("not allowed"),
            "{denied_overlay}"
        );
    }
    async fn wait_state(engine: &assi_session::SessionEngine, state: assi_session::SessionState) {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while engine.snapshot().state != state {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn simultaneous_tls_sessions_keep_commands_and_close_isolated() {
        use assi_session::{SessionEngine, SessionState};
        let registry = SessionRegistry::with_main(Remote::new(None));
        let first = Remote::new_for_window(None, "viewer-1".into());
        let second = Remote::new_for_window(None, "viewer-2".into());
        {
            let mut guard = registry.inner.lock().unwrap();
            for (label, remote) in [("viewer-1", first.clone()), ("viewer-2", second.clone())] {
                guard.entries.insert(
                    label.into(),
                    Entry {
                        remote,
                        recording: Arc::new(RecordingState::default()),
                        closing: false,
                    },
                );
            }
        }
        let first_host = SessionEngine::new();
        let second_host = SessionEngine::new();
        let address: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        for (host, viewer) in [(&first_host, &first), (&second_host, &second)] {
            let invite = host
                .start_host(address, address.ip(), "Host".into())
                .await
                .unwrap();
            viewer
                .engine
                .connect(invite, "Viewer".into())
                .await
                .unwrap();
            wait_state(host, SessionState::Pending).await;
            host.accept(host.snapshot().request_id.unwrap(), false)
                .await
                .unwrap();
            wait_state(&viewer.engine, SessionState::Active).await;
        }
        let first_id = first.engine.snapshot().request_id.unwrap();
        let second_id = second.engine.snapshot().request_id.unwrap();
        assert_ne!(first_id, second_id);
        assert!(second
            .engine
            .send_chat(first_id.clone(), "Wrong window".into())
            .await
            .is_err());
        first
            .engine
            .send_chat(first_id.clone(), "Only first".into())
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while first_host
                .chat(first_id.clone())
                .unwrap()
                .messages
                .is_empty()
            {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(second_host.chat(second_id).unwrap().messages.is_empty());
        registry.close("viewer-1").await.unwrap();
        assert_eq!(second.engine.snapshot().state, SessionState::Active);
        assert_eq!(second_host.snapshot().state, SessionState::Active);
        registry.shutdown_all().await;
        first_host.stop().await.unwrap();
        second_host.stop().await.unwrap();
    }
    #[tokio::test]
    async fn closing_viewer_stops_both_pumps_and_drops_its_remote_only() {
        let main = Remote::new(None);
        let registry = SessionRegistry::with_main(main.clone());
        let child = Remote::new_for_window(None, "viewer-1".into());
        let weak = Arc::downgrade(&child);
        registry.inner.lock().unwrap().entries.insert(
            "viewer-1".into(),
            Entry {
                remote: child.clone(),
                recording: Arc::new(RecordingState::default()),
                closing: false,
            },
        );
        let pump = tokio::spawn(child.pump());
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        registry.close("viewer-1").await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), pump)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while weak.upgrade().is_some() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(registry.lookup("viewer-1").is_err());
        assert!(Arc::ptr_eq(&registry.lookup("main").unwrap().0, &main));
        assert!(!main.closed.load(std::sync::atomic::Ordering::Acquire));
    }
    #[test]
    fn window_limit_and_exit_gate_prevent_unbounded_pumps() {
        let mut registry = Registry::default();
        for _ in 0..MAX_VIEWERS {
            let label = registry.next_label().unwrap();
            registry.entries.insert(
                label,
                Entry {
                    remote: Remote::new(None),
                    recording: Arc::new(RecordingState::default()),
                    closing: false,
                },
            );
        }
        assert!(registry.next_label().is_err());
        registry.entries.remove("viewer-1");
        assert_eq!(registry.next_label().unwrap(), "viewer-5");
        registry.closing = true;
        assert!(registry.next_label().is_err());
    }
    #[test]
    fn viewer_cannot_host_modify_access_or_impersonate_overlay() {
        for command in [
            "session_host",
            "session_accept",
            "access_enable",
            "access_disable",
            "access_invitation",
            "session_terminal_allow",
            "session_system_decide",
            "save_settings",
        ] {
            assert!(route("viewer-1", command).is_err(), "{command}");
        }
        assert!(route("viewer-1", "session_connect").is_ok());
        assert!(route("viewer-1", "recording_chunk").is_ok());
        assert!(route("viewer-bad", "session_connect").is_err());
        assert_eq!(
            route("annotations-overlay", "session_overlay_snapshot").unwrap(),
            "overlay"
        );
        assert!(route("annotations-overlay", "session_connect").is_err());
    }
    #[test]
    fn lookup_uses_exact_registered_window_and_recordings_are_isolated() {
        let main = Remote::new(None);
        let registry = SessionRegistry::with_main(main.clone());
        assert!(Arc::ptr_eq(&registry.lookup("main").unwrap().0, &main));
        assert!(registry.lookup("viewer-1").is_err());
        let child = Remote::new(None);
        registry.inner.lock().unwrap().entries.insert(
            "viewer-1".into(),
            Entry {
                remote: child.clone(),
                recording: Arc::new(RecordingState::default()),
                closing: false,
            },
        );
        assert!(Arc::ptr_eq(&registry.lookup("viewer-1").unwrap().0, &child));
        assert!(!Arc::ptr_eq(
            &registry.lookup("main").unwrap().1,
            &registry.lookup("viewer-1").unwrap().1
        ));
        registry
            .inner
            .lock()
            .unwrap()
            .entries
            .get_mut("viewer-1")
            .unwrap()
            .closing = true;
        assert!(registry.lookup("viewer-1").is_err());
    }
}
