//! A single explicitly accepted transfer, with bounded chunks and no-clobber publication.
use crate::{
    wire::{Control, Packet},
    Error, Result,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::digest::{Context, SHA256};
use serde::Serialize;
use std::{
    collections::VecDeque,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime},
};
use tempfile::NamedTempFile;
use uuid::Uuid;
pub const FILE_CHUNK: usize = 64 * 1024;
pub const MAX_FILE_SIZE: u64 = 1024 * 1024 * 1024;
const OFFER_TIMEOUT: Duration = Duration::from_secs(60);
const PROGRESS_TIMEOUT: Duration = Duration::from_secs(30);
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferDirection {
    Sending,
    Receiving,
}
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    Offered,
    Transferring,
    Paused,
    Completed,
    Rejected,
    Cancelled,
    Error,
}
#[derive(Debug, Clone, Serialize)]
pub struct TransferSnapshot {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub transferred: u64,
    pub direction: TransferDirection,
    pub state: TransferState,
    pub error: Option<String>,
    pub pause_ready: bool,
    pub resume_requested: bool,
}
impl TransferSnapshot {
    fn busy(&self) -> bool {
        matches!(
            self.state,
            TransferState::Offered | TransferState::Transferring | TransferState::Paused
        )
    }
}
struct Transfer {
    view: TransferSnapshot,
    source: Option<File>,
    destination: Option<PathBuf>,
    temporary: Option<NamedTempFile>,
    digest: Context,
    last_progress: Instant,
    awaiting_ack: bool,
    resume_waiting: bool,
    checkpoint: Option<(u64, String)>,
    source_modified: Option<SystemTime>,
}
impl Transfer {
    fn new(id: String, name: String, size: u64, direction: TransferDirection) -> Self {
        Self {
            view: TransferSnapshot {
                id,
                name,
                size,
                transferred: 0,
                direction,
                state: TransferState::Offered,
                error: None,
                pause_ready: false,
                resume_requested: false,
            },
            source: None,
            destination: None,
            temporary: None,
            digest: Context::new(&SHA256),
            last_progress: Instant::now(),
            awaiting_ack: false,
            resume_waiting: false,
            checkpoint: None,
            source_modified: None,
        }
    }
    fn finish(&mut self, state: TransferState, error: Option<String>) {
        self.view.state = state;
        self.view.error = error;
        self.view.pause_ready = false;
        self.view.resume_requested = false;
        self.resume_waiting = false;
        self.checkpoint = None;
        self.source = None;
        self.temporary = None;
        self.destination = None;
    }
}
pub(crate) struct FileManager {
    active: Arc<AtomicBool>,
    current: Option<Transfer>,
    outgoing: VecDeque<Control>,
    retired: VecDeque<String>,
    #[cfg(test)]
    before_commit: Option<Box<dyn FnOnce() + Send>>,
}
pub(crate) struct FileHandle {
    active: Arc<AtomicBool>,
    manager: Mutex<FileManager>,
    view: Mutex<Option<TransferSnapshot>>,
}
impl FileHandle {
    pub(crate) fn new() -> Arc<Self> {
        let manager = FileManager::new();
        Arc::new(Self {
            active: manager.active.clone(),
            manager: Mutex::new(manager),
            view: Mutex::new(None),
        })
    }
    pub(crate) fn snapshot(&self) -> Option<TransferSnapshot> {
        self.view.lock().ok()?.clone()
    }
    pub(crate) fn stop(&self) {
        self.active.store(false, Ordering::SeqCst);
    }
    pub(crate) fn cleanup(&self) -> Result<()> {
        self.run(|manager| {
            manager.deactivate();
            Ok(())
        })
    }
    pub(crate) fn run<T>(&self, op: impl FnOnce(&mut FileManager) -> Result<T>) -> Result<T> {
        let mut manager = self.manager.lock().map_err(|_| Error::State)?;
        let result = op(&mut manager);
        *self.view.lock().map_err(|_| Error::State)? = manager.snapshot();
        result
    }
}

