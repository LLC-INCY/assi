//! Read-only host annotation surface. Session consent and enabled state remain in Remote.
//! Call sync/hide under Remote's lifecycle lock; never use this window for input.

#[cfg(any(target_os = "macos", target_os = "windows"))]
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

pub const LABEL: &str = "annotations-overlay";

pub fn supported() -> bool {
    // Current Linux capture has no reliable exclusion for our own overlay.
    // Android needs a separately consented native overlay, not a WebviewWindow.
    cfg!(any(target_os = "macos", target_os = "windows"))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn sync(app: &tauri::AppHandle, active: bool) -> Result<(), String> {
    if !active {
        return hide(app);
    }
    let monitor = app
        .primary_monitor()
        .map_err(|e| e.to_string())?
        .ok_or("Основной монитор недоступен для пометок")?;
    let position = *monitor.position();
    let size = *monitor.size();
    if size.width == 0 || size.height == 0 {
        hide(app)?;
        return Err("Размер основного монитора недоступен".into());
    }
    let window = if let Some(window) = app.get_webview_window(LABEL) {
        window
    } else {
        let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("overlay.html".into()))
            .title("Асси — общие пометки")
            .visible(false)
            .focused(false)
            .focusable(false)
            .decorations(false)
            .resizable(false)
            .shadow(false)
            .transparent(true)
            .always_on_top(true)
            .visible_on_all_workspaces(true)
            .skip_taskbar(true)
            .content_protected(true)
            .build()
            .map_err(|e| e.to_string())?;
        // Configure before first show: no transient interception of local clicks.
        if let Err(error) = window.set_ignore_cursor_events(true) {
            let _ = window.destroy();
            return Err(error.to_string());
        }
        window
    };
    // Tauri's physical monitor coordinates and sizes account for display scaling.
    // xcap captures the primary display; no window-based or multi-monitor capture yet.
    let result = (|| -> tauri::Result<()> {
        if window.outer_position()? != position {
            window.set_position(position)?;
        }
        if window.inner_size()? != size {
            window.set_size(size)?;
        }
        if !window.is_visible()? {
            window.show()?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = window.hide();
        return Err(error.to_string());
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn hide(app: &tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(LABEL) {
        // Destroy instead of merely hiding so cached marks cannot flash on a new
        // session before the first IPC poll. destroy bypasses main close consent.
        window.destroy().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn sync(_app: &tauri::AppHandle, _active: bool) -> Result<(), String> {
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn hide(_app: &tauri::AppHandle) -> Result<(), String> {
    Ok(())
}
