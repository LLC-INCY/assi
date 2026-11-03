//! Native boundary for consented session tools. Peer messages never grant OS access.
use super::*;
#[tauri::command]
pub async fn session_clipboard_enable(
    remote: crate::sessions::RemoteSession,
    request_id: String,
    enabled: bool,
) -> Result<(), String> {
    remote
        .engine
        .set_clipboard_enabled(request_id, enabled)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn session_clipboard_send(
    remote: crate::sessions::RemoteSession,
    request_id: String,
    text: String,
) -> Result<(), String> {
    remote
        .engine
        .send_clipboard(request_id, text)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub fn session_clipboard_read(
    remote: crate::sessions::RemoteSession,
    request_id: String,
) -> Result<assi_session::ClipboardBatch, String> {
    remote
        .engine
        .take_clipboard(request_id)
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub fn session_chat(
    remote: crate::sessions::RemoteSession,
    request_id: String,
) -> Result<assi_session::ChatSnapshot, String> {
    remote.engine.chat(request_id).map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn session_chat_send(
    remote: crate::sessions::RemoteSession,
    request_id: String,
    text: String,
) -> Result<(), String> {
    remote
        .engine
        .send_chat(request_id, text)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub fn session_terminal_request(
    remote: crate::sessions::RemoteSession,
    request_id: String,
) -> Result<(), String> {
    remote
        .engine
        .terminal_request(request_id)
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn session_terminal_allow(
    remote: crate::sessions::RemoteSession,
    request_id: String,
    enabled: bool,
) -> Result<(), String> {
    let _guard = remote.lifecycle.lock().await;
    let now = remote.engine.snapshot();
    if now.state != SessionState::Active
        || now.role != Role::Host
        || now.request_id.as_deref() != Some(&request_id)
    {
        return Err("Нет активного запроса".into());
    }
    if enabled {
        if !remote
            .engine
            .terminal_snapshot(request_id.clone())
            .map_err(|e| e.to_string())?
            .requested
        {
            return Err("Сначала нужен запрос терминала".into());
        }
        remote
            .terminal
            .lock()
            .map_err(|_| "Ошибка терминала")?
            .start_approved(&request_id, 100, 30)
            .map_err(|e| e.to_string())?;
    } else {
        remote
            .terminal
            .lock()
            .map_err(|_| "Ошибка терминала")?
            .stop();
    }
    let result = remote
        .engine
        .terminal_allow(request_id, enabled)
        .map_err(|e| e.to_string());
    if result.is_err() {
        if let Ok(mut terminal) = remote.terminal.lock() {
            terminal.stop();
        }
    }
    result
}
#[tauri::command]
pub fn session_terminal_input(
    remote: crate::sessions::RemoteSession,
    request_id: String,
    bytes: Vec<u8>,
) -> Result<(), String> {
    remote
        .engine
        .terminal_input(request_id, bytes)
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub fn session_terminal_resize(
    remote: crate::sessions::RemoteSession,
    request_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    remote
        .engine
        .terminal_resize(request_id, cols, rows)
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub fn session_terminal_read(
    remote: crate::sessions::RemoteSession,
    request_id: String,
) -> Result<assi_session::TerminalSnapshot, String> {
    remote
        .engine
        .terminal_snapshot(request_id)
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn session_terminal_close(
    remote: crate::sessions::RemoteSession,
    request_id: String,
) -> Result<(), String> {
    let _guard = remote.lifecycle.lock().await;
    if remote.engine.snapshot().request_id.as_deref() != Some(&request_id) {
        return Err("Сеанс изменился".into());
    }
    if let Ok(mut terminal) = remote.terminal.lock() {
        terminal.stop();
    }
    remote
        .engine
        .terminal_close(request_id)
        .map_err(|e| e.to_string())
}
impl Remote {
    pub(super) async fn terminal_tick(&self, snapshot: &Snapshot) {
        let Some(id) = snapshot.request_id.clone() else {
            return;
        };
        let Ok(state) = self.engine.terminal_snapshot(id.clone()) else {
            return;
        };
        if !state.enabled {
            if let Ok(mut terminal) = self.terminal.lock() {
                terminal.stop();
            }
            return;
        }
        let outcome = (|| -> Result<(), String> {
            let mut terminal = self.terminal.lock().map_err(|_| "Ошибка терминала")?;
            for command in self
                .engine
                .take_terminal_commands(id.clone())
                .map_err(|e| e.to_string())?
            {
                let current = self.engine.snapshot();
                let permission = self
                    .engine
                    .terminal_snapshot(id.clone())
                    .map_err(|e| e.to_string())?;
                if current.state != SessionState::Active
                    || current.role != Role::Host
                    || current.request_id.as_deref() != Some(&id)
                    || !permission.enabled
                    || permission.epoch != state.epoch
                {
                    terminal.stop();
                    return Ok(());
                }
                match command {
                    assi_session::TerminalCommand::Input(bytes) => {
                        terminal.write(&id, bytes).map_err(|e| e.to_string())?
                    }
                    assi_session::TerminalCommand::Resize { cols, rows } => terminal
                        .resize(&id, cols, rows)
                        .map_err(|e| e.to_string())?,
                    assi_session::TerminalCommand::Close => {
                        terminal.stop();
                        return Ok(());
                    }
                }
            }
            let batch = terminal.drain(&id).map_err(|e| e.to_string())?;
            for chunk in batch.bytes.chunks(assi_session::TERMINAL_CHUNK) {
                self.engine
                    .terminal_publish(id.clone(), chunk.to_vec())
                    .map_err(|e| e.to_string())?;
            }
            if batch.closed || batch.error {
                terminal.stop();
                self.engine
                    .terminal_close(id.clone())
                    .map_err(|e| e.to_string())?;
            }
            Ok(())
        })();
        if let Err(error) = outcome {
            if let Ok(mut terminal) = self.terminal.lock() {
                terminal.stop();
            }
            let _ = self.engine.terminal_allow(id, false);
            *self.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
        }
    }
}
#[tauri::command]
pub fn session_file_queue(
    remote: crate::sessions::RemoteSession,
    request_id: String,
) -> Result<Vec<crate::file_queue::QueueEntry>, String> {
    let now = remote.engine.snapshot();
    if now.state != SessionState::Active || now.request_id.as_deref() != Some(&request_id) {
        return Err("Сеанс изменился".into());
    }
    Ok(remote
        .file_queue
        .lock()
        .map_err(|_| "Ошибка очереди")?
        .snapshot(&request_id))
}
#[tauri::command]
pub fn session_file_queue_remove(
    remote: crate::sessions::RemoteSession,
    request_id: String,
    id: u64,
) -> Result<(), String> {
    remote
        .file_queue
        .lock()
        .map_err(|_| "Ошибка очереди")?
        .remove(&request_id, id)
}
impl Remote {
    pub(super) async fn file_queue_tick(&self, snapshot: &Snapshot) {
        let Some(id) = snapshot.request_id.clone() else {
            return;
        };
        if let Ok(mut queue) = self.file_queue.lock() {
            queue.cleanup_previous(&id);
        }
        let next = self
            .file_queue
            .lock()
            .ok()
            .and_then(|mut q| q.next(&id, snapshot.transfer.as_ref()));
        if let Some((entry, path)) = next {
            let valid = std::fs::symlink_metadata(&path)
                .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink());
            let result = if valid {
                self.engine.offer_file(id, path).await
            } else {
                Err(assi_session::Error::InvalidFile)
            };
            if let Ok(mut queue) = self.file_queue.lock() {
                match result {
                    Ok(transfer) => queue.started(entry, transfer),
                    Err(_) => queue.failed(entry),
                }
            }
        }
    }
}
