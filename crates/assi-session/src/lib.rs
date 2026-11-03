//! Invitation-authenticated end-to-end TLS sessions, optional relay and approved files.
//! Capture and input injection remain in the platform adapter.
mod system_actions;
pub use system_actions::{SystemAction, SystemCapabilities, SystemRequestState, SystemSnapshot};
mod terminal;
pub use terminal::{TerminalCommand, TerminalEvent, TerminalSnapshot, TERMINAL_CHUNK};
mod trusted;
pub use trusted::HostIdentity;
mod tools;
pub use tools::{
    ChatAuthor, ChatMessage, ChatSnapshot, ClipboardBatch, MAX_CHAT_BYTES, MAX_CHAT_MESSAGES,
    MAX_CLIPBOARD_BYTES,
};
mod displays;
pub use displays::RemoteDisplay;
mod video;
pub use video::{VideoCodec, VideoFrame, MAX_VIDEO_FRAMES};
mod annotations;
mod audio;
use audio::AudioState;
pub use audio::{
    AudioBatch, AudioPacket, AUDIO_PACKET_SAMPLES, AUDIO_SAMPLE_RATE, MAX_AUDIO_PACKETS,
};
mod files;
use annotations::AnnotationEvent;
pub use annotations::{
    AnnotationAuthor, AnnotationColor, AnnotationCommand, AnnotationPoint, AnnotationSnapshot,
    AnnotationStroke, AnnotationTool, SharedAnnotation, MAX_ANNOTATION_POINTS,
    MAX_ANNOTATION_STROKES,
};
mod wire;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use files::FileHandle;
pub use files::{TransferDirection, TransferSnapshot, TransferState, MAX_FILE_SIZE};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::{
    collections::VecDeque,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot, watch},
    time::timeout,
};
use tokio_rustls::{
    rustls::{
        self,
        pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName},
    },
    TlsAcceptor, TlsConnector,
};
use uuid::Uuid;
use wire::{Control, Packet};
pub const PROTOCOL_VERSION: u8 = 4;
pub const PENDING_TIMEOUT: Duration = Duration::from_secs(60);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(8);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_INPUTS: usize = 128;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Приглашение недействительно или слишком длинное.")]
    InvalidInvitation,
    #[error("Имя должно содержать от 1 до 60 символов без управляющих знаков.")]
    InvalidName,
    #[error("Сеанс находится в другом состоянии. Обновите экран.")]
    InvalidState,
    #[error("Нет разрешения на удалённое управление.")]
    PermissionDenied,
    #[error("Недопустимое событие ввода.")]
    InvalidInput,
    #[error("Недопустимая пометка или превышен лимит рисования.")]
    InvalidAnnotation,
    #[error("Недопустимый аудиопакет: ожидается PCM 48 кГц, 20 мс, моно или стерео.")]
    InvalidAudio,
    #[error("Недопустимый кадр или превышен лимит изображения.")]
    InvalidFrame,
    #[error("Ошибка протокола соединения.")]
    Protocol,
    #[error("Не удалось проверить приглашение.")]
    Authentication,
    #[error("Истекло время ожидания.")]
    Timeout,
    #[error("Слишком много событий ввода. Сеанс необходимо завершить.")]
    Backpressure,
    #[error("Не удалось создать защищённое соединение.")]
    Tls,
    #[error("Не удалось открыть сетевое соединение.")]
    Io(#[from] std::io::Error),
    #[error("Внутренняя ошибка состояния сеанса.")]
    State,
    #[error("Уже передаётся другой файл.")]
    TransferBusy,
    #[error("Недопустимое имя, размер или содержимое файла.")]
    InvalidFile,
    #[error("Не удалось прочитать или сохранить файл.")]
    FileIo,
    #[error("Файл изменился или проверка целостности не пройдена.")]
    FileIntegrity,
    #[error("Файл назначения уже существует или не может быть сохранён.")]
    DestinationExists,
    #[error("Не удалось установить соединение через relay.")]
    Relay,
}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Idle,
    Host,
    Viewer,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Idle,
    Listening,
    Connecting,
    Pending,
    Active,
    Closed,
    Error,
}
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub role: Role,
    pub state: SessionState,
    pub request_id: Option<String>,
    pub peer_name: Option<String>,
    pub control: bool,
    pub audio_enabled: bool,
    pub audio_epoch: u64,
    pub clipboard_enabled: bool,
    pub clipboard_epoch: u64,
    pub screen_epoch: u64,
    pub screen_pending: bool,
    pub displays: Vec<RemoteDisplay>,
    pub selected_display: Option<String>,
    pub video_codec: VideoCodec,
    pub error: Option<String>,
    pub transfer: Option<TransferSnapshot>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            role: Role::Idle,
            state: SessionState::Idle,
            request_id: None,
            peer_name: None,
            control: false,
            audio_enabled: false,
            audio_epoch: 0,
            clipboard_enabled: false,
            clipboard_epoch: 0,
            screen_epoch: 1,
            screen_pending: false,
            displays: Vec::new(),
            selected_display: None,
            video_codec: VideoCodec::Jpeg,
            error: None,
            transfer: None,
        }
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    #[serde(rename = "0")]
    Digit0,
    #[serde(rename = "1")]
    Digit1,
    #[serde(rename = "2")]
    Digit2,
    #[serde(rename = "3")]
    Digit3,
    #[serde(rename = "4")]
    Digit4,
    #[serde(rename = "5")]
    Digit5,
    #[serde(rename = "6")]
    Digit6,
    #[serde(rename = "7")]
    Digit7,
    #[serde(rename = "8")]
    Digit8,
    #[serde(rename = "9")]
    Digit9,
    Enter,
    Escape,
    Backspace,
    Tab,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Control,
    Alt,
    Shift,
    Meta,
    Space,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    CapsLock,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputEvent {
    ReleaseAll,
    Move { x: f64, y: f64 },
    Wheel { x: i32, y: i32 },
    Button { button: MouseButton, down: bool },
    Key { key: Key, down: bool },
    Text { text: String },
}
impl InputEvent {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Move { x, y }
                if !x.is_finite()
                    || !y.is_finite()
                    || !(0.0..=1.0).contains(x)
                    || !(0.0..=1.0).contains(y) =>
            {
                Err(Error::InvalidInput)
            }
            Self::Wheel { x, y } if !(-100..=100).contains(x) || !(-100..=100).contains(y) => {
                Err(Error::InvalidInput)
            }
            Self::Text { text } => {
                if text.is_empty()
                    || text.len() > 4096
                    || text
                        .chars()
                        .any(|c| c.is_control() && c != '\n' && c != '\t')
                {
                    return Err(Error::InvalidInput);
                }
                // Escaping tabs/newlines can double bytes. Reject before queueing
                // so a valid local text operation cannot overflow a control packet.
                let bytes = serde_json::to_vec(&Control::Input {
                    screen_epoch: 1,
                    event: self.clone(),
                })
                .map_err(|_| Error::InvalidInput)?;
                if bytes.len() >= wire::MAX_CONTROL {
                    return Err(Error::InvalidInput);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Invitation {
    version: u8,
    endpoint: SocketAddr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    relay: Option<RelayRoute>,
    certificate: String,
    token: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RelayRoute {
    config: assi_relay::RelayConfig,
    room: String,
    ticket: String,
}
impl Invitation {
    fn encode(&self) -> Result<String> {
        Ok(format!(
            "assi://{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).map_err(|_| Error::InvalidInvitation)?)
        ))
    }
    fn decode(text: &str) -> Result<Self> {
        if text.len() > 16_384 {
            return Err(Error::InvalidInvitation);
        }
        let raw = URL_SAFE_NO_PAD
            .decode(
                text.trim()
                    .strip_prefix("assi://")
                    .ok_or(Error::InvalidInvitation)?,
            )
            .map_err(|_| Error::InvalidInvitation)?;
        let value: Self = serde_json::from_slice(&raw).map_err(|_| Error::InvalidInvitation)?;
        let cert = URL_SAFE_NO_PAD
            .decode(&value.certificate)
            .map_err(|_| Error::InvalidInvitation)?;
        let token = URL_SAFE_NO_PAD
            .decode(&value.token)
            .map_err(|_| Error::InvalidInvitation)?;
        if value.version != PROTOCOL_VERSION
            || value.endpoint.port() == 0
            || !usable_ip(value.endpoint.ip())
            || cert.is_empty()
            || cert.len() > 4096
            || token.len() != 32
        {
            return Err(Error::InvalidInvitation);
        }
        if let Some(relay) = &value.relay {
            relay
                .config
                .validate()
                .map_err(|_| Error::InvalidInvitation)?;
            if relay.room.len() > 128
                || relay.room.is_empty()
                || relay.ticket.len() > 128
                || relay.ticket.is_empty()
            {
                return Err(Error::InvalidInvitation);
            }
        }
        Ok(value)
    }
}
fn usable_ip(ip: IpAddr) -> bool {
    !ip.is_unspecified() && !ip.is_multicast() && !matches!(ip,IpAddr::V4(ip) if ip.is_broadcast())
}
fn name_ok(name: &str) -> bool {
    let len = name.trim().chars().count();
    (1..=60).contains(&len) && !name.chars().any(char::is_control)
}
struct Inner {
    generation: u64,
    snapshot: Snapshot,
    cancel: Option<watch::Sender<bool>>,
    decision: Option<oneshot::Sender<Option<bool>>>,
    pending_deadline: Option<tokio::time::Instant>,
    frames: Option<watch::Sender<Option<Arc<Vec<u8>>>>>,
    outbound: Option<mpsc::Sender<(u64, InputEvent)>>,
    annotation_outbound: Option<mpsc::Sender<Control>>,
    annotations: AnnotationSnapshot,
    audio: AudioState,
    system: system_actions::SystemState,
    terminal: terminal::TerminalState,
    tools: tools::TextTools,
    display: displays::DisplayState,
    video: video::VideoState,
    auto_accept: Option<bool>,
    frame: Option<Vec<u8>>,
    inputs: VecDeque<InputEvent>,
    task: Option<tokio::task::JoinHandle<()>>,
    files: Option<Arc<FileHandle>>,
    file_cleanup: Vec<tokio::task::JoinHandle<Result<()>>>,
    file_cleanup_signals: Vec<watch::Receiver<Option<bool>>>,
}
pub struct SessionEngine {
    inner: Mutex<Inner>,
    lifecycle: tokio::sync::Mutex<()>,
}
impl SessionEngine {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                generation: 0,
                snapshot: Snapshot::default(),
                cancel: None,
                decision: None,
                pending_deadline: None,
                frames: None,
                outbound: None,
                annotation_outbound: None,
                annotations: AnnotationSnapshot::default(),
                audio: AudioState::default(),
                system: system_actions::SystemState::default(),
                terminal: terminal::TerminalState::default(),
                tools: tools::TextTools::default(),
                display: displays::DisplayState::default(),
                video: video::VideoState::default(),
                auto_accept: None,
                frame: None,
                inputs: VecDeque::new(),
                task: None,
                files: None,
                file_cleanup: Vec::new(),
                file_cleanup_signals: Vec::new(),
            }),
            lifecycle: tokio::sync::Mutex::new(()),
        })
    }
    fn lock(&self) -> Result<MutexGuard<'_, Inner>> {
        self.inner.lock().map_err(|_| Error::State)
    }
    pub fn snapshot(&self) -> Snapshot {
        self.lock()
            .map(|i| {
                let mut snapshot = i.snapshot.clone();
                snapshot.transfer = i.files.as_ref().and_then(|files| files.snapshot());
                snapshot
            })
            .unwrap_or_else(|_| Snapshot {
                state: SessionState::Error,
                error: Some(Error::State.to_string()),
                ..Snapshot::default()
            })
    }
    fn reset(&self, role: Role, state: SessionState) -> Result<(u64, watch::Receiver<bool>)> {
        let mut i = self.lock()?;
        if let Some(cancel) = i.cancel.take() {
            let _ = cancel.send(true);
        }
        i.generation = i.generation.wrapping_add(1);
        i.snapshot = Snapshot {
            role,
            state,
            ..Snapshot::default()
        };
        if let Some(files) = i.files.take() {
            Self::schedule_file_cleanup(&mut i, files);
        }
        i.decision = None;
        i.pending_deadline = None;
        i.frames = None;
        i.outbound = None;
        i.annotation_outbound = None;
        i.annotations = AnnotationSnapshot::default();
        i.audio = AudioState::default();
        i.system = system_actions::SystemState::default();
        i.terminal = terminal::TerminalState::default();
        i.tools = tools::TextTools::default();
        i.display = displays::DisplayState::default();
        i.video = video::VideoState::default();
        i.auto_accept = None;
        i.frame = None;
        i.inputs.clear();
        let (tx, rx) = watch::channel(false);
        i.cancel = Some(tx);
        Ok((i.generation, rx))
    }
    fn schedule_file_cleanup(i: &mut Inner, files: Arc<FileHandle>) {
        files.stop();
        // Completion witnesses survive movement or cancellation of JoinHandles.
        // Only known successes can be pruned; panic/failed cleanup stays visible.
        i.file_cleanup_signals
            .retain(|signal| *signal.borrow() != Some(true));
        let (finished, signal) = watch::channel(None);
        i.file_cleanup_signals.push(signal);
        i.file_cleanup.push(tokio::task::spawn_blocking(move || {
            let result = files.cleanup();
            finished.send_replace(Some(result.is_ok()));
            result
        }));
    }
    /// Wait for cleanups scheduled before this call without stopping a listener
    /// or touching a newer active peer. Call after observing the prior request's
    /// transition out of Active. Timeout/error means native temporary-directory
    /// guards must be retained and retried, never removed speculatively.
    pub async fn wait_file_cleanup(&self) -> Result<()> {
        let signals = { self.lock()?.file_cleanup_signals.clone() };
        timeout(Duration::from_secs(5), async move {
            for mut signal in signals {
                loop {
                    let completed = *signal.borrow_and_update();
                    match completed {
                        Some(true) => break,
                        Some(false) => return Err(Error::FileIo),
                        None => signal.changed().await.map_err(|_| Error::State)?,
                    }
                }
            }
            Ok(())
        })
        .await
        .map_err(|_| Error::Timeout)?
    }
    pub async fn stop(&self) -> Result<()> {
        let _op = self.lifecycle.lock().await;
        self.shutdown_current().await
    }
    // Caller holds lifecycle, so no new task can appear while this one is joined.
    async fn shutdown_current(&self) -> Result<()> {
        self.reset(Role::Idle, SessionState::Closed)?;
        let task = { self.lock()?.task.take() };
        let session_result = if let Some(task) = task {
            task.await.map_err(|_| Error::State)
        } else {
            Ok(())
        };
        let cleanup = { std::mem::take(&mut self.lock()?.file_cleanup) };
        let mut cleanup_result = Ok(());
        for task in cleanup {
            let result = task.await.map_err(|_| Error::State).and_then(|r| r);
            if result.is_err() {
                cleanup_result = result;
            }
        }
        // A cancelled trusted loop may already have taken these JoinHandles;
        // its detached blocking cleanup is still represented by the witnesses.
        let barrier_result = self.wait_file_cleanup().await;
        session_result.and(cleanup_result).and(barrier_result)
    }

    fn finish(&self, generation: u64, result: Result<()>) {
        if let Ok(mut i) = self.lock() {
            if i.generation != generation {
                return;
            }
            i.snapshot.state = if result.is_ok() {
                SessionState::Closed
            } else {
                SessionState::Error
            };
            i.snapshot.error = result.err().map(|e| e.to_string());
            i.snapshot.control = false;
            i.snapshot.audio_enabled = false;
            i.snapshot.audio_epoch = 0;
            i.snapshot.clipboard_enabled = false;
            i.snapshot.clipboard_epoch = 0;
            i.snapshot.screen_pending = false;
            i.snapshot.displays.clear();
            i.snapshot.selected_display = None;
            if let Some(files) = i.files.take() {
                Self::schedule_file_cleanup(&mut i, files);
            }
            i.decision = None;
            i.pending_deadline = None;
            i.frames = None;
            i.outbound = None;
            i.annotation_outbound = None;
            i.annotations = AnnotationSnapshot::default();
            i.audio = AudioState::default();
            i.system = system_actions::SystemState::default();
            i.terminal = terminal::TerminalState::default();
            i.tools = tools::TextTools::default();
            i.display = displays::DisplayState::default();
            i.video = video::VideoState::default();
            i.auto_accept = None;
            i.frame = None;
            i.inputs.clear();
            i.cancel = None;
        }
    }
    /// Clear one trusted peer without rotating its long-lived listener or credentials.
    fn reset_trusted_peer(&self, generation: u64) -> Result<()> {
        let mut i = self.lock()?;
        if i.generation != generation {
            return Err(Error::InvalidState);
        }
        if let Some(files) = i.files.take() {
            Self::schedule_file_cleanup(&mut i, files);
        }
        i.snapshot = Snapshot {
            role: Role::Host,
            state: SessionState::Listening,
            ..Snapshot::default()
        };
        i.decision = None;
        i.pending_deadline = None;
        i.frames = None;
        i.outbound = None;
        i.annotation_outbound = None;
        i.annotations = AnnotationSnapshot::default();
        i.audio = AudioState::default();
        i.system = system_actions::SystemState::default();
        i.terminal = terminal::TerminalState::default();
        i.tools = tools::TextTools::default();
        i.display = displays::DisplayState::default();
        i.video = video::VideoState::default();
        i.frame = None;
        i.inputs.clear();
        Ok(())
    }
    pub async fn start_host(
        self: &Arc<Self>,
        bind: SocketAddr,
        advertised_ip: IpAddr,
        name: String,
    ) -> Result<String> {
        if !name_ok(&name) {
            return Err(Error::InvalidName);
        }
        if !usable_ip(advertised_ip) {
            return Err(Error::InvalidInvitation);
        }
        let _op = self.lifecycle.lock().await;
        let key = rcgen::generate_simple_self_signed(vec!["assi.local".into()])
            .map_err(|_| Error::Tls)?;
        let cert = key.cert.der().clone();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| Error::Tls)?
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.clone()],
            PrivatePkcs8KeyDer::from(key.signing_key.serialize_der()).into(),
        )
        .map_err(|_| Error::Tls)?;
        self.shutdown_current().await?;
        let listener = TcpListener::bind(bind).await?;
        let endpoint = SocketAddr::new(advertised_ip, listener.local_addr()?.port());
        let mut token = [0u8; 32];
        SystemRandom::new()
            .fill(&mut token)
            .map_err(|_| Error::Tls)?;
        let invite = Invitation {
            version: PROTOCOL_VERSION,
            endpoint,
            relay: None,
            certificate: URL_SAFE_NO_PAD.encode(cert.as_ref()),
            token: URL_SAFE_NO_PAD.encode(token),
        }
        .encode()?;
        let (generation, mut cancel) = self.reset(Role::Host, SessionState::Listening)?;
        let engine = Arc::clone(self);
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let task = tokio::spawn(async move {
            let result = tokio::select! {biased;_ = cancel.changed()=>Ok(()),result=engine.host_loop(generation,listener,acceptor,token,name)=>result};
            engine.finish(generation, result);
        });
        self.lock()?.task = Some(task);
        Ok(invite)
    }
    /// Reserve an outbound relay connection; screen/data remain protected by inner TLS.
    pub async fn start_host_relay(
        self: &Arc<Self>,
        relay_config: assi_relay::RelayConfig,
        name: String,
    ) -> Result<String> {
        if !name_ok(&name) {
            return Err(Error::InvalidName);
        }
        relay_config
            .validate()
            .map_err(|_| Error::InvalidInvitation)?;
        let _op = self.lifecycle.lock().await;
        let key = rcgen::generate_simple_self_signed(vec!["assi.local".into()])
            .map_err(|_| Error::Tls)?;
        let cert = key.cert.der().clone();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| Error::Tls)?
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.clone()],
            PrivatePkcs8KeyDer::from(key.signing_key.serialize_der()).into(),
        )
        .map_err(|_| Error::Tls)?;
        self.shutdown_current().await?;
        let reservation = assi_relay::reserve(&relay_config)
            .await
            .map_err(|_| Error::Relay)?;
        let mut token = [0u8; 32];
        SystemRandom::new()
            .fill(&mut token)
            .map_err(|_| Error::Tls)?;
        let invite = Invitation {
            version: PROTOCOL_VERSION,
            endpoint: relay_config.endpoint,
            relay: Some(RelayRoute {
                config: relay_config,
                room: reservation.room.clone(),
                ticket: reservation.ticket.clone(),
            }),
            certificate: URL_SAFE_NO_PAD.encode(cert.as_ref()),
            token: URL_SAFE_NO_PAD.encode(token),
        }
        .encode()?;
        let (generation, mut cancel) = self.reset(Role::Host, SessionState::Listening)?;
        let engine = self.clone();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let task = tokio::spawn(async move {
            let session = async {
                let transport = reservation.wait().await.map_err(|_| Error::Relay)?;
                let (stream, peer) = timeout(HANDSHAKE_TIMEOUT, async {
                    let mut stream = acceptor.accept(transport).await.map_err(|_| Error::Tls)?;
                    let peer = read_hello(&mut stream, &token).await?;
                    Ok::<_, Error>((stream, peer))
                })
                .await
                .map_err(|_| Error::Timeout)??;
                engine.host_session(generation, stream, peer, name).await
            };
            let result = tokio::select! {biased;_=cancel.changed()=>Ok(()),result=session=>result};
            engine.finish(generation, result);
        });
        self.lock()?.task = Some(task);
        Ok(invite)
    }
    pub async fn connect(self: &Arc<Self>, invitation: String, name: String) -> Result<()> {
        if !name_ok(&name) {
            return Err(Error::InvalidName);
        }
        let invite = Invitation::decode(&invitation)?;
        let cert = URL_SAFE_NO_PAD
            .decode(&invite.certificate)
            .map_err(|_| Error::InvalidInvitation)?;
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(CertificateDer::from(cert))
            .map_err(|_| Error::InvalidInvitation)?;
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| Error::Tls)?
        .with_root_certificates(roots)
        .with_no_client_auth();
        let _op = self.lifecycle.lock().await;
        self.shutdown_current().await?;
        let (generation, mut cancel) = self.reset(Role::Viewer, SessionState::Connecting)?;
        let engine = Arc::clone(self);
        let task = tokio::spawn(async move {
            let result = tokio::select! {biased;_=cancel.changed()=>Ok(()),result=engine.viewer_connect(generation,invite,name,config)=>result};
            engine.finish(generation, result);
        });
        self.lock()?.task = Some(task);
        Ok(())
    }
    pub async fn accept(&self, request_id: String, control: bool) -> Result<()> {
        let mut i = self.lock()?;
        if i.snapshot.role != Role::Host
            || i.snapshot.state != SessionState::Pending
            || i.snapshot.request_id.as_deref() != Some(&request_id)
            || i.pending_deadline
                .is_none_or(|deadline| tokio::time::Instant::now() >= deadline)
        {
            return Err(Error::InvalidState);
        }
        i.decision
            .take()
            .ok_or(Error::InvalidState)?
            .send(Some(control))
            .map_err(|_| Error::InvalidState)
    }
    pub async fn reject(&self, request_id: String) -> Result<()> {
        let _op = self.lifecycle.lock().await;
        let mut i = self.lock()?;
        if i.snapshot.role != Role::Host
            || i.snapshot.state != SessionState::Pending
            || i.snapshot.request_id.as_deref() != Some(&request_id)
            || i.pending_deadline
                .is_none_or(|deadline| tokio::time::Instant::now() >= deadline)
        {
            return Err(Error::InvalidState);
        }
        i.decision
            .take()
            .ok_or(Error::InvalidState)?
            .send(None)
            .map_err(|_| Error::InvalidState)?;
        i.snapshot.state = SessionState::Closed;
        Ok(())
    }
    pub fn publish_frame(&self, bytes: Vec<u8>) -> Result<()> {
        use image::ImageDecoder;
        wire::validate_frame(&bytes)?;
        let decoder = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(&bytes))
            .map_err(|_| Error::InvalidFrame)?;
        let (width, height) = decoder.dimensions();
        let request_id = self.snapshot().request_id.ok_or(Error::InvalidState)?;
        self.publish_video(
            request_id,
            VideoFrame {
                codec: VideoCodec::Jpeg,
                width,
                height,
                timestamp_us: 0,
                keyframe: true,
                sequence: 0,
                screen_epoch: 0,
                bytes,
            },
        )
    }
    pub fn latest_frame(&self) -> Option<Vec<u8>> {
        self.lock().ok().and_then(|i| {
            if i.snapshot.state == SessionState::Active {
                i.frame.clone()
            } else {
                None
            }
        })
    }
    /// Current board for the active approved session; independent from UI session status.
    pub fn annotations(&self) -> AnnotationSnapshot {
        self.lock()
            .map(|i| i.annotations.clone())
            .unwrap_or_default()
    }
    /// Host serializes both participants' commands. Viewer changes become visible
    /// when the host's ordered echo arrives. A full queue never mutates the board.
    pub async fn annotate(&self, request_id: String, command: AnnotationCommand) -> Result<()> {
        let epoch = self.snapshot().screen_epoch;
        self.annotate_at(request_id, epoch, command).await
    }
    pub async fn annotate_at(
        &self,
        request_id: String,
        screen_epoch: u64,
        command: AnnotationCommand,
    ) -> Result<()> {
        command.validate()?;
        let mut i = self.lock()?;
        if i.snapshot.state != SessionState::Active
            || i.snapshot.request_id.as_deref() != Some(&request_id)
        {
            return Err(Error::InvalidState);
        }
        if screen_epoch != i.snapshot.screen_epoch {
            return Ok(());
        }
        match i.snapshot.role {
            Role::Host => Self::apply_annotation(&mut i, AnnotationAuthor::Host, command),
            Role::Viewer => i
                .annotation_outbound
                .as_ref()
                .ok_or(Error::InvalidState)?
                .try_send(Control::AnnotationCommand {
                    screen_epoch,
                    command,
                })
                .map_err(|_| Error::Backpressure),
            Role::Idle => Err(Error::InvalidState),
        }
    }
    fn apply_annotation(
        i: &mut Inner,
        author: AnnotationAuthor,
        command: AnnotationCommand,
    ) -> Result<()> {
        let event = AnnotationEvent {
            revision: i
                .annotations
                .revision
                .checked_add(1)
                .ok_or(Error::Protocol)?,
            author,
            command,
        };
        let mut next = i.annotations.clone();
        next.apply(&event)?;
        i.annotation_outbound
            .as_ref()
            .ok_or(Error::InvalidState)?
            .try_send(Control::AnnotationEvent { event })
            .map_err(|_| Error::Backpressure)?;
        i.annotations = next;
        Ok(())
    }
    /// Audio capture is controlled only by the host and independent of keyboard permission.
    pub async fn set_audio_enabled(&self, request_id: String, enabled: bool) -> Result<()> {
        let mut i = self.lock()?;
        if i.snapshot.state != SessionState::Active
            || i.snapshot.request_id.as_deref() != Some(&request_id)
        {
            return Err(Error::InvalidState);
        }
        if i.snapshot.role != Role::Host {
            return Err(Error::PermissionDenied);
        }
        if i.snapshot.audio_enabled == enabled {
            return Ok(());
        }
        i.snapshot.audio_epoch = i
            .snapshot
            .audio_epoch
            .checked_add(1)
            .ok_or(Error::Protocol)?;
        i.snapshot.audio_enabled = enabled;
        i.audio.clear();
        i.audio.notify.notify_one();
        Ok(())
    }
    /// Accept one native 20ms PCM block, discarding oldest queued blocks under load.
    pub fn publish_audio(&self, request_id: String, packet: AudioPacket) -> Result<()> {
        packet.validate()?;
        let mut i = self.lock()?;
        if i.snapshot.state != SessionState::Active
            || i.snapshot.request_id.as_deref() != Some(&request_id)
        {
            return Err(Error::InvalidState);
        }
        if i.snapshot.role != Role::Host || !i.snapshot.audio_enabled {
            return Err(Error::PermissionDenied);
        }
        let epoch = i.snapshot.audio_epoch;
        i.audio.push(epoch, packet)
    }
    /// Drain at most 200ms of audio. Consumers must flush native playback whenever
    /// snapshot.audio_epoch changes or the session stops.
    pub fn take_audio(&self, request_id: String) -> Result<Vec<AudioPacket>> {
        Ok(self.take_audio_batch(request_id)?.packets)
    }
    /// Atomic playback epoch plus packets for IPC consumers. Do not pair a drain
    /// with a separately read snapshot: revocation can race the two reads.
    pub fn take_audio_batch(&self, request_id: String) -> Result<AudioBatch> {
        let mut i = self.lock()?;
        if i.snapshot.state != SessionState::Active
            || i.snapshot.request_id.as_deref() != Some(&request_id)
        {
            return Err(Error::InvalidState);
        }
        if i.snapshot.role != Role::Viewer {
            return Err(Error::PermissionDenied);
        }
        Ok(AudioBatch {
            request_id,
            audio_epoch: i.snapshot.audio_epoch,
            audio_enabled: i.snapshot.audio_enabled,
            packets: i.audio.incoming.drain(..).collect(),
        })
    }
    async fn next_audio_packet(
        &self,
        generation: u64,
        sent_epoch: u64,
        notify: &tokio::sync::Notify,
    ) -> Result<Packet> {
        loop {
            {
                let mut i = self.lock()?;
                if i.generation != generation || i.snapshot.state != SessionState::Active {
                    return Err(Error::InvalidState);
                }
                if i.snapshot.audio_epoch != sent_epoch {
                    return Ok(Packet::Control(Control::AudioStatus {
                        epoch: i.snapshot.audio_epoch,
                        enabled: i.snapshot.audio_enabled,
                    }));
                }
                if let Some(packet) = i.audio.outgoing.pop_front() {
                    return Ok(Packet::Audio(packet));
                }
            }
            notify.notified().await;
        }
    }
    pub async fn send_input(&self, event: InputEvent) -> Result<()> {
        let snapshot = self.snapshot();
        self.send_input_at(
            snapshot.request_id.ok_or(Error::InvalidState)?,
            snapshot.screen_epoch,
            event,
        )
        .await
    }
    pub async fn send_input_at(
        &self,
        request_id: String,
        screen_epoch: u64,
        event: InputEvent,
    ) -> Result<()> {
        event.validate()?;
        let (generation, failed) = {
            let i = self.lock()?;
            tools::active(&i, &request_id)?;
            if i.snapshot.role != Role::Viewer || !i.snapshot.control {
                return Err(Error::PermissionDenied);
            }
            if !matches!(event, InputEvent::ReleaseAll)
                && (screen_epoch != i.snapshot.screen_epoch || i.snapshot.screen_pending)
            {
                // The native snapshot can advance before the UI learns about a
                // monitor transition. Discard stale coordinates without ending a
                // healthy session; release-all must still reach the host.
                return Ok(());
            }
            (
                i.generation,
                i.outbound
                    .as_ref()
                    .ok_or(Error::InvalidState)?
                    .try_send((screen_epoch, event))
                    .is_err(),
            )
        };
        if failed {
            let _op = self.lifecycle.lock().await;
            let still_current = { self.lock()?.generation == generation };
            if still_current {
                self.shutdown_current().await?;
            }
            return Err(Error::Backpressure);
        }
        Ok(())
    }
    pub fn take_input(&self) -> Vec<InputEvent> {
        if let Ok(mut i) = self.lock() {
            if i.snapshot.role == Role::Host
                && i.snapshot.state == SessionState::Active
                && i.snapshot.control
            {
                return i.inputs.drain(..).collect();
            }
            i.inputs.clear();
        }
        Vec::new()
    }
    fn file_manager(&self, request_id: &str) -> Result<Arc<FileHandle>> {
        let i = self.lock()?;
        if i.snapshot.state != SessionState::Active
            || i.snapshot.request_id.as_deref() != Some(request_id)
        {
            return Err(Error::InvalidState);
        }
        i.files.clone().ok_or(Error::InvalidState)
    }
    pub async fn offer_file(&self, request_id: String, source: PathBuf) -> Result<String> {
        let files = self.file_manager(&request_id)?;
        tokio::task::spawn_blocking(move || files.run(|manager| manager.offer(source)))
            .await
            .map_err(|_| Error::State)?
    }
    pub async fn accept_file(
        &self,
        request_id: String,
        transfer_id: String,
        destination: PathBuf,
    ) -> Result<()> {
        let files = self.file_manager(&request_id)?;
        tokio::task::spawn_blocking(move || {
            files.run(|manager| manager.accept(&transfer_id, destination))
        })
        .await
        .map_err(|_| Error::State)?
    }
    pub async fn reject_file(&self, request_id: String, transfer_id: String) -> Result<()> {
        let files = self.file_manager(&request_id)?;
        tokio::task::spawn_blocking(move || files.run(|manager| manager.reject(&transfer_id)))
            .await
            .map_err(|_| Error::State)?
    }
    pub async fn pause_file(&self, request_id: String, transfer_id: String) -> Result<()> {
        let files = self.file_manager(&request_id)?;
        tokio::task::spawn_blocking(move || files.run(|manager| manager.pause(&transfer_id)))
            .await
            .map_err(|_| Error::State)?
    }
    pub async fn resume_file(&self, request_id: String, transfer_id: String) -> Result<()> {
        let files = self.file_manager(&request_id)?;
        tokio::task::spawn_blocking(move || files.run(|manager| manager.resume(&transfer_id)))
            .await
            .map_err(|_| Error::State)?
    }
    pub async fn cancel_file(&self, request_id: String, transfer_id: String) -> Result<()> {
        let files = self.file_manager(&request_id)?;
        tokio::task::spawn_blocking(move || files.run(|manager| manager.cancel(&transfer_id)))
            .await
            .map_err(|_| Error::State)?
    }
    fn transition(&self, generation: u64, update: impl FnOnce(&mut Inner)) -> Result<()> {
        let mut i = self.lock()?;
        if i.generation != generation {
            return Err(Error::InvalidState);
        }
        update(&mut i);
        Ok(())
    }
    async fn host_loop(
        self: &Arc<Self>,
        generation: u64,
        listener: TcpListener,
        acceptor: TlsAcceptor,
        token: [u8; 32],
        name: String,
    ) -> Result<()> {
        for _ in 0..16 {
            let (tcp, _) = listener.accept().await?;
            tcp.set_nodelay(true)?;
            let handshake = timeout(HANDSHAKE_TIMEOUT, async {
                let mut tls = acceptor.accept(tcp).await.map_err(|_| Error::Tls)?;
                let peer = read_hello(&mut tls, &token).await?;
                Ok::<_, Error>((tls, peer))
            })
            .await;
            if let Ok(Ok((tls, peer))) = handshake {
                drop(listener);
                return self.host_session(generation, tls, peer, name).await;
            }
        }
        Err(Error::Authentication)
    }
    async fn host_session(
        &self,
        generation: u64,
        stream: impl AsyncRead + AsyncWrite + Unpin,
        peer: String,
        name: String,
    ) -> Result<()> {
        let (mut reader, mut writer) = tokio::io::split(stream);
        let request_id = Uuid::new_v4().to_string();
        let (decision, answer) = oneshot::channel();
        let deadline = tokio::time::Instant::now() + PENDING_TIMEOUT;
        self.transition(generation, |i| {
            i.snapshot.state = SessionState::Pending;
            i.snapshot.peer_name = Some(peer);
            i.snapshot.request_id = Some(request_id.clone());
            i.decision = Some(decision);
            i.pending_deadline = Some(deadline);
        })?;
        timeout(
            WRITE_TIMEOUT,
            wire::control(&mut writer, &Control::Waiting { request_id, name }),
        )
        .await
        .map_err(|_| Error::Timeout)??;
        let auto_accept = {
            let i = self.lock()?;
            if i.generation != generation {
                return Err(Error::InvalidState);
            }
            i.auto_accept
        };
        let decision = if let Some(control) = auto_accept {
            Some(control)
        } else {
            tokio::select! {result=tokio::time::timeout_at(deadline,answer)=>result.map_err(|_|Error::Timeout)?.map_err(|_|Error::InvalidState)?,_=wire::read(&mut reader)=>return Ok(())}
        };
        let Some(control) = decision else {
            timeout(
                WRITE_TIMEOUT,
                wire::control(&mut writer, &Control::Rejected),
            )
            .await
            .map_err(|_| Error::Timeout)??;
            return Ok(());
        };
        timeout(
            WRITE_TIMEOUT,
            wire::control(&mut writer, &Control::Accepted { control }),
        )
        .await
        .map_err(|_| Error::Timeout)??;
        self.active_session(generation, reader.unsplit(writer), Role::Host, control)
            .await
    }
    async fn viewer_connect(
        &self,
        generation: u64,
        invite: Invitation,
        name: String,
        config: rustls::ClientConfig,
    ) -> Result<()> {
        let mut stream = timeout(HANDSHAKE_TIMEOUT, async {
            let transport: Box<dyn Transport> = if let Some(relay) = &invite.relay {
                Box::new(
                    assi_relay::join(&relay.config, &relay.room, &relay.ticket)
                        .await
                        .map_err(|_| Error::Relay)?,
                )
            } else {
                let tcp = TcpStream::connect(invite.endpoint).await?;
                tcp.set_nodelay(true)?;
                Box::new(tcp)
            };
            TlsConnector::from(Arc::new(config))
                .connect(
                    ServerName::try_from("assi.local").map_err(|_| Error::Tls)?,
                    transport,
                )
                .await
                .map_err(|_| Error::Tls)
        })
        .await
        .map_err(|_| Error::Timeout)??;
        timeout(
            WRITE_TIMEOUT,
            wire::control(
                &mut stream,
                &Control::Hello {
                    token: invite.token,
                    name,
                },
            ),
        )
        .await
        .map_err(|_| Error::Timeout)??;
        match timeout(HANDSHAKE_TIMEOUT, wire::read(&mut stream))
            .await
            .map_err(|_| Error::Timeout)??
        {
            Packet::Control(Control::Waiting { request_id, name }) if name_ok(&name) => self
                .transition(generation, |i| {
                    i.snapshot.state = SessionState::Pending;
                    i.snapshot.peer_name = Some(name);
                    i.snapshot.request_id = Some(request_id);
                })?,
            _ => return Err(Error::Protocol),
        }
        let control = match timeout(PENDING_TIMEOUT + HANDSHAKE_TIMEOUT, wire::read(&mut stream))
            .await
            .map_err(|_| Error::Timeout)??
        {
            Packet::Control(Control::Accepted { control }) => control,
            Packet::Control(Control::Rejected) => return Ok(()),
            _ => return Err(Error::Protocol),
        };
        self.active_session(generation, stream, Role::Viewer, control)
            .await
    }
    async fn active_session(
        &self,
        generation: u64,
        stream: impl AsyncRead + AsyncWrite + Unpin,
        role: Role,
        control: bool,
    ) -> Result<()> {
        let (frames, _incoming) = watch::channel(None);
        let (outbound, mut events) = mpsc::channel(MAX_INPUTS);
        let (video_tx, mut video_rx) = mpsc::channel(MAX_VIDEO_FRAMES);
        let (annotation_outbound, mut annotation_events) = mpsc::channel(32);
        let files = FileHandle::new();
        let audio = AudioState::default();
        let audio_notify = audio.notify.clone();
        self.transition(generation, |i| {
            i.snapshot.state = SessionState::Active;
            i.pending_deadline = None;
            i.snapshot.control = control;
            i.frames = Some(frames);
            i.outbound = Some(outbound);
            i.annotation_outbound = Some(annotation_outbound);
            i.annotations = AnnotationSnapshot {
                request_id: i.snapshot.request_id.clone(),
                ..AnnotationSnapshot::default()
            };
            i.files = Some(files.clone());
            i.audio = audio;
            i.video.sender = Some(video_tx);
            i.display.acknowledged_epoch = i.snapshot.screen_epoch;
        })?;
        let (file_tx, mut file_rx) = mpsc::channel(2);
        let manager = files.clone();
        let pump = FilePump(tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(20));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let Ok(permit) = file_tx.reserve().await else {
                    return;
                };
                let manager = manager.clone();
                let result = tokio::task::spawn_blocking(move || {
                    manager.run(|manager| manager.next_packet())
                })
                .await;
                match result {
                    Ok(Ok(Some(packet))) => permit.send(packet),
                    Ok(Ok(None)) => {}
                    _ => return,
                }
            }
        }));
        let (mut reader, mut writer) = tokio::io::split(stream);
        let read = async {
            loop {
                let packet = timeout(IDLE_TIMEOUT, wire::read(&mut reader))
                    .await
                    .map_err(|_| Error::Timeout)??;
                match packet {
                    Packet::Control(Control::Input {
                        screen_epoch,
                        event,
                    }) if role == Role::Host => {
                        event.validate()?;
                        let mut i = self.lock()?;
                        if i.generation != generation {
                            return Err(Error::InvalidState);
                        }
                        if !i.snapshot.control || i.snapshot.state != SessionState::Active {
                            return Err(Error::PermissionDenied);
                        }
                        if !matches!(event, InputEvent::ReleaseAll)
                            && (screen_epoch != i.snapshot.screen_epoch
                                || i.snapshot.screen_pending
                                || i.display.acknowledged_epoch != screen_epoch)
                        {
                            continue;
                        }
                        if i.inputs.len() >= MAX_INPUTS {
                            return Err(Error::Backpressure);
                        }
                        i.inputs.push_back(event);
                    }
                    Packet::Control(Control::AnnotationCommand {
                        screen_epoch,
                        command,
                    }) if role == Role::Host => {
                        let mut i = self.lock()?;
                        if i.generation != generation || i.snapshot.state != SessionState::Active {
                            return Err(Error::InvalidState);
                        }
                        if screen_epoch != i.snapshot.screen_epoch {
                            continue;
                        }
                        Self::apply_annotation(&mut i, AnnotationAuthor::Viewer, command)?;
                    }
                    Packet::Control(Control::AnnotationEvent { event }) if role == Role::Viewer => {
                        let mut i = self.lock()?;
                        if i.generation != generation || i.snapshot.state != SessionState::Active {
                            return Err(Error::InvalidState);
                        }
                        i.annotations.apply(&event)?;
                    }
                    Packet::Control(Control::AudioStatus { epoch, enabled })
                        if role == Role::Viewer =>
                    {
                        let mut i = self.lock()?;
                        if i.generation != generation || i.snapshot.state != SessionState::Active {
                            return Err(Error::InvalidState);
                        }
                        if epoch <= i.snapshot.audio_epoch {
                            return Err(Error::Protocol);
                        }
                        i.snapshot.audio_epoch = epoch;
                        i.snapshot.audio_enabled = enabled;
                        i.audio.clear();
                    }
                    Packet::Audio(envelope) if role == Role::Viewer => {
                        let mut i = self.lock()?;
                        if i.generation != generation || i.snapshot.state != SessionState::Active {
                            return Err(Error::InvalidState);
                        }
                        if envelope.epoch < i.snapshot.audio_epoch {
                            continue;
                        }
                        if envelope.epoch != i.snapshot.audio_epoch || !i.snapshot.audio_enabled {
                            return Err(Error::PermissionDenied);
                        }
                        i.audio.receive(envelope)?;
                    }
                    Packet::Frame(frame) if role == Role::Viewer => {
                        self.transition(generation, |i| i.frame = Some(frame))?
                    }
                    Packet::Video(frame) => self.receive_video(generation, frame)?,
                    Packet::Control(Control::System { event }) => {
                        self.receive_system(generation, event)?
                    }
                    Packet::Control(Control::Terminal { event }) => {
                        self.receive_terminal(generation, event)?
                    }
                    Packet::Control(Control::Tool { message }) => {
                        self.receive_tool(generation, message)?
                    }
                    Packet::Control(Control::Display { message }) => {
                        self.receive_display(generation, message)?
                    }
                    Packet::Control(Control::Video { message }) => {
                        self.receive_video_message(generation, message)?
                    }
                    Packet::Control(Control::Ping) => {}
                    Packet::Control(Control::Close) => return Ok(()),
                    packet => {
                        let files = files.clone();
                        tokio::task::spawn_blocking(move || {
                            files.run(|manager| manager.receive(packet))
                        })
                        .await
                        .map_err(|_| Error::State)??;
                    }
                }
            }
        };
        let write = async {
            let mut sent_audio_epoch = 0;
            let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
            heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                let packet = tokio::select! {
                    biased;
                    _=heartbeat.tick()=>Packet::Control(Control::Ping),
                    event=events.recv(),if role==Role::Viewer=>{let (screen_epoch,event)=event.ok_or(Error::InvalidState)?;Packet::Control(Control::Input{screen_epoch,event})},
                    event=annotation_events.recv()=>Packet::Control(event.ok_or(Error::InvalidState)?),
                    packet=self.next_audio_packet(generation,sent_audio_epoch,&audio_notify),if role==Role::Host=>packet?,
                    packet=async {
                        tokio::select! {
                            frame=video_rx.recv(),if role==Role::Host=>Ok(Packet::Video(frame.ok_or(Error::InvalidState)?)),
                            packet=file_rx.recv()=>packet.ok_or(Error::State),
                        }
                    }=>packet?,
                };
                match &packet {
                    Packet::Control(Control::Tool {
                        message: tools::ToolMessage::Clipboard { epoch, .. },
                    }) => {
                        let i = self.lock()?;
                        if i.generation != generation
                            || !i.snapshot.clipboard_enabled
                            || *epoch != i.snapshot.clipboard_epoch
                        {
                            continue;
                        }
                    }
                    Packet::Video(frame) => {
                        let i = self.lock()?;
                        if i.generation != generation
                            || frame.screen_epoch != i.snapshot.screen_epoch
                            || (frame.codec == VideoCodec::H264
                                && i.snapshot.video_codec != VideoCodec::H264)
                        {
                            continue;
                        }
                    }
                    Packet::Control(Control::AudioStatus { epoch, .. }) => {
                        sent_audio_epoch = *epoch;
                    }
                    Packet::Audio(envelope) => {
                        let i = self.lock()?;
                        if i.generation != generation
                            || !i.snapshot.audio_enabled
                            || i.snapshot.audio_epoch != envelope.epoch
                        {
                            continue;
                        }
                    }
                    _ => {}
                }
                timeout(WRITE_TIMEOUT, wire::packet(&mut writer, packet))
                    .await
                    .map_err(|_| Error::Timeout)??;
            }
        };
        let result = tokio::select! {result=read=>result,result=write=>result};
        drop(pump);
        result
    }
}
struct FilePump(tokio::task::JoinHandle<()>);
impl Drop for FilePump {
    fn drop(&mut self) {
        self.0.abort();
    }
}

