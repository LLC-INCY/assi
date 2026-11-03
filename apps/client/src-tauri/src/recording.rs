//! Local, explicitly started video recording. The renderer streams bounded chunks
//! to a private temporary file; only a successful finish exposes the final file.
use crate::remote::Remote;
use crate::sessions::{RecordingSession, RemoteSession};
use assi_session::SessionState;
use serde::Serialize;
use std::{
    io::Write,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};
#[cfg(not(target_os = "android"))]
use tauri::Manager;

const MAX_CHUNK: usize = 1024 * 1024;
const MAX_RECORDING: u64 = 512 * 1024 * 1024;
const MAX_DURATION: Duration = Duration::from_secs(30 * 60);
const FINAL_GRACE: Duration = Duration::from_secs(5);
const MAX_FINAL_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Default)]
pub struct RecordingState {
    active: Mutex<Option<Recording>>,
    #[cfg(not(target_os = "android"))]
    dialog: tokio::sync::Mutex<()>,
}
struct Recording {
    id: String,
    request: String,
    mime: String,
    destination: PathBuf,
    file: tempfile::NamedTempFile,
    bytes: u64,
    started: Instant,
    finalizing: Option<(Instant, u64)>,
}
#[derive(Serialize)]
pub struct RecordingStarted {
    id: String,
    mime: String,
}

fn extension(mime: &str) -> Result<&'static str, String> {
    match mime {
        "video/webm" | "video/webm;codecs=vp8" | "video/webm;codecs=vp9" => Ok("webm"),
        "video/mp4" | "video/mp4;codecs=avc1.42E01E" => Ok("mp4"),
        _ => Err("Этот формат записи не поддерживается".into()),
    }
}
fn require_active(remote: &Remote, request: &str) -> Result<(), String> {
    let snapshot = remote.recording_snapshot();
    if snapshot.state != SessionState::Active || snapshot.request_id.as_deref() != Some(request) {
        return Err("Запись доступна только в текущем сеансе".into());
    }
    Ok(())
}
fn matching<'a>(
    guard: &'a mut Option<Recording>,
    id: &str,
    request: &str,
) -> Result<&'a mut Recording, String> {
    guard
        .as_mut()
        .filter(|record| record.id == id && record.request == request)
        .ok_or_else(|| "Запись уже завершена или относится к другому сеансу".into())
}
impl RecordingState {
    pub(crate) fn cancel(&self) {
        if let Ok(mut guard) = self.active.lock() {
            guard.take();
        }
    }
}
impl Recording {
    #[cfg(any(not(target_os = "android"), test))]
    fn new(request: String, mime: String, destination: PathBuf) -> Result<Self, String> {
        let ext = extension(&mime)?;
        if destination
            .extension()
            .and_then(|value| value.to_str())
            .is_none_or(|value| !value.eq_ignore_ascii_case(ext))
        {
            return Err(format!("Выберите имя файла с расширением .{ext}"));
        }
        if destination.exists() {
            return Err("Файл уже существует. Выберите новое имя".into());
        }
        let parent = destination
            .parent()
            .ok_or("Не удалось определить папку записи")?;
        let file = tempfile::NamedTempFile::new_in(parent)
            .map_err(|_| "Не удалось создать файл записи")?;
        Ok(Self {
            id: uuid::Uuid::new_v4().to_string(),
            request,
            mime,
            destination,
            file,
            bytes: 0,
            started: Instant::now(),
            finalizing: None,
        })
    }
    fn begin_finish(&mut self) {
        if self.finalizing.is_none() {
            self.finalizing = Some((Instant::now(), self.bytes));
        }
    }
    fn append(&mut self, bytes: &[u8], active: bool) -> Result<(), String> {
        if bytes.is_empty() || bytes.len() > MAX_CHUNK {
            return Err("Блок записи превышает лимит".into());
        }
        // A peer can disconnect before the renderer receives its snapshot. Drain
        // only an already-started local file, under the same bounded final lease.
        if !active && self.bytes > 0 {
            self.begin_finish();
        }
        if let Some((started, original)) = self.finalizing {
            if started.elapsed() > FINAL_GRACE
                || self.bytes + bytes.len() as u64 - original > MAX_FINAL_BYTES
            {
                return Err("Истекло время завершения записи".into());
            }
        } else if !active || self.started.elapsed() > MAX_DURATION {
            return Err("Сеанс завершён или достигнут предел записи".into());
        }
        if self.bytes + bytes.len() as u64 > MAX_RECORDING {
            return Err("Достигнут предел записи: 512 МиБ".into());
        }
        if self.bytes == 0 {
            let valid = if extension(&self.mime)? == "webm" {
                bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3])
            } else {
                bytes.len() >= 12 && &bytes[4..8] == b"ftyp"
            };
            if !valid {
                return Err("Некорректный заголовок видео".into());
            }
        }
        self.file
            .write_all(bytes)
            .map_err(|_| "Не удалось записать видео. Проверьте свободное место")?;
        self.bytes += bytes.len() as u64;
        Ok(())
    }
    fn finish(mut self) -> Result<bool, String> {
        if self.bytes == 0 {
            return Err("В записи нет видеоданных".into());
        }
        self.file
            .flush()
            .map_err(|_| "Не удалось завершить запись")?;
        self.file
            .as_file()
            .sync_all()
            .map_err(|_| "Не удалось сохранить запись")?;
        self.file
            .persist_noclobber(self.destination)
            .map_err(|_| "Не удалось сохранить запись: файл уже существует или папка недоступна")?;
        Ok(true)
    }
}

