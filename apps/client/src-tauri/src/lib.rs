use assi_core::{Device, DeviceInput, Settings, Snapshot, Store, StoreError};
use serde::Serialize;
use std::sync::Arc;
mod access;
mod file_queue;
mod ipc_catalog;
mod overlay;
mod recording;
mod remote;
mod sessions;
mod storage;
use storage::Storage;
use tauri::{Manager, State};

struct AppStore(Arc<Storage>);
#[derive(Serialize)]
struct AppSnapshot {
    data: Snapshot,
    platform: &'static str,
    version: &'static str,
}
fn platform() -> &'static str {
    if cfg!(target_os = "android") {
        "Android"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else {
        "Linux"
    }
}
fn client_error(error: StoreError) -> String {
    // Keep filesystem paths and SQLite diagnostics out of the webview.
    match error {
        StoreError::Database(_) => "Не удалось сохранить или прочитать данные. Проверьте свободное место и перезапустите приложение.".into(),
        other => other.to_string(),
    }
}
async fn with_store<T: Send + 'static>(
    state: State<'_, AppStore>,
    f: impl FnOnce(&mut Store) -> Result<T, StoreError> + Send + 'static,
) -> Result<T, String> {
    let shared = Arc::clone(&state.0);
    tauri::async_runtime::spawn_blocking(move || {
        let name = match platform() {
            "macOS" => "Этот Mac",
            "Android" => "Этот телефон",
            _ => "Этот компьютер",
        };
        shared.access(name, f)
    })
    .await
    .map_err(|_| "Операция хранилища прервана. Перезапустите приложение.".to_string())?
}
#[tauri::command]
async fn load_snapshot(state: State<'_, AppStore>) -> Result<AppSnapshot, String> {
    with_store(state, |store| {
        Ok(AppSnapshot {
            data: store.snapshot()?,
            platform: platform(),
            version: env!("CARGO_PKG_VERSION"),
        })
    })
    .await
}
#[tauri::command]
async fn add_device(
    _main: sessions::MainWindow,
    state: State<'_, AppStore>,
    input: DeviceInput,
) -> Result<Device, String> {
    with_store(state, move |s| s.add_device(input)).await
}
#[tauri::command]
async fn remove_device(
    _main: sessions::MainWindow,
    state: State<'_, AppStore>,
    id: String,
) -> Result<(), String> {
    with_store(state, move |s| s.remove_device(&id)).await
}
#[tauri::command]
async fn save_settings(
    _main: sessions::MainWindow,
    state: State<'_, AppStore>,
    settings: Settings,
) -> Result<Settings, String> {
    with_store(state, move |s| s.update_settings(settings)).await
}

pub(crate) fn app_context<R: tauri::Runtime>() -> tauri::Context<R> {
    tauri::generate_context!()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(target_os = "android")]
    let builder = builder.plugin(assi_capture::init());
    #[cfg(not(target_os = "android"))]
    let builder = builder.plugin(tauri_plugin_dialog::init());
    let result = builder
        .setup(|app| {
            let path = app
                .path()
                .app_data_dir()
                .map(|dir| dir.join("assi.sqlite3"))
                .map_err(|_| {
                    "Не удалось определить папку данных приложения. Перезапустите Асси.".to_string()
                });
            app.manage(AppStore(Arc::new(Storage::new(path))));
            let remote = remote::Remote::new(Some(app.handle().clone()));
            app.manage(Arc::new(sessions::SessionRegistry::with_main(
                remote.clone(),
            )));
            let restoring = remote.clone();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = access::restore(&handle, &restoring).await {
                    restoring.report_error(error);
                }
                restoring.pump().await;
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "annotations-overlay" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let registry = window
                    .state::<Arc<sessions::SessionRegistry>>()
                    .inner()
                    .clone();
                let window = window.clone();
                tauri::async_runtime::spawn(async move {
                    if registry.close(window.label()).await.is_err() {
                        eprintln!("Session cleanup failed while closing window");
                    }
                    if window.label() == "main" {
                        if let Some(overlay) = window
                            .app_handle()
                            .get_webview_window(crate::overlay::LABEL)
                        {
                            let _ = overlay.destroy();
                        }
                    }
                    let _ = window.destroy();
                });
            }
        })
        .invoke_handler(tauri::generate_handler![
            sessions::session_windows,
            sessions::session_window_open,
            sessions::session_window_focus,
            sessions::session_window_close,
            load_snapshot,
            add_device,
            remove_device,
            save_settings,
            recording::recording_start,
            recording::recording_chunk,
            recording::recording_prepare_finish,
            recording::recording_finish,
            recording::recording_abort,
            access::session_system_snapshot,
            access::session_system_request,
            access::session_system_decide,
            access::access_status,
            access::access_enable,
            access::access_resume,
            access::access_disable,
            access::access_invitation,
            access::access_autostart,
            remote::session_video,
            remote::session_video_codec,
            remote::session_video_keyframe,
            remote::session_screen_ack,
            remote::session_display,
            remote::session_clipboard_enable,
            remote::session_clipboard_send,
            remote::session_clipboard_read,
            remote::session_chat,
            remote::session_chat_send,
            remote::session_terminal_request,
            remote::session_terminal_allow,
            remote::session_terminal_input,
            remote::session_terminal_resize,
            remote::session_terminal_read,
            remote::session_terminal_close,
            remote::session_snapshot,
            remote::session_audio,
            remote::session_audio_enable,
            remote::session_capture_options,
            remote::session_capture_set,
            remote::session_save_image,
            remote::session_annotations,
            remote::session_annotate,
            remote::session_overlay_snapshot,
            remote::session_overlay_enable,
            remote::session_fullscreen,
            remote::session_host,
            remote::session_connect,
            remote::session_accept,
            remote::session_reject,
            remote::session_stop,
            remote::session_frame,
            remote::session_input,
            remote::session_host_relay,
            remote::session_file_offer_folder,
            remote::session_file_accept_folder,
            remote::session_file_folder_status,
            remote::session_file_pause,
            remote::session_file_resume,
            remote::session_file_offer,
            remote::session_file_queue,
            remote::session_file_queue_remove,
            remote::session_file_accept,
            remote::session_file_reject,
            remote::session_file_cancel,
        ])
        .build(app_context());
    let app = match result {
        Ok(app) => app,
        Err(error) => {
            eprintln!("Асси не удалось запустить: {error}");
            std::process::exit(1);
        }
    };
    // Cmd+Q / application quit can bypass the window CloseRequested event.
    let exit_phase = Arc::new(std::sync::atomic::AtomicU8::new(0));
    app.run(move |handle, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            use std::sync::atomic::Ordering;
            if exit_phase.load(Ordering::SeqCst) == 2 {
                return;
            }
            api.prevent_exit();
            if exit_phase
                .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                let phase = exit_phase.clone();
                let registry = handle
                    .state::<Arc<sessions::SessionRegistry>>()
                    .inner()
                    .clone();
                let handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    registry.shutdown_all().await;
                    phase.store(2, Ordering::SeqCst);
                    handle.exit(0);
                });
            }
        }
    });
}