impl FileManager {
    pub(crate) fn new() -> Self {
        Self {
            active: Arc::new(AtomicBool::new(true)),
            current: None,
            outgoing: VecDeque::new(),
            retired: VecDeque::new(),
            #[cfg(test)]
            before_commit: None,
        }
    }
    pub(crate) fn snapshot(&self) -> Option<TransferSnapshot> {
        self.current.as_ref().map(|t| t.view.clone())
    }
    pub(crate) fn deactivate(&mut self) {
        self.active.store(false, Ordering::SeqCst);
        self.outgoing.clear();
        if let Some(t) = self.current.as_mut() {
            if t.view.busy() {
                t.finish(TransferState::Cancelled, None);
            }
        }
    }
    fn retire_current(&mut self) {
        if let Some(t) = &self.current {
            if !t.view.busy() {
                self.retired.push_back(t.view.id.clone());
                if self.retired.len() > 16 {
                    self.retired.pop_front();
                }
            }
        }
    }
    fn ensure_active(&self) -> Result<()> {
        if self.active.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(Error::InvalidState)
        }
    }
    fn ensure_available(&self) -> Result<()> {
        self.ensure_active()?;
        if self.current.as_ref().is_some_and(|t| t.view.busy()) {
            Err(Error::TransferBusy)
        } else {
            Ok(())
        }
    }
    fn enqueue(&mut self, message: Control) -> Result<()> {
        if self.outgoing.len() >= 16 {
            return Err(Error::Backpressure);
        }
        self.outgoing.push_back(message);
        Ok(())
    }
    pub(crate) fn offer(&mut self, path: PathBuf) -> Result<String> {
        self.ensure_available()?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(Error::InvalidFile)?
            .to_string();
        validate_name(&name)?;
        let file = File::open(path).map_err(|_| Error::FileIo)?;
        let metadata = file.metadata().map_err(|_| Error::FileIo)?;
        if !metadata.is_file() || metadata.len() > MAX_FILE_SIZE {
            return Err(Error::InvalidFile);
        }
        let id = Uuid::new_v4().to_string();
        let mut transfer = Transfer::new(
            id.clone(),
            name.clone(),
            metadata.len(),
            TransferDirection::Sending,
        );
        transfer.source_modified = metadata.modified().ok();
        transfer.source = Some(file);
        self.enqueue(Control::FileOffer {
            id: id.clone(),
            name,
            size: metadata.len(),
        })?;
        self.retire_current();
        self.current = Some(transfer);
        Ok(id)
    }
    pub(crate) fn accept(&mut self, id: &str, destination: PathBuf) -> Result<()> {
        self.ensure_active()?;
        let t = self
            .current
            .as_mut()
            .filter(|t| {
                t.view.id == id
                    && t.view.direction == TransferDirection::Receiving
                    && t.view.state == TransferState::Offered
            })
            .ok_or(Error::InvalidState)?;
        if t.last_progress.elapsed() >= OFFER_TIMEOUT {
            return Err(Error::Timeout);
        }
        if destination.symlink_metadata().is_ok() {
            return Err(Error::DestinationExists);
        }
        if destination.file_name().is_none() {
            return Err(Error::InvalidFile);
        }
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let temporary = NamedTempFile::new_in(parent).map_err(|_| Error::FileIo)?;
        t.temporary = Some(temporary);
        t.destination = Some(destination);
        t.view.state = TransferState::Transferring;
        t.last_progress = Instant::now();
        self.enqueue(Control::FileAccept { id: id.into() })
    }
    pub(crate) fn reject(&mut self, id: &str) -> Result<()> {
        self.ensure_active()?;
        let t = self
            .current
            .as_mut()
            .filter(|t| {
                t.view.id == id
                    && t.view.direction == TransferDirection::Receiving
                    && t.view.state == TransferState::Offered
            })
            .ok_or(Error::InvalidState)?;
        t.finish(TransferState::Rejected, None);
        self.enqueue(Control::FileReject { id: id.into() })
    }
    pub(crate) fn cancel(&mut self, id: &str) -> Result<()> {
        self.ensure_active()?;
        let t = self
            .current
            .as_mut()
            .filter(|t| t.view.id == id && t.view.busy())
            .ok_or(Error::InvalidState)?;
        t.finish(TransferState::Cancelled, None);
        self.enqueue(Control::FileCancel { id: id.into() })
    }
    pub(crate) fn pause(&mut self, id: &str) -> Result<()> {
        self.ensure_active()?;
        let t = self
            .current
            .as_mut()
            .filter(|t| t.view.id == id)
            .ok_or(Error::InvalidState)?;
        if t.view.state == TransferState::Paused {
            return Ok(());
        }
        if t.view.state != TransferState::Transferring || t.awaiting_ack {
            return Err(Error::InvalidState);
        }
        t.view.state = TransferState::Paused;
        t.view.pause_ready = false;
        t.view.resume_requested = false;
        t.resume_waiting = false;
        t.checkpoint = None;
        t.last_progress = Instant::now();
        self.enqueue(Control::FilePause { id: id.into() })
    }
    /// Sender asks; receiver explicitly approves. Only the existing accepted
    /// destination and authenticated live session can be resumed.
    pub(crate) fn resume(&mut self, id: &str) -> Result<()> {
        self.ensure_active()?;
        let active = self.active.clone();
        let t = self
            .current
            .as_mut()
            .filter(|t| {
                t.view.id == id && t.view.state == TransferState::Paused && t.view.pause_ready
            })
            .ok_or(Error::InvalidState)?;
        if t.view.direction == TransferDirection::Sending {
            if t.view.resume_requested {
                return Ok(());
            }
            t.view.resume_requested = true;
            return self.enqueue(Control::FileResumeRequest { id: id.into() });
        }
        if t.resume_waiting {
            return Ok(());
        }
        let result = (|| {
            let temporary = t.temporary.as_mut().ok_or(Error::InvalidState)?;
            if temporary
                .as_file()
                .metadata()
                .map_err(|_| Error::FileIo)?
                .len()
                != t.view.transferred
            {
                return Err(Error::FileIntegrity);
            }
            let digest = prefix_digest(temporary.as_file_mut(), t.view.transferred, &active)?;
            let sha256 = URL_SAFE_NO_PAD.encode(digest.clone().finish().as_ref());
            if digest.clone().finish().as_ref() != t.digest.clone().finish().as_ref() {
                return Err(Error::FileIntegrity);
            }
            Ok(sha256)
        })();
        match result {
            Ok(sha256) => {
                let offset = t.view.transferred;
                t.checkpoint = Some((offset, sha256.clone()));
                t.resume_waiting = true;
                t.view.resume_requested = true;
                t.last_progress = Instant::now();
                self.enqueue(Control::FileResume {
                    id: id.into(),
                    offset,
                    sha256,
                })
            }
            Err(error) => {
                t.finish(TransferState::Error, Some(error.to_string()));
                self.enqueue(Control::FileCancel { id: id.into() })?;
                Err(error)
            }
        }
    }
    pub(crate) fn receive(&mut self, packet: Packet) -> Result<()> {
        self.ensure_active()?;
        let incoming_id = match &packet {
            Packet::FileChunk { id, .. }
            | Packet::Control(
                Control::FileAccept { id }
                | Control::FileReject { id }
                | Control::FileCancel { id }
                | Control::FileDone { id, .. }
                | Control::FileComplete { id }
                | Control::FilePause { id }
                | Control::FilePaused { id, .. }
                | Control::FileResumeRequest { id }
                | Control::FileResume { id, .. }
                | Control::FileResumed { id, .. },
            ) => Some(id),
            _ => None,
        };
        if matches!(
            &packet,
            Packet::Control(
                Control::FilePause { .. }
                    | Control::FilePaused { .. }
                    | Control::FileResumeRequest { .. }
                    | Control::FileResume { .. }
                    | Control::FileResumed { .. }
            )
        ) && incoming_id.is_some_and(|id| {
            self.current
                .as_ref()
                .is_some_and(|t| &t.view.id == id && t.view.state == TransferState::Completed)
        }) {
            return Ok(());
        }
        if incoming_id.is_some_and(|id| {
            self.retired.contains(id)
                || self.current.as_ref().is_some_and(|t| {
                    &t.view.id == id
                        && matches!(
                            t.view.state,
                            TransferState::Cancelled
                                | TransferState::Rejected
                                | TransferState::Error
                        )
                })
        }) {
            return Ok(());
        }
        match packet {
            Packet::Control(Control::FileOffer { id, name, size }) => {
                valid_id(&id)?;
                validate_name(&name)?;
                if size > MAX_FILE_SIZE {
                    return Err(Error::InvalidFile);
                }
                if self.current.as_ref().is_some_and(|t| t.view.busy()) {
                    return self.enqueue(Control::FileReject { id });
                }
                self.retire_current();
                self.current = Some(Transfer::new(id, name, size, TransferDirection::Receiving));
            }
            Packet::Control(Control::FileAccept { id }) => {
                let t = self
                    .current
                    .as_mut()
                    .filter(|t| {
                        t.view.id == id
                            && t.view.direction == TransferDirection::Sending
                            && t.view.state == TransferState::Offered
                    })
                    .ok_or(Error::Protocol)?;
                if t.last_progress.elapsed() >= OFFER_TIMEOUT {
                    return Err(Error::Timeout);
                }
                t.view.state = TransferState::Transferring;
                t.last_progress = Instant::now();
            }
            Packet::Control(Control::FileReject { id }) => {
                if let Some(t) = self
                    .current
                    .as_mut()
                    .filter(|t| t.view.id == id && t.view.busy())
                {
                    t.finish(TransferState::Rejected, None);
                }
            }
            Packet::Control(Control::FileCancel { id }) => {
                if let Some(t) = self
                    .current
                    .as_mut()
                    .filter(|t| t.view.id == id && t.view.busy())
                {
                    t.finish(TransferState::Cancelled, None);
                }
            }
            Packet::Control(Control::FilePause { id }) => {
                let t = self
                    .current
                    .as_mut()
                    .filter(|t| {
                        t.view.id == id
                            && matches!(
                                t.view.state,
                                TransferState::Transferring | TransferState::Paused
                            )
                    })
                    .ok_or(Error::Protocol)?;
                if t.view.direction == TransferDirection::Sending {
                    // FileDone already precedes this request on the ordered wire.
                    // Let completion win rather than resurrecting a completed file.
                    if t.awaiting_ack {
                        return Ok(());
                    }
                    t.view.state = TransferState::Paused;
                    t.view.pause_ready = false;
                    t.view.resume_requested = false;
                    t.resume_waiting = false;
                    t.checkpoint = None;
                    t.last_progress = Instant::now();
                    self.enqueue(Control::FilePause { id })?;
                } else {
                    t.view.state = TransferState::Paused;
                    t.view.pause_ready = true;
                    t.last_progress = Instant::now();
                    let offset = t.view.transferred;
                    let sha256 = URL_SAFE_NO_PAD.encode(t.digest.clone().finish().as_ref());
                    t.checkpoint = Some((offset, sha256.clone()));
                    self.enqueue(Control::FilePaused { id, offset, sha256 })?;
                }
            }
            Packet::Control(Control::FilePaused { id, offset, sha256 }) => {
                let t = self
                    .current
                    .as_mut()
                    .filter(|t| {
                        t.view.id == id
                            && t.view.direction == TransferDirection::Sending
                            && t.view.state == TransferState::Paused
                    })
                    .ok_or(Error::Protocol)?;
                valid_checkpoint(offset, &sha256, t.view.size)?;
                if offset != t.view.transferred {
                    return Err(Error::InvalidFile);
                }
                t.checkpoint = Some((offset, sha256));
                t.view.pause_ready = true;
                t.last_progress = Instant::now();
            }
            Packet::Control(Control::FileResumeRequest { id }) => {
                let t = self
                    .current
                    .as_mut()
                    .filter(|t| {
                        t.view.id == id
                            && t.view.direction == TransferDirection::Receiving
                            && t.view.state == TransferState::Paused
                            && t.view.pause_ready
                    })
                    .ok_or(Error::Protocol)?;
                t.view.resume_requested = true;
            }
            Packet::Control(Control::FileResume { id, offset, sha256 }) => {
                let active = self.active.clone();
                let t = self
                    .current
                    .as_mut()
                    .filter(|t| {
                        t.view.id == id
                            && t.view.direction == TransferDirection::Sending
                            && t.view.state == TransferState::Paused
                            && t.view.pause_ready
                    })
                    .ok_or(Error::Protocol)?;
                valid_checkpoint(offset, &sha256, t.view.size)?;
                if t.checkpoint.as_ref() != Some(&(offset, sha256.clone())) {
                    return Err(Error::InvalidFile);
                }
                let result = (|| {
                    let source = t.source.as_mut().ok_or(Error::InvalidState)?;
                    let metadata = source.metadata().map_err(|_| Error::FileIo)?;
                    if metadata.len() != t.view.size
                        || metadata.modified().ok() != t.source_modified
                    {
                        return Err(Error::FileIntegrity);
                    }
                    let digest = prefix_digest(source, offset, &active)?;
                    if URL_SAFE_NO_PAD.encode(digest.clone().finish().as_ref()) != sha256 {
                        return Err(Error::FileIntegrity);
                    }
                    let metadata = source.metadata().map_err(|_| Error::FileIo)?;
                    if metadata.len() != t.view.size
                        || metadata.modified().ok() != t.source_modified
                    {
                        return Err(Error::FileIntegrity);
                    }
                    Ok(digest)
                })();
                match result {
                    Ok(digest) => {
                        t.digest = digest;
                        t.view.transferred = offset;
                        t.awaiting_ack = false;
                        t.view.state = TransferState::Transferring;
                        t.view.pause_ready = false;
                        t.view.resume_requested = false;
                        t.checkpoint = None;
                        t.last_progress = Instant::now();
                        self.enqueue(Control::FileResumed { id, offset, sha256 })?;
                    }
                    Err(error) => {
                        t.finish(TransferState::Error, Some(error.to_string()));
                        self.enqueue(Control::FileCancel { id })?;
                    }
                }
            }
            Packet::Control(Control::FileResumed { id, offset, sha256 }) => {
                let t = self
                    .current
                    .as_mut()
                    .filter(|t| {
                        t.view.id == id
                            && t.view.direction == TransferDirection::Receiving
                            && t.view.state == TransferState::Paused
                            && t.resume_waiting
                    })
                    .ok_or(Error::Protocol)?;
                valid_checkpoint(offset, &sha256, t.view.size)?;
                if t.checkpoint.as_ref() != Some(&(offset, sha256)) || offset != t.view.transferred
                {
                    return Err(Error::InvalidFile);
                }
                t.view.state = TransferState::Transferring;
                t.view.pause_ready = false;
                t.view.resume_requested = false;
                t.resume_waiting = false;
                t.checkpoint = None;
                t.last_progress = Instant::now();
            }
            Packet::FileChunk { id, offset, bytes } => {
                let t = self
                    .current
                    .as_mut()
                    .filter(|t| {
                        t.view.id == id
                            && t.view.direction == TransferDirection::Receiving
                            && (t.view.state == TransferState::Transferring
                                || t.view.state == TransferState::Paused && !t.view.pause_ready)
                    })
                    .ok_or(Error::Protocol)?;
                if bytes.is_empty()
                    || bytes.len() > FILE_CHUNK
                    || offset != t.view.transferred
                    || offset
                        .checked_add(bytes.len() as u64)
                        .is_none_or(|end| end > t.view.size)
                {
                    return Err(Error::InvalidFile);
                }
                if t.temporary
                    .as_mut()
                    .ok_or(Error::InvalidState)?
                    .write_all(&bytes)
                    .is_err()
                {
                    t.finish(TransferState::Error, Some(Error::FileIo.to_string()));
                    return self.enqueue(Control::FileCancel { id });
                }
                t.digest.update(&bytes);
                t.view.transferred += bytes.len() as u64;
                t.last_progress = Instant::now();
            }
            Packet::Control(Control::FileDone { id, sha256 }) => {
                let active = self.active.clone();
                #[cfg(test)]
                let before_commit = self.before_commit.take();
                let t = self
                    .current
                    .as_mut()
                    .filter(|t| {
                        t.view.id == id
                            && t.view.direction == TransferDirection::Receiving
                            && (t.view.state == TransferState::Transferring
                                || t.view.state == TransferState::Paused && !t.view.pause_ready)
                    })
                    .ok_or(Error::Protocol)?;
                let expected = URL_SAFE_NO_PAD
                    .decode(sha256)
                    .map_err(|_| Error::InvalidFile)?;
                if expected.as_slice() != t.digest.clone().finish().as_ref()
                    || t.view.transferred != t.view.size
                {
                    t.finish(TransferState::Error, Some(Error::FileIntegrity.to_string()));
                    return self.enqueue(Control::FileCancel { id });
                }
                let temporary = t.temporary.take().ok_or(Error::InvalidState)?;
                let result = temporary
                    .as_file()
                    .sync_all()
                    .map_err(|_| Error::FileIo)
                    .and_then(|_| {
                        #[cfg(test)]
                        if let Some(barrier) = before_commit {
                            barrier();
                        }
                        // Commit boundary follows the potentially slow flush. Stop may
                        // revoke while flushing; in that case the temporary is discarded.
                        if !active.load(Ordering::SeqCst) {
                            return Err(Error::InvalidState);
                        }
                        temporary
                            .persist_noclobber(t.destination.as_ref().ok_or(Error::InvalidState)?)
                            .map(|_| ())
                            .map_err(|_| Error::DestinationExists)
                    });
                match result {
                    Ok(()) => {
                        t.finish(TransferState::Completed, None);
                        self.enqueue(Control::FileComplete { id })?;
                    }
                    Err(error) => {
                        t.finish(TransferState::Error, Some(error.to_string()));
                        self.enqueue(Control::FileCancel { id })?;
                    }
                }
            }
            Packet::Control(Control::FileComplete { id }) => {
                let t = self
                    .current
                    .as_mut()
                    .filter(|t| {
                        t.view.id == id
                            && t.view.direction == TransferDirection::Sending
                            && t.view.state == TransferState::Transferring
                            && t.awaiting_ack
                    })
                    .ok_or(Error::Protocol)?;
                t.finish(TransferState::Completed, None);
            }
            _ => return Err(Error::Protocol),
        }
        Ok(())
    }
    pub(crate) fn next_packet(&mut self) -> Result<Option<Packet>> {
        if !self.active.load(Ordering::SeqCst) {
            return Ok(None);
        }
        if let Some(message) = self.outgoing.pop_front() {
            return Ok(Some(Packet::Control(message)));
        }
        let Some(t) = self.current.as_mut() else {
            return Ok(None);
        };
        if t.view.busy()
            && (t.view.state != TransferState::Paused || !t.view.pause_ready || t.resume_waiting)
            && t.last_progress.elapsed()
                >= if t.view.state == TransferState::Offered {
                    OFFER_TIMEOUT
                } else {
                    PROGRESS_TIMEOUT
                }
        {
            let id = t.view.id.clone();
            t.finish(TransferState::Error, Some(Error::Timeout.to_string()));
            return Ok(Some(Packet::Control(Control::FileCancel { id })));
        }
        if t.view.direction != TransferDirection::Sending
            || t.view.state != TransferState::Transferring
            || t.awaiting_ack
        {
            return Ok(None);
        }
        let result = (|| {
            let mut bytes = vec![0; FILE_CHUNK];
            let count = t
                .source
                .as_mut()
                .ok_or(Error::InvalidState)?
                .read(&mut bytes)
                .map_err(|_| Error::FileIo)?;
            bytes.truncate(count);
            if count == 0 {
                if t.view.transferred != t.view.size {
                    return Err(Error::FileIntegrity);
                }
                t.awaiting_ack = true;
                return Ok(Packet::Control(Control::FileDone {
                    id: t.view.id.clone(),
                    sha256: URL_SAFE_NO_PAD.encode(t.digest.clone().finish().as_ref()),
                }));
            }
            if t.view.transferred + count as u64 > t.view.size {
                return Err(Error::FileIntegrity);
            }
            let offset = t.view.transferred;
            t.digest.update(&bytes);
            t.view.transferred += count as u64;
            t.last_progress = Instant::now();
            Ok(Packet::FileChunk {
                id: t.view.id.clone(),
                offset,
                bytes,
            })
        })();
        match result {
            Ok(packet) => Ok(Some(packet)),
            Err(error) => {
                let id = t.view.id.clone();
                t.finish(TransferState::Error, Some(error.to_string()));
                Ok(Some(Packet::Control(Control::FileCancel { id })))
            }
        }
    }
}
fn valid_checkpoint(offset: u64, sha256: &str, size: u64) -> Result<()> {
    if offset > size
        || sha256.len() != 43
        || !URL_SAFE_NO_PAD
            .decode(sha256)
            .is_ok_and(|bytes| bytes.len() == 32)
    {
        Err(Error::InvalidFile)
    } else {
        Ok(())
    }
}
fn prefix_digest(file: &mut File, offset: u64, active: &AtomicBool) -> Result<Context> {
    file.seek(SeekFrom::Start(0)).map_err(|_| Error::FileIo)?;
    let mut digest = Context::new(&SHA256);
    let mut remaining = offset;
    let mut buffer = vec![0u8; FILE_CHUNK];
    while remaining > 0 {
        if !active.load(Ordering::SeqCst) {
            return Err(Error::InvalidState);
        }
        let length = remaining.min(FILE_CHUNK as u64) as usize;
        file.read_exact(&mut buffer[..length])
            .map_err(|_| Error::FileIntegrity)?;
        digest.update(&buffer[..length]);
        remaining -= length as u64;
    }
    if !active.load(Ordering::SeqCst) {
        return Err(Error::InvalidState);
    }
    Ok(digest)
}
pub(crate) fn valid_id(id: &str) -> Result<()> {
    if Uuid::parse_str(id).is_ok_and(|value| value.to_string() == id) {
        Ok(())
    } else {
        Err(Error::InvalidFile)
    }
}
pub(crate) fn validate_name(name: &str) -> Result<()> {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if name.is_empty()||name.len()>255||name=="."||name==".."||name.ends_with(['.',' '])||name.chars().any(|c|c.is_control()||matches!(c,'/'|'\\'|':'|'<'|'>'|'"'|'|'|'?'|'*'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}'))||matches!(stem.as_str(),"CON"|"PRN"|"AUX"|"NUL")||((stem.starts_with("COM")||stem.starts_with("LPT"))&&stem.len()==4&&stem.as_bytes()[3].is_ascii_digit()) {Err(Error::InvalidFile)}else{Ok(())}
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wrong_offset_and_declared_size_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = FileManager::new();
        let id = Uuid::new_v4().to_string();
        m.receive(Packet::Control(Control::FileOffer {
            id: id.clone(),
            name: "test.txt".into(),
            size: 3,
        }))
        .unwrap();
        m.accept(&id, dir.path().join("file")).unwrap();
        assert!(m
            .receive(Packet::FileChunk {
                id: id.clone(),
                offset: 1,
                bytes: vec![1]
            })
            .is_err());
        assert!(m
            .receive(Packet::FileChunk {
                id,
                offset: 0,
                bytes: vec![0; 4]
            })
            .is_err());
        m.deactivate();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
    #[test]
    fn expired_offer_cannot_be_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = FileManager::new();
        let id = Uuid::new_v4().to_string();
        m.receive(Packet::Control(Control::FileOffer {
            id: id.clone(),
            name: "test.txt".into(),
            size: 0,
        }))
        .unwrap();
        m.current.as_mut().unwrap().last_progress = Instant::now() - OFFER_TIMEOUT;
        assert!(m.accept(&id, dir.path().join("file")).is_err());
        assert!(matches!(
            m.next_packet().unwrap(),
            Some(Packet::Control(Control::FileCancel { .. }))
        ));
    }
    #[test]
    fn atomic_revocation_does_not_wait_for_disk_mutex() {
        let handle = FileHandle::new();
        let guard = handle.manager.lock().unwrap();
        handle.stop();
        assert!(handle.snapshot().is_none());
        assert!(!handle.active.load(Ordering::SeqCst));
        drop(guard);
        assert!(handle
            .run(|m| m.offer(PathBuf::from("never-opened")))
            .is_err());
    }
    #[test]
    fn stop_between_flush_and_commit_never_publishes() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("received.txt");
        let id = Uuid::new_v4().to_string();
        let handle = FileHandle::new();
        handle
            .run(|m| {
                m.receive(Packet::Control(Control::FileOffer {
                    id: id.clone(),
                    name: "test.txt".into(),
                    size: 3,
                }))
            })
            .unwrap();
        handle.run(|m| m.accept(&id, target.clone())).unwrap();
        handle
            .run(|m| {
                m.receive(Packet::FileChunk {
                    id: id.clone(),
                    offset: 0,
                    bytes: b"abc".to_vec(),
                })
            })
            .unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        handle.manager.lock().unwrap().before_commit = Some(Box::new(move || {
            ready_tx.send(()).unwrap();
            resume_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        }));
        let worker = handle.clone();
        let task = std::thread::spawn(move || {
            worker.run(|m| {
                m.receive(Packet::Control(Control::FileDone {
                    id,
                    sha256: URL_SAFE_NO_PAD.encode(ring::digest::digest(&SHA256, b"abc").as_ref()),
                }))
            })
        });
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        handle.stop();
        resume_tx.send(()).unwrap();
        task.join().unwrap().unwrap();
        handle.cleanup().unwrap();
        assert!(!target.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
    #[test]
    fn unsafe_names_rejected() {
        for name in [
            "../x",
            "a/b",
            "a\\b",
            "C:ads",
            "CON.txt",
            "..",
            "trailing.",
            "x\u{202e}exe",
        ] {
            assert!(validate_name(name).is_err(), "{name}");
        }
        assert!(validate_name("Отчёт.pdf").is_ok());
    }
    #[test]
    fn integrity_failure_cleans_partial() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = FileManager::new();
        let id = Uuid::new_v4().to_string();
        m.receive(Packet::Control(Control::FileOffer {
            id: id.clone(),
            name: "a.txt".into(),
            size: 3,
        }))
        .unwrap();
        let target = dir.path().join("a.txt");
        m.accept(&id, target.clone()).unwrap();
        m.receive(Packet::FileChunk {
            id: id.clone(),
            offset: 0,
            bytes: b"abc".to_vec(),
        })
        .unwrap();
        m.receive(Packet::Control(Control::FileDone {
            id,
            sha256: URL_SAFE_NO_PAD.encode([0u8; 32]),
        }))
        .unwrap();
        assert!(!target.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        assert_eq!(m.snapshot().unwrap().state, TransferState::Error);
    }
    #[test]
    fn deactivate_removes_partial_and_blocks_late_write() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = FileManager::new();
        let id = Uuid::new_v4().to_string();
        m.receive(Packet::Control(Control::FileOffer {
            id: id.clone(),
            name: "a.txt".into(),
            size: 3,
        }))
        .unwrap();
        m.accept(&id, dir.path().join("a.txt")).unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        m.deactivate();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        assert!(m
            .receive(Packet::FileChunk {
                id,
                offset: 0,
                bytes: b"abc".to_vec()
            })
            .is_err());
    }
    #[test]
    fn unexpected_chunk_requires_acceptance() {
        let mut m = FileManager::new();
        let id = Uuid::new_v4().to_string();
        m.receive(Packet::Control(Control::FileOffer {
            id: id.clone(),
            name: "a.txt".into(),
            size: 3,
        }))
        .unwrap();
        assert!(m
            .receive(Packet::FileChunk {
                id,
                offset: 0,
                bytes: vec![1]
            })
            .is_err());
    }
}

