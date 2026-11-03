//! A bounded, self-hosted TLS relay. The returned stream must carry a second,
//! end-to-end authenticated TLS session; this service only pairs byte streams.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::{IpAddr, SocketAddr},
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{oneshot, watch, OwnedSemaphorePermit, Semaphore},
    task::{JoinHandle, JoinSet},
    time::{timeout, Instant},
};
use tokio_rustls::{
    rustls::{
        self,
        pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName},
    },
    TlsAcceptor, TlsConnector,
};

pub type RelayStream = tokio_rustls::client::TlsStream<TcpStream>;
type ServerStream = tokio_rustls::server::TlsStream<TcpStream>;
pub type Result<T> = std::result::Result<T, Error>;
const MAX_MESSAGE: usize = 2048;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_ROOM_TTL: Duration = Duration::from_secs(300);

#[derive(Debug, Error)]
pub enum Error {
    #[error("Некорректная конфигурация relay")]
    Config,
    #[error("Не удалось проверить TLS-сертификат relay или установить соединение")]
    Tls,
    #[error("Relay недоступен или операция была прервана")]
    Io(#[from] std::io::Error),
    #[error("Истекло время ожидания relay")]
    Timeout,
    #[error("Неверный или просроченный запрос relay")]
    Protocol,
    #[error("Relay отклонил запрос: комната недоступна или достигнут лимит")]
    Rejected,
    #[error("Ошибка внутреннего состояния relay")]
    State,
    #[error("Файл идентичности relay повреждён, небезопасен или недоступен")]
    Identity,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayConfig {
    pub endpoint: SocketAddr,
    /// Public certificate only, base64url without padding. Never contains a key.
    pub certificate: String,
}
impl RelayConfig {
    pub fn validate(&self) -> Result<()> {
        if self.endpoint.port() == 0
            || !usable_ip(self.endpoint.ip())
            || self.certificate.len() > 5462
        {
            return Err(Error::Config);
        }
        let cert = URL_SAFE_NO_PAD
            .decode(&self.certificate)
            .map_err(|_| Error::Config)?;
        if cert.is_empty() || cert.len() > 4096 {
            return Err(Error::Config);
        }
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(CertificateDer::from(cert))
            .map_err(|_| Error::Config)?;
        Ok(())
    }
    pub fn encode(&self) -> Result<String> {
        self.validate()?;
        Ok(format!(
            "assi-relay://{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).map_err(|_| Error::Config)?)
        ))
    }
    pub fn decode(text: &str) -> Result<Self> {
        if text.len() > 8192 {
            return Err(Error::Config);
        }
        let value: Self = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(
                    text.trim()
                        .strip_prefix("assi-relay://")
                        .ok_or(Error::Config)?,
                )
                .map_err(|_| Error::Config)?,
        )
        .map_err(|_| Error::Config)?;
        value.validate()?;
        Ok(value)
    }
}
fn usable_ip(ip: IpAddr) -> bool {
    !ip.is_unspecified() && !ip.is_multicast() && !matches!(ip,IpAddr::V4(ip) if ip.is_broadcast())
}

/// Private server identity. Intentionally does not implement Debug or Serialize.
pub struct Identity {
    certificate: Vec<u8>,
    key: Vec<u8>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredIdentity {
    certificate: String,
    key: String,
}
impl Identity {
    pub fn generate() -> Result<Self> {
        let key = rcgen::generate_simple_self_signed(vec!["assi-relay.local".into()])
            .map_err(|_| Error::Tls)?;
        Ok(Self {
            certificate: key.cert.der().to_vec(),
            key: key.signing_key.serialize_der(),
        })
    }
    pub fn certificate(&self) -> &[u8] {
        &self.certificate
    }
    /// Create a private file exclusively, or read an existing regular private file.
    /// On Unix the file must have exactly mode 0600. Parent directory is operator-owned.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        match fs::symlink_metadata(path) {
            Ok(meta) => {
                if !meta.is_file() || meta.len() > 16_384 {
                    return Err(Error::Identity);
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if meta.permissions().mode() & 0o777 != 0o600 {
                        return Err(Error::Identity);
                    }
                }
                let mut bytes = Vec::new();
                fs::File::open(path)
                    .map_err(|_| Error::Identity)?
                    .take(16_385)
                    .read_to_end(&mut bytes)
                    .map_err(|_| Error::Identity)?;
                if bytes.len() > 16_384 {
                    return Err(Error::Identity);
                }
                let stored: StoredIdentity =
                    serde_json::from_slice(&bytes).map_err(|_| Error::Identity)?;
                let identity = Self {
                    certificate: URL_SAFE_NO_PAD
                        .decode(stored.certificate)
                        .map_err(|_| Error::Identity)?,
                    key: URL_SAFE_NO_PAD
                        .decode(stored.key)
                        .map_err(|_| Error::Identity)?,
                };
                identity.server_config()?;
                Ok(identity)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let identity = Self::generate()?;
                let bytes = serde_json::to_vec(&StoredIdentity {
                    certificate: URL_SAFE_NO_PAD.encode(&identity.certificate),
                    key: URL_SAFE_NO_PAD.encode(&identity.key),
                })
                .map_err(|_| Error::Identity)?;
                let mut options = OpenOptions::new();
                options.create_new(true).write(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options.open(path).map_err(|_| Error::Identity)?;
                file.write_all(&bytes)
                    .and_then(|_| file.sync_all())
                    .map_err(|_| Error::Identity)?;
                Ok(identity)
            }
            Err(_) => Err(Error::Identity),
        }
    }
    fn server_config(&self) -> Result<rustls::ServerConfig> {
        rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| Error::Tls)?
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(self.certificate.clone())],
            PrivatePkcs8KeyDer::from(self.key.clone()).into(),
        )
        .map_err(|_| Error::Identity)
    }
}
#[derive(Clone, Debug)]
pub struct Limits {
    pub max_rooms: usize,
    pub max_connections: usize,
    pub room_ttl: Duration,
    pub handshake_timeout: Duration,
    pub idle_timeout: Duration,
    pub max_session_duration: Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_rooms: 128,
            max_connections: 256,
            room_ttl: MAX_ROOM_TTL,
            handshake_timeout: CONNECT_TIMEOUT,
            idle_timeout: Duration::from_secs(60),
            max_session_duration: Duration::from_secs(12 * 3600),
        }
    }
}
impl Limits {
    fn validate(&self) -> Result<()> {
        if self.max_rooms == 0
            || self.max_rooms > 10_000
            || self.max_connections == 0
            || self.max_connections > 20_000
            || self.room_ttl.is_zero()
            || self.room_ttl > MAX_ROOM_TTL
            || self.handshake_timeout.is_zero()
            || self.idle_timeout.is_zero()
            || self.max_session_duration.is_zero()
        {
            Err(Error::Config)
        } else {
            Ok(())
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Message {
    Reserve,
    Join {
        room: String,
        ticket: String,
    },
    Reserved {
        room: String,
        ticket: String,
        ttl_ms: u64,
    },
    Paired,
    Rejected,
}
async fn send(stream: &mut (impl AsyncWrite + Unpin), message: &Message) -> Result<()> {
    let bytes = serde_json::to_vec(message).map_err(|_| Error::Protocol)?;
    if bytes.len() > MAX_MESSAGE {
        return Err(Error::Protocol);
    }
    stream.write_u16(bytes.len() as u16).await?;
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}
async fn receive(stream: &mut (impl AsyncRead + Unpin)) -> Result<Message> {
    let length = usize::from(stream.read_u16().await?);
    if length == 0 || length > MAX_MESSAGE {
        return Err(Error::Protocol);
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes).map_err(|_| Error::Protocol)
}
async fn connect(config: &RelayConfig) -> Result<RelayStream> {
    config.validate()?;
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(
            URL_SAFE_NO_PAD
                .decode(&config.certificate)
                .map_err(|_| Error::Config)?,
        ))
        .map_err(|_| Error::Config)?;
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| Error::Tls)?
    .with_root_certificates(roots)
    .with_no_client_auth();
    timeout(CONNECT_TIMEOUT, async {
        let tcp = TcpStream::connect(config.endpoint).await?;
        tcp.set_nodelay(true)?;
        TlsConnector::from(Arc::new(tls))
            .connect(
                ServerName::try_from("assi-relay.local").map_err(|_| Error::Tls)?,
                tcp,
            )
            .await
            .map_err(|_| Error::Tls)
    })
    .await
    .map_err(|_| Error::Timeout)?
}
/// A pending host socket. Dropping it closes the reservation on the server.
/// ticket is a secret; do not log, persist, or include in Debug output.
pub struct Reservation {
    pub room: String,
    pub ticket: String,
    stream: RelayStream,
    deadline: Instant,
}
impl Reservation {
    pub async fn wait(mut self) -> Result<RelayStream> {
        match tokio::time::timeout_at(self.deadline, receive(&mut self.stream))
            .await
            .map_err(|_| Error::Timeout)??
        {
            Message::Paired => Ok(self.stream),
            Message::Rejected => Err(Error::Rejected),
            _ => Err(Error::Protocol),
        }
    }
}
pub async fn reserve(config: &RelayConfig) -> Result<Reservation> {
    let mut stream = connect(config).await?;
    let message = timeout(CONNECT_TIMEOUT, async {
        send(&mut stream, &Message::Reserve).await?;
        receive(&mut stream).await
    })
    .await
    .map_err(|_| Error::Timeout)??;
    match message {
        Message::Reserved {
            room,
            ticket,
            ttl_ms,
        } if valid_room(&room)
            && valid_ticket(&ticket)
            && ttl_ms > 0
            && ttl_ms <= MAX_ROOM_TTL.as_millis() as u64 =>
        {
            Ok(Reservation {
                room,
                ticket,
                stream,
                deadline: Instant::now() + Duration::from_millis(ttl_ms),
            })
        }
        Message::Rejected => Err(Error::Rejected),
        _ => Err(Error::Protocol),
    }
}
pub async fn join(config: &RelayConfig, room: &str, ticket: &str) -> Result<RelayStream> {
    if !valid_room(room) || !valid_ticket(ticket) {
        return Err(Error::Protocol);
    }
    let mut stream = connect(config).await?;
    let message = timeout(CONNECT_TIMEOUT, async {
        send(
            &mut stream,
            &Message::Join {
                room: room.into(),
                ticket: ticket.into(),
            },
        )
        .await?;
        receive(&mut stream).await
    })
    .await
    .map_err(|_| Error::Timeout)??;
    match message {
        Message::Paired => Ok(stream),
        Message::Rejected => Err(Error::Rejected),
        _ => Err(Error::Protocol),
    }
}
fn valid_room(room: &str) -> bool {
    room.len() == 16 && URL_SAFE_NO_PAD.decode(room).is_ok_and(|v| v.len() == 12)
}
fn valid_ticket(ticket: &str) -> bool {
    ticket.len() == 43 && URL_SAFE_NO_PAD.decode(ticket).is_ok_and(|v| v.len() == 32)
}
fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| Error::State)?;
    Ok(bytes)
}
struct Peer {
    stream: ServerStream,
    _permit: OwnedSemaphorePermit,
}
struct Room {
    ticket: [u8; 32],
    deadline: Instant,
    peer: oneshot::Sender<Peer>,
}
struct Shared {
    rooms: Mutex<HashMap<String, Room>>,
    slots: Arc<Semaphore>,
    limits: Limits,
}
impl Shared {
    fn remove(&self, room: &str, ticket: &[u8; 32]) {
        if let Ok(mut rooms) = self.rooms.lock() {
            if rooms.get(room).is_some_and(|entry| entry.ticket == *ticket) {
                rooms.remove(room);
            }
        }
    }
}
pub struct Server {
    config: RelayConfig,
    local_addr: SocketAddr,
    shared: Arc<Shared>,
    cancel: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}
