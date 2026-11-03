//! Local UI boundary for optional persistent access. Nothing is enabled by default.
use crate::remote::Remote;
use crate::sessions::{MainWindow, RemoteSession};
use assi_access::{AccessConfig, AccessIdentity, AccessStore};
use serde::Serialize;
use std::sync::Arc;
use tauri::Manager;

#[derive(Serialize)]
pub struct AccessStatus {
    supported: bool,
    enabled: bool,
    configured: bool,
    config: Option<AccessConfig>,
    autostart: bool,
    scope: &'static str,
}
fn supported() -> bool {
    !cfg!(target_os = "android") && assi_platform::Platform::capabilities().capture
}
fn store(app: &tauri::AppHandle) -> Result<AccessStore, String> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|_| "Не удалось определить папку приложения")?;
    std::fs::create_dir_all(&base).map_err(|_| "Не удалось открыть папку приложения")?;
    Ok(AccessStore::new(base.join("access")))
}
fn host_identity(identity: AccessIdentity) -> assi_session::HostIdentity {
    assi_session::HostIdentity {
        certificate_der: identity.certificate_der,
        private_key_pkcs8: identity.private_key_pkcs8,
        token: identity.token,
    }
}
#[tauri::command]
pub fn access_status(app: tauri::AppHandle, remote: RemoteSession) -> Result<AccessStatus, String> {
    if !supported() {
        return Ok(AccessStatus {
            supported: false,
            enabled: false,
            configured: false,
            config: None,
            autostart: false,
            scope: "user_session",
        });
    }
    let identity = store(&app)?.load().map_err(|e| e.to_string())?;
    Ok(AccessStatus {
        supported: true,
        enabled: identity.is_some() && remote.engine.trusted_listener_active(),
        configured: identity.is_some(),
        config: identity.map(|value| value.config),
        autostart: assi_access::autostart_enabled().map_err(|e| e.to_string())?,
        scope: "user_session",
    })
}
#[tauri::command]
pub async fn access_enable(
    app: tauri::AppHandle,
    remote: RemoteSession,
    config: AccessConfig,
) -> Result<String, String> {
    if !supported() {
        return Err("Постоянный доступ на этой платформе недоступен".into());
    }
    if config.allow_control && !assi_platform::Platform::capabilities().control {
        return Err("Удалённое управление на этой платформе недоступно".into());
    }
    config.validate().map_err(|e| e.to_string())?;
    let _guard = remote.lifecycle.lock().await;
    if remote.closed.load(std::sync::atomic::Ordering::Acquire) {
        return Err("Окно закрывается".into());
    }
    remote.reset().await?;
    let store = store(&app)?;
    let identity = store.enable(config.clone()).map_err(|e| e.to_string())?;
    match remote
        .engine
        .start_trusted_host(
            config.bind,
            config.advertised_ip,
            config.name,
            host_identity(identity),
            config.allow_control,
        )
        .await
    {
        Ok(invitation) => Ok(invitation),
        Err(error) => {
            store.revoke().map_err(|e| e.to_string())?;
            Err(error.to_string())
        }
    }
}
#[tauri::command]
pub async fn access_disable(app: tauri::AppHandle, remote: RemoteSession) -> Result<(), String> {
    let _guard = remote.lifecycle.lock().await;
    if remote.closed.load(std::sync::atomic::Ordering::Acquire) {
        return Err("Окно закрывается".into());
    }
    // Attempt both, even if disk removal or capture cleanup fails.
    let removal = store(&app).and_then(|store| store.revoke().map_err(|e| e.to_string()));
    let stop = remote.reset().await;
    removal.and(stop)
}
#[tauri::command]
pub fn access_invitation(app: tauri::AppHandle, _main: MainWindow) -> Result<String, String> {
    let identity = store(&app)?
        .load()
        .map_err(|e| e.to_string())?
        .ok_or("Постоянный доступ отключён")?;
    let endpoint = identity.config.endpoint();
    host_identity(identity)
        .invitation(endpoint)
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub fn access_autostart(enabled: bool, _main: MainWindow) -> Result<(), String> {
    if !supported() {
        return Err("Автозапуск на этой платформе недоступен".into());
    }
    let executable =
        std::env::current_exe().map_err(|_| "Не удалось определить путь приложения")?;
    assi_access::set_autostart(enabled, &executable).map_err(|e| e.to_string())
}
/// Call once at app startup, before the normal media pump. Only previously saved
/// explicit local grants are restored. The invitation is never logged.
pub async fn restore(app: &tauri::AppHandle, remote: &Arc<Remote>) -> Result<(), String> {
    if !supported() {
        return Ok(());
    }
    let _guard = remote.lifecycle.lock().await;
    if remote.closed.load(std::sync::atomic::Ordering::Acquire) {
        return Err("Окно закрывается".into());
    }
    let Some(identity) = store(app)?.load().map_err(|e| e.to_string())? else {
        return Ok(());
    };
    start_saved(remote, identity).await.map(|_| ())
}
#[tauri::command]
pub async fn access_resume(app: tauri::AppHandle, remote: RemoteSession) -> Result<String, String> {
    if !supported() {
        return Err("Постоянный доступ на этой платформе недоступен".into());
    }
    let _guard = remote.lifecycle.lock().await;
    if remote.closed.load(std::sync::atomic::Ordering::Acquire) {
        return Err("Окно закрывается".into());
    }
    let identity = store(&app)?
        .load()
        .map_err(|e| e.to_string())?
        .ok_or("Постоянный доступ не настроен")?;
    start_saved(&remote, identity).await
}
async fn start_saved(remote: &Remote, identity: AccessIdentity) -> Result<String, String> {
    let config = identity.config.clone();
    if config.allow_control && !assi_platform::Platform::capabilities().control {
        return Err("Сохранённое разрешение управления не поддерживается этой платформой".into());
    }
    remote.reset().await?;
    remote
        .engine
        .start_trusted_host(
            config.bind,
            config.advertised_ip,
            config.name,
            host_identity(identity),
            config.allow_control,
        )
        .await
        .map_err(|e| e.to_string())
}

pub(crate) fn publish_system_capabilities(
    engine: &assi_session::SessionEngine,
    request_id: &str,
) -> Result<(), String> {
    engine
        .set_system_capabilities(
            request_id.to_owned(),
            assi_session::SystemCapabilities {
                lock: assi_access::system_action_supported(assi_access::SystemAction::Lock),
                restart: assi_access::system_action_supported(assi_access::SystemAction::Restart),
            },
        )
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub fn session_system_snapshot(
    remote: RemoteSession,
    request_id: String,
) -> Result<assi_session::SystemSnapshot, String> {
    if remote.engine.snapshot().role == assi_session::Role::Host {
        publish_system_capabilities(&remote.engine, &request_id)?;
    }
    remote
        .engine
        .system_snapshot(request_id)
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub fn session_system_request(
    remote: RemoteSession,
    request_id: String,
    action: assi_session::SystemAction,
) -> Result<String, String> {
    remote
        .engine
        .request_system_action(request_id, action)
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn session_system_decide(
    remote: RemoteSession,
    request_id: String,
    id: String,
    approved: bool,
) -> Result<(), String> {
    let _guard = remote.lifecycle.lock().await;
    if remote.closed.load(std::sync::atomic::Ordering::Acquire) {
        return Err("Окно закрывается".into());
    }
    let action = remote
        .engine
        .decide_system_action(request_id.clone(), id.clone(), approved)
        .map_err(|e| e.to_string())?;
    let Some(action) = action else { return Ok(()) };
    let native = match action {
        assi_session::SystemAction::Lock => assi_access::SystemAction::Lock,
        assi_session::SystemAction::Restart => assi_access::SystemAction::Restart,
    };
    let result = match remote.release_held_inputs().await {
        Err(error) => Err(error),
        Ok(()) => {
            let executing = remote.inner().clone();
            let expected_request = request_id.clone();
            let expected_action = id.clone();
            tauri::async_runtime::spawn_blocking(move || {
                if executing.closed.load(std::sync::atomic::Ordering::Acquire) {
                    return Err("Окно закрывается".to_string());
                }
                let state = executing
                    .engine
                    .system_snapshot(expected_request)
                    .map_err(|error| error.to_string())?;
                if !state.latest.is_some_and(|result| {
                    result.id == expected_action
                        && result.state == assi_session::SystemRequestState::Approved
                }) {
                    return Err("Разрешение системного действия устарело".to_string());
                }
                assi_access::execute_system_action(native).map_err(|error| error.to_string())
            })
            .await
            .map_err(|_| "Не удалось выполнить системное действие".to_string())
            .and_then(|result| result)
        }
    };
    // OS acceptance is not proof that a reboot completed. A restart may close
    // the transport before this result is delivered; do not promise final ack.
    let completion = remote
        .engine
        .complete_system_action(request_id, id, result.is_ok())
        .map_err(|e| e.to_string());
    result.and(completion)
}