#[cfg(test)]
mod pause_tests {
    use super::*;
    fn pair(
        size: usize,
    ) -> (
        tempfile::TempDir,
        FileManager,
        FileManager,
        String,
        PathBuf,
        PathBuf,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.bin");
        let destination = dir.path().join("received.bin");
        std::fs::write(
            &source,
            (0..size).map(|i| (i % 251) as u8).collect::<Vec<_>>(),
        )
        .unwrap();
        let mut sender = FileManager::new();
        let mut receiver = FileManager::new();
        let id = sender.offer(source.clone()).unwrap();
        receiver
            .receive(sender.next_packet().unwrap().unwrap())
            .unwrap();
        receiver.accept(&id, destination.clone()).unwrap();
        sender
            .receive(receiver.next_packet().unwrap().unwrap())
            .unwrap();
        (dir, sender, receiver, id, source, destination)
    }
    fn forward(sender: &mut FileManager, receiver: &mut FileManager) {
        receiver
            .receive(sender.next_packet().unwrap().unwrap())
            .unwrap();
    }
    #[test]
    fn pause_drains_inflight_chunks_and_resume_requires_receiver_approval() {
        let (_dir, mut sender, mut receiver, id, source, destination) = pair(FILE_CHUNK * 3 + 9);
        forward(&mut sender, &mut receiver);
        let inflight = sender.next_packet().unwrap().unwrap();
        sender.pause(&id).unwrap();
        assert_eq!(sender.snapshot().unwrap().state, TransferState::Paused);
        receiver.receive(inflight).unwrap();
        forward(&mut sender, &mut receiver);
        forward(&mut receiver, &mut sender);
        assert!(sender.snapshot().unwrap().pause_ready);
        assert!(receiver.snapshot().unwrap().pause_ready);
        assert_eq!(
            receiver.snapshot().unwrap().transferred,
            (FILE_CHUNK * 2) as u64
        );
        sender.current.as_mut().unwrap().last_progress = Instant::now() - PROGRESS_TIMEOUT * 2;
        assert!(sender.next_packet().unwrap().is_none());
        sender.resume(&id).unwrap();
        forward(&mut sender, &mut receiver);
        assert!(receiver.snapshot().unwrap().resume_requested);
        assert_eq!(sender.snapshot().unwrap().state, TransferState::Paused);
        assert!(sender.next_packet().unwrap().is_none());
        receiver.resume(&id).unwrap();
        forward(&mut receiver, &mut sender);
        forward(&mut sender, &mut receiver);
        let packet = sender.next_packet().unwrap().unwrap();
        assert!(matches!(&packet,Packet::FileChunk{offset,..} if *offset==(FILE_CHUNK*2)as u64));
        receiver.receive(packet).unwrap();
        while sender.snapshot().unwrap().state != TransferState::Completed {
            if let Some(packet) = sender.next_packet().unwrap() {
                receiver.receive(packet).unwrap();
            }
            if let Some(packet) = receiver.next_packet().unwrap() {
                sender.receive(packet).unwrap();
            }
        }
        assert_eq!(
            std::fs::read(source).unwrap(),
            std::fs::read(destination).unwrap()
        );
    }
    #[test]
    fn changed_source_prefix_cancels_resume_and_removes_partial() {
        let (_dir, mut sender, mut receiver, id, source, destination) = pair(FILE_CHUNK * 2);
        forward(&mut sender, &mut receiver);
        sender.pause(&id).unwrap();
        forward(&mut sender, &mut receiver);
        forward(&mut receiver, &mut sender);
        let mut bytes = std::fs::read(&source).unwrap();
        bytes[0] ^= 1;
        std::fs::write(source, bytes).unwrap();
        receiver.resume(&id).unwrap();
        forward(&mut receiver, &mut sender);
        assert_eq!(sender.snapshot().unwrap().state, TransferState::Error);
        forward(&mut sender, &mut receiver);
        assert!(receiver.current.as_ref().unwrap().temporary.is_none());
        assert!(!destination.exists());
    }
    #[test]
    fn receiver_pause_handles_completion_already_inflight() {
        let (_dir, mut sender, mut receiver, id, _, destination) = pair(1);
        forward(&mut sender, &mut receiver);
        let done = sender.next_packet().unwrap().unwrap();
        receiver.pause(&id).unwrap();
        forward(&mut receiver, &mut sender);
        receiver.receive(done).unwrap();
        forward(&mut receiver, &mut sender);
        assert_eq!(sender.snapshot().unwrap().state, TransferState::Completed);
        assert!(destination.exists());
    }
    #[test]
    fn paused_transfer_stays_busy_and_disconnect_cleans_it() {
        let (_dir, mut sender, mut receiver, id, source, destination) = pair(FILE_CHUNK * 2);
        forward(&mut sender, &mut receiver);
        sender.pause(&id).unwrap();
        forward(&mut sender, &mut receiver);
        forward(&mut receiver, &mut sender);
        assert!(matches!(sender.offer(source), Err(Error::TransferBusy)));
        receiver.deactivate();
        assert!(receiver.current.as_ref().unwrap().temporary.is_none());
        assert!(!destination.exists());
        assert!(receiver.resume(&id).is_err());
    }
    #[test]
    fn pause_checkpoint_rejects_offset_hash_and_changed_partial() {
        let (_dir, mut sender, mut receiver, id, _, destination) = pair(FILE_CHUNK * 2);
        forward(&mut sender, &mut receiver);
        sender.pause(&id).unwrap();
        forward(&mut sender, &mut receiver);
        forward(&mut receiver, &mut sender);
        let (offset, sha256) = sender.current.as_ref().unwrap().checkpoint.clone().unwrap();
        for (bad_offset, bad_hash) in [
            (offset + 1, sha256.clone()),
            (offset, URL_SAFE_NO_PAD.encode([0u8; 32])),
            (offset, "bad".into()),
        ] {
            assert!(sender
                .receive(Packet::Control(Control::FileResume {
                    id: id.clone(),
                    offset: bad_offset,
                    sha256: bad_hash
                }))
                .is_err());
            assert_eq!(sender.snapshot().unwrap().state, TransferState::Paused);
        }
        let partial = receiver
            .current
            .as_mut()
            .unwrap()
            .temporary
            .as_mut()
            .unwrap();
        partial.as_file_mut().seek(SeekFrom::Start(0)).unwrap();
        partial.as_file_mut().write_all(&[255]).unwrap();
        assert!(matches!(receiver.resume(&id), Err(Error::FileIntegrity)));
        assert_eq!(receiver.snapshot().unwrap().state, TransferState::Error);
        assert!(!destination.exists());
        assert!(receiver.current.as_ref().unwrap().temporary.is_none());
    }
    #[test]
    fn simultaneous_pause_barriers_and_empty_prefix_resume_are_valid() {
        let (_dir, mut sender, mut receiver, id, _, destination) = pair(3);
        sender.pause(&id).unwrap();
        receiver.pause(&id).unwrap();
        forward(&mut receiver, &mut sender);
        forward(&mut sender, &mut receiver);
        forward(&mut sender, &mut receiver);
        forward(&mut receiver, &mut sender);
        forward(&mut receiver, &mut sender);
        assert_eq!(sender.snapshot().unwrap().transferred, 0);
        assert!(sender.snapshot().unwrap().pause_ready);
        receiver.resume(&id).unwrap();
        forward(&mut receiver, &mut sender);
        forward(&mut sender, &mut receiver);
        forward(&mut sender, &mut receiver);
        forward(&mut sender, &mut receiver);
        forward(&mut receiver, &mut sender);
        assert_eq!(std::fs::metadata(destination).unwrap().len(), 3);
        assert_eq!(sender.snapshot().unwrap().state, TransferState::Completed);
    }
    #[test]
    fn destination_created_while_paused_is_never_overwritten() {
        let (_dir, mut sender, mut receiver, id, _, destination) = pair(FILE_CHUNK + 1);
        forward(&mut sender, &mut receiver);
        sender.pause(&id).unwrap();
        forward(&mut sender, &mut receiver);
        forward(&mut receiver, &mut sender);
        std::fs::write(&destination, b"existing owner file").unwrap();
        receiver.resume(&id).unwrap();
        forward(&mut receiver, &mut sender);
        forward(&mut sender, &mut receiver);
        forward(&mut sender, &mut receiver);
        forward(&mut sender, &mut receiver);
        forward(&mut receiver, &mut sender);
        assert_eq!(std::fs::read(destination).unwrap(), b"existing owner file");
        assert_eq!(receiver.snapshot().unwrap().state, TransferState::Error);
    }
}