impl Server {
    pub async fn bind(
        bind: SocketAddr,
        advertised_ip: IpAddr,
        identity: Identity,
        limits: Limits,
    ) -> Result<Self> {
        limits.validate()?;
        if !usable_ip(advertised_ip) {
            return Err(Error::Config);
        }
        let tls = identity.server_config()?;
        let listener = TcpListener::bind(bind).await?;
        let local_addr = listener.local_addr()?;
        let config = RelayConfig {
            endpoint: SocketAddr::new(advertised_ip, local_addr.port()),
            certificate: URL_SAFE_NO_PAD.encode(identity.certificate()),
        };
        let shared = Arc::new(Shared {
            rooms: Mutex::new(HashMap::new()),
            slots: Arc::new(Semaphore::new(limits.max_connections)),
            limits,
        });
        let (cancel, mut stop) = watch::channel(false);
        let tasks_shared = shared.clone();
        let acceptor = TlsAcceptor::from(Arc::new(tls));
        let task = tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            loop {
                tokio::select! {biased;
                    _=stop.changed()=>break,
                    Some(_)=tasks.join_next()=>{},
                    accepted=listener.accept()=>{
                        let Ok((tcp,_))=accepted else {break;};
                        let Ok(permit)=tasks_shared.slots.clone().try_acquire_owned() else {continue;};
                        let shared=tasks_shared.clone();let tls=acceptor.clone();let mut cancel=stop.clone();
                        tasks.spawn(async move{tokio::select!{biased;_=cancel.changed()=>{},_=serve(tcp,tls,shared,permit)=>{}}});
                    }
                }
            }
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
            if let Ok(mut rooms) = tasks_shared.rooms.lock() {
                rooms.clear();
            }
        });
        Ok(Self {
            config,
            local_addr,
            shared,
            cancel,
            task: Some(task),
        })
    }
    pub fn config(&self) -> RelayConfig {
        self.config.clone()
    }
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
    pub fn pending_rooms(&self) -> usize {
        self.shared.rooms.lock().map(|v| v.len()).unwrap_or(0)
    }
    pub fn active_connections(&self) -> usize {
        self.shared.limits.max_connections - self.shared.slots.available_permits()
    }
    pub async fn shutdown(mut self) -> Result<()> {
        let _ = self.cancel.send(true);
        if let Some(task) = self.task.take() {
            task.await.map_err(|_| Error::State)?;
        }
        Ok(())
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.cancel.send(true);
    }
}
struct RoomGuard {
    shared: Arc<Shared>,
    room: String,
    ticket: [u8; 32],
}
impl Drop for RoomGuard {
    fn drop(&mut self) {
        self.shared.remove(&self.room, &self.ticket);
    }
}
async fn serve(
    tcp: TcpStream,
    acceptor: TlsAcceptor,
    shared: Arc<Shared>,
    permit: OwnedSemaphorePermit,
) -> Result<()> {
    tcp.set_nodelay(true)?;
    let (mut stream, message) = timeout(shared.limits.handshake_timeout, async {
        let mut tls = acceptor.accept(tcp).await.map_err(|_| Error::Tls)?;
        let message = receive(&mut tls).await?;
        Ok::<_, Error>((tls, message))
    })
    .await
    .map_err(|_| Error::Timeout)??;
    match message {
        Message::Reserve => {
            let room = URL_SAFE_NO_PAD.encode(random::<12>()?);
            let secret = random::<32>()?;
            let ticket = URL_SAFE_NO_PAD.encode(secret);
            let deadline = Instant::now() + shared.limits.room_ttl;
            let (sender, receiver) = oneshot::channel();
            let inserted = {
                let mut rooms = shared.rooms.lock().map_err(|_| Error::State)?;
                if rooms.len() >= shared.limits.max_rooms || rooms.contains_key(&room) {
                    false
                } else {
                    rooms.insert(
                        room.clone(),
                        Room {
                            ticket: secret,
                            deadline,
                            peer: sender,
                        },
                    );
                    true
                }
            };
            if !inserted {
                timeout(CONNECT_TIMEOUT, send(&mut stream, &Message::Rejected))
                    .await
                    .map_err(|_| Error::Timeout)??;
                return Err(Error::Rejected);
            }
            let _guard = RoomGuard {
                shared: shared.clone(),
                room: room.clone(),
                ticket: secret,
            };
            tokio::time::timeout_at(
                deadline,
                send(
                    &mut stream,
                    &Message::Reserved {
                        room,
                        ticket,
                        ttl_ms: shared.limits.room_ttl.as_millis() as u64,
                    },
                ),
            )
            .await
            .map_err(|_| Error::Timeout)??;
            let mut peer = tokio::select! {biased;_=tokio::time::sleep_until(deadline)=>return Err(Error::Timeout),_=stream.read_u8()=>return Err(Error::Protocol),peer=receiver=>peer.map_err(|_|Error::Rejected)?};
            timeout(CONNECT_TIMEOUT, send(&mut stream, &Message::Paired))
                .await
                .map_err(|_| Error::Timeout)??;
            let _permit = permit;
            timeout(
                shared.limits.max_session_duration,
                proxy(&mut stream, &mut peer.stream, shared.limits.idle_timeout),
            )
            .await
            .map_err(|_| Error::Timeout)?
        }
        Message::Join { room, ticket } => {
            let offered = URL_SAFE_NO_PAD
                .decode(&ticket)
                .map_err(|_| Error::Protocol)?;
            let pairing = {
                let mut rooms = shared.rooms.lock().map_err(|_| Error::State)?;
                let valid = valid_room(&room)
                    && offered.len() == 32
                    && rooms.get(&room).is_some_and(|entry| {
                        entry.deadline > Instant::now()
                            && bool::from(entry.ticket.as_slice().ct_eq(&offered))
                    });
                if valid {
                    rooms.remove(&room)
                } else {
                    None
                }
            };
            let Some(pairing) = pairing else {
                timeout(CONNECT_TIMEOUT, send(&mut stream, &Message::Rejected))
                    .await
                    .map_err(|_| Error::Timeout)??;
                return Err(Error::Rejected);
            };
            timeout(CONNECT_TIMEOUT, send(&mut stream, &Message::Paired))
                .await
                .map_err(|_| Error::Timeout)??;
            pairing
                .peer
                .send(Peer {
                    stream,
                    _permit: permit,
                })
                .map_err(|_| Error::Rejected)?;
            Ok(())
        }
        _ => Err(Error::Protocol),
    }
}
async fn proxy(a: &mut ServerStream, b: &mut ServerStream, idle: Duration) -> Result<()> {
    let (ar, aw) = tokio::io::split(a);
    let (br, bw) = tokio::io::split(b);
    tokio::select! {result=copy(ar,bw,idle)=>result,result=copy(br,aw,idle)=>result}
}
async fn copy(
    mut reader: impl AsyncRead + Unpin,
    mut writer: impl AsyncWrite + Unpin,
    idle: Duration,
) -> Result<()> {
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let count = timeout(idle, reader.read(&mut buffer))
            .await
            .map_err(|_| Error::Timeout)??;
        if count == 0 {
            return Ok(());
        }
        timeout(idle, async {
            writer.write_all(&buffer[..count]).await?;
            writer.flush().await
        })
        .await
        .map_err(|_| Error::Timeout)??;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn malformed_and_oversized_requests_rejected_before_body_allocation() {
        let bytes = ((MAX_MESSAGE + 1) as u16).to_be_bytes();
        assert!(matches!(
            receive(&mut bytes.as_slice()).await,
            Err(Error::Protocol)
        ));
        let mut bytes = Vec::from((16u16).to_be_bytes());
        bytes.extend_from_slice(br#"{"type":"shell"}"#);
        assert!(matches!(
            receive(&mut bytes.as_slice()).await,
            Err(Error::Protocol)
        ));
    }
}
