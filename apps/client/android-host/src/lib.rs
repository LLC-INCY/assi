//! Android MediaProjection adapter. Only the native session consent boundary may
//! call request_capture. The plugin has no commands exposed to WebView JavaScript.
use tauri::{
    plugin::{Builder, TauriPlugin},
    Runtime,
};

#[cfg(target_os = "android")]
mod mobile {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::Deserialize;
    use tauri::{plugin::PluginHandle, AppHandle, Manager, Runtime};
    pub struct Capture<R: Runtime>(pub PluginHandle<R>);
    #[derive(Deserialize)]
    struct Frame {
        jpeg: Option<String>,
    }
    pub async fn request_capture<R: Runtime>(
        app: &AppHandle<R>,
        request_id: String,
    ) -> Result<(), String> {
        app.state::<Capture<R>>()
            .0
            .run_mobile_plugin_async::<serde_json::Value>(
                "requestCapture",
                serde_json::json!({"requestId": request_id}),
            )
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    pub async fn capture_jpeg<R: Runtime>(
        app: &AppHandle<R>,
        request_id: &str,
    ) -> Result<Option<Vec<u8>>, String> {
        let frame = app
            .state::<Capture<R>>()
            .0
            .run_mobile_plugin_async::<Frame>(
                "captureFrame",
                serde_json::json!({"requestId": request_id}),
            )
            .await
            .map_err(|e| e.to_string())?;
        let Some(jpeg) = frame.jpeg else {
            return Ok(None);
        };
        if jpeg.len() > 5_592_408 {
            return Err("Кадр экрана слишком велик".into());
        }
        let bytes = STANDARD
            .decode(jpeg)
            .map_err(|_| "Неверный формат кадра экрана")?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("Кадр экрана слишком велик".into());
        }
        Ok(Some(bytes))
    }
    pub async fn stop<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
        app.state::<Capture<R>>()
            .0
            .run_mobile_plugin_async::<serde_json::Value>("stopCapture", serde_json::json!({}))
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    pub async fn stop_request<R: Runtime>(
        app: &AppHandle<R>,
        request_id: &str,
    ) -> Result<(), String> {
        app.state::<Capture<R>>()
            .0
            .run_mobile_plugin_async::<serde_json::Value>(
                "stopCapture",
                serde_json::json!({"requestId": request_id}),
            )
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}
#[cfg(target_os = "android")]
pub use mobile::{capture_jpeg, request_capture, stop, stop_request};

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("assi-capture")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            {
                use tauri::Manager;
                let handle = api.register_android_plugin("app.assi.capture", "CapturePlugin")?;
                app.manage(mobile::Capture(handle));
            }
            #[cfg(not(target_os = "android"))]
            let _ = (app, api);
            Ok(())
        })
        .build()
}