trait Transport: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Transport for T {}
async fn read_hello(stream: &mut (impl AsyncRead + Unpin), token: &[u8; 32]) -> Result<String> {
    match wire::read(stream).await? {
        Packet::Control(Control::Hello {
            token: offered,
            name: peer,
        }) => {
            let secret = URL_SAFE_NO_PAD
                .decode(offered)
                .map_err(|_| Error::Authentication)?;
            if secret.len() != 32 || !bool::from(secret.as_slice().ct_eq(token)) || !name_ok(&peer)
            {
                return Err(Error::Authentication);
            }
            Ok(peer)
        }
        _ => Err(Error::Protocol),
    }
}

#[cfg(test)]
mod cleanup_barrier_tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn unfinished_cleanup_times_out_and_can_be_retried_without_cancelling_it() {
        let engine = SessionEngine::new();
        let (completion, witness) = watch::channel(None);
        engine.lock().unwrap().file_cleanup_signals.push(witness);
        assert!(matches!(
            engine.wait_file_cleanup().await,
            Err(Error::Timeout)
        ));
        assert_eq!(engine.snapshot().state, SessionState::Idle);
        completion.send_replace(Some(true));
        engine.wait_file_cleanup().await.unwrap();
    }
    #[tokio::test]
    async fn aborted_or_failed_cleanup_is_not_reported_as_complete() {
        let engine = SessionEngine::new();
        let (completion, witness) = watch::channel(None);
        engine.lock().unwrap().file_cleanup_signals.push(witness);
        drop(completion);
        assert!(matches!(
            engine.wait_file_cleanup().await,
            Err(Error::State)
        ));
        engine.lock().unwrap().file_cleanup_signals.clear();
        let (_completion, witness) = watch::channel(Some(false));
        engine.lock().unwrap().file_cleanup_signals.push(witness);
        assert!(matches!(
            engine.wait_file_cleanup().await,
            Err(Error::FileIo)
        ));
    }
}