#[tauri::command]
pub async fn recording_start(
    app: tauri::AppHandle,
    remote: RemoteSession,
    state: RecordingSession,
    request_id: String,
    mime: String,
) -> Result<Option<RecordingStarted>, String> {
    extension(&mime)?;
    require_active(&remote, &request_id)?;
    #[cfg(target_os = "android")]
    {
        let _ = (app, state);
        Err("Сохранение записи на Android пока недоступно".into())
    }
    #[cfg(not(target_os = "android"))]
    {
        use tauri_plugin_dialog::DialogExt;
        let _dialog = state
            .dialog
            .try_lock()
            .map_err(|_| "Окно сохранения уже открыто")?;
        if state.active.lock().map_err(|_| "Ошибка записи")?.is_some() {
            return Err("Запись уже идёт".into());
        }
        let ext = extension(&mime)?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let owner = app
            .get_webview_window(remote.label())
            .ok_or("Окно сеанса закрыто")?;
        app.dialog()
            .file()
            .set_parent(&owner)
            .set_title("Сохранить видео без звука под новым именем")
            .set_file_name(format!("Асси-сеанс.{ext}"))
            .add_filter("Видео", &[ext])
            .save_file(move |path| {
                let _ = tx.send(path);
            });
        let Some(path) = rx.await.map_err(|_| "Выбор места сохранения прерван")?
        else {
            return Ok(None);
        };
        require_active(&remote, &request_id)?;
        let path = path
            .into_path()
            .map_err(|_| "Неподдерживаемое место сохранения")?;
        let state = state.inner().clone();
        let remote = remote.inner().clone();
        tauri::async_runtime::spawn_blocking(move || {
            require_active(&remote, &request_id)?;
            let mut guard = state.active.lock().map_err(|_| "Ошибка записи")?;
            if guard.is_some() {
                return Err("Запись уже идёт".into());
            }
            let record = Recording::new(request_id, mime, path)?;
            let started = RecordingStarted {
                id: record.id.clone(),
                mime: record.mime.clone(),
            };
            *guard = Some(record);
            Ok(Some(started))
        })
        .await
        .map_err(|_| "Создание записи прервано")?
    }
}
#[tauri::command]
pub async fn recording_chunk(
    remote: RemoteSession,
    state: RecordingSession,
    request_id: String,
    recording_id: String,
    bytes: Vec<u8>,
) -> Result<(), String> {
    if bytes.is_empty() || bytes.len() > MAX_CHUNK {
        return Err("Блок записи превышает лимит".into());
    }
    let state = state.inner().clone();
    let remote = remote.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut guard = state.active.lock().map_err(|_| "Ошибка записи")?;
        let active = require_active(&remote, &request_id).is_ok();
        matching(&mut guard, &recording_id, &request_id)?.append(&bytes, active)
    })
    .await
    .map_err(|_| "Запись видеоблока прервана")?
}
#[tauri::command]
pub fn recording_prepare_finish(
    state: RecordingSession,
    request_id: String,
    recording_id: String,
) -> Result<(), String> {
    let mut guard = state.active.lock().map_err(|_| "Ошибка записи")?;
    matching(&mut guard, &recording_id, &request_id)?.begin_finish();
    Ok(())
}
#[tauri::command]
pub async fn recording_finish(
    state: RecordingSession,
    request_id: String,
    recording_id: String,
) -> Result<bool, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut guard = state.active.lock().map_err(|_| "Ошибка записи")?;
        matching(&mut guard, &recording_id, &request_id)?;
        let record = guard.take().ok_or("Запись уже завершена")?;
        drop(guard);
        record.finish()
    })
    .await
    .map_err(|_| "Завершение записи прервано")?
}
#[tauri::command]
pub fn recording_abort(
    state: RecordingSession,
    request_id: String,
    recording_id: String,
) -> Result<(), String> {
    let mut guard = state.active.lock().map_err(|_| "Ошибка записи")?;
    matching(&mut guard, &recording_id, &request_id)?;
    guard.take();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const WEBM: &[u8] = &[0x1a, 0x45, 0xdf, 0xa3, 0, 0, 0, 0];
    fn record(dir: &std::path::Path) -> Recording {
        Recording::new(
            "request".into(),
            "video/webm".into(),
            dir.join("record.webm"),
        )
        .unwrap()
    }
    #[test]
    fn streams_to_temporary_file_and_publishes_only_after_finish() {
        let dir = tempfile::tempdir().unwrap();
        let mut recording = record(dir.path());
        recording.append(WEBM, true).unwrap();
        recording.append(&[1, 2, 3], true).unwrap();
        assert!(!dir.path().join("record.webm").exists());
        recording.finish().unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("record.webm")).unwrap(),
            [WEBM, &[1, 2, 3]].concat()
        );
    }
    #[test]
    fn never_overwrites_destination_created_during_recording() {
        let dir = tempfile::tempdir().unwrap();
        let mut recording = record(dir.path());
        recording.append(WEBM, true).unwrap();
        std::fs::write(dir.path().join("record.webm"), b"original").unwrap();
        assert!(recording.finish().is_err());
        assert_eq!(
            std::fs::read(dir.path().join("record.webm")).unwrap(),
            b"original"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[test]
    fn abort_drops_private_partial_file_and_wrong_handle_does_not_mutate() {
        let dir = tempfile::tempdir().unwrap();
        let mut active = Some(record(dir.path()));
        assert!(matching(&mut active, "wrong", "request").is_err());
        assert!(active.is_some());
        drop(active.take());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
    #[test]
    fn validates_container_size_and_session_before_write() {
        let dir = tempfile::tempdir().unwrap();
        let mut recording = record(dir.path());
        assert!(recording.append(b"not a video", true).is_err());
        assert!(recording.append(WEBM, false).is_err());
        assert!(recording.append(&vec![0; MAX_CHUNK + 1], true).is_err());
        assert_eq!(recording.bytes, 0);
        recording.append(WEBM, true).unwrap();
        recording.bytes = MAX_RECORDING;
        assert!(recording.append(&[1], true).is_err());
    }
    #[test]
    fn final_tail_after_disconnect_is_bounded_and_not_a_new_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut recording = record(dir.path());
        recording.append(WEBM, true).unwrap();
        recording.begin_finish();
        recording.append(&[1, 2, 3], false).unwrap();
        recording.finalizing = Some((
            Instant::now() - FINAL_GRACE - Duration::from_secs(1),
            recording.bytes,
        ));
        assert!(recording.append(&[4], false).is_err());
    }
    #[test]
    fn duration_and_mime_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let mut recording = record(dir.path());
        recording.started = Instant::now() - MAX_DURATION - Duration::from_secs(1);
        assert!(recording.append(WEBM, true).is_err());
        assert!(extension("text/html").is_err());
        assert!(Recording::new("r".into(), "video/webm".into(), dir.path().join("x.exe")).is_err());
    }
}
