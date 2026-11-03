use assi_relay::{join, reserve, Identity, Limits, RelayConfig, Server};
use std::{
    net::{IpAddr, Ipv4Addr},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn start_server(limits: Limits) -> Server {
    Server::bind(
        "127.0.0.1:0".parse().unwrap(),
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        Identity::generate().unwrap(),
        limits,
    )
    .await
    .unwrap()
}
async fn until(mut f: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !f() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn tls_pair_carries_both_directions_and_room_is_single_use() {
    let server = start_server(Limits::default()).await;
    let config = server.config();
    assert_eq!(
        RelayConfig::decode(&config.encode().unwrap()).unwrap(),
        config
    );
    let reservation = reserve(&config).await.unwrap();
    let (room, ticket) = (reservation.room.clone(), reservation.ticket.clone());
    let mut viewer = join(&config, &room, &ticket).await.unwrap();
    let mut host = reservation.wait().await.unwrap();
    assert!(join(&config, &room, &ticket).await.is_err());
    viewer.write_all(b"inner TLS client bytes").await.unwrap();
    let mut buf = [0; 22];
    host.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"inner TLS client bytes");
    host.write_all(b"reply").await.unwrap();
    let mut reply = [0; 5];
    viewer.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply, b"reply");
    server.shutdown().await.unwrap();
    assert!(host.read_u8().await.is_err());
}
#[tokio::test]
async fn bad_ticket_does_not_consume_room() {
    let server = start_server(Limits::default()).await;
    let config = server.config();
    let host = reserve(&config).await.unwrap();
    assert!(join(&config, &host.room, &"A".repeat(43)).await.is_err());
    assert_eq!(server.pending_rooms(), 1);
    let viewer = join(&config, &host.room, &host.ticket).await.unwrap();
    let _host = host.wait().await.unwrap();
    drop(viewer);
    server.shutdown().await.unwrap();
}
#[tokio::test]
async fn dropped_host_and_expired_room_release_capacity() {
    let server = start_server(Limits {
        max_rooms: 1,
        room_ttl: Duration::from_millis(150),
        ..Limits::default()
    })
    .await;
    let config = server.config();
    let first = reserve(&config).await.unwrap();
    assert!(reserve(&config).await.is_err());
    drop(first);
    until(|| server.pending_rooms() == 0).await;
    let second = reserve(&config).await.unwrap();
    let room = second.room.clone();
    let ticket = second.ticket.clone();
    tokio::time::sleep(Duration::from_millis(220)).await;
    assert!(second.wait().await.is_err());
    assert!(join(&config, &room, &ticket).await.is_err());
    assert_eq!(server.pending_rooms(), 0);
    let _third = reserve(&config).await.unwrap();
    server.shutdown().await.unwrap();
}
#[tokio::test]
async fn wrong_certificate_rejected_and_shutdown_closes_pending() {
    let server = start_server(Limits::default()).await;
    let mut wrong = server.config();
    let other = start_server(Limits::default()).await;
    wrong.certificate = other.config().certificate;
    assert!(reserve(&wrong).await.is_err());
    let waiting = reserve(&server.config()).await.unwrap();
    server.shutdown().await.unwrap();
    assert!(waiting.wait().await.is_err());
    other.shutdown().await.unwrap();
}
#[tokio::test]
async fn idle_pairs_close() {
    let server = start_server(Limits {
        idle_timeout: Duration::from_millis(150),
        ..Limits::default()
    })
    .await;
    let config = server.config();
    let waiting = reserve(&config).await.unwrap();
    let mut viewer = join(&config, &waiting.room, &waiting.ticket).await.unwrap();
    let _host = waiting.wait().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), viewer.read_u8())
            .await
            .unwrap()
            .is_err()
    );
    server.shutdown().await.unwrap();
}
#[test]
fn identity_persists_and_config_rejects_malformed_addresses() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("identity.json");
    let a = Identity::load_or_create(&path).unwrap();
    let b = Identity::load_or_create(&path).unwrap();
    assert_eq!(a.certificate(), b.certificate());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(RelayConfig::decode("assi-relay://bad").is_err());
}

#[tokio::test]
async fn stalled_handshakes_are_bounded_and_return_capacity() {
    let server = start_server(Limits {
        max_connections: 1,
        handshake_timeout: Duration::from_millis(200),
        ..Limits::default()
    })
    .await;
    let raw = tokio::net::TcpStream::connect(server.local_addr())
        .await
        .unwrap();
    until(|| server.active_connections() == 1).await;
    assert!(reserve(&server.config()).await.is_err());
    drop(raw);
    until(|| server.active_connections() == 0).await;
    let _host = reserve(&server.config()).await.unwrap();
    server.shutdown().await.unwrap();
}

#[test]
fn unsafe_or_corrupt_identity_is_never_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("identity.json");
    std::fs::write(&path, b"not an identity").unwrap();
    assert!(Identity::load_or_create(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"not an identity");
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let link = dir.path().join("link.json");
        symlink(&path, &link).unwrap();
        assert!(Identity::load_or_create(&link).is_err());
    }
}
