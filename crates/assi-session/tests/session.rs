use assi_session::{Error, InputEvent, Key, SessionEngine, SessionState};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(4), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("state transition timed out");
}
fn jpeg() -> Vec<u8> {
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut bytes)
        .encode(&[24, 92, 82], 1, 1, image::ExtendedColorType::Rgb8)
        .unwrap();
    bytes
}
async fn host() -> (Arc<SessionEngine>, String) {
    let engine = SessionEngine::new();
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let invitation = engine
        .start_host(SocketAddr::new(ip, 0), ip, "Host".into())
        .await
        .unwrap();
    (engine, invitation)
}
async fn pair() -> (Arc<SessionEngine>, Arc<SessionEngine>) {
    let (host, invitation) = host().await;
    let viewer = SessionEngine::new();
    viewer.connect(invitation, "Viewer".into()).await.unwrap();
    until(|| {
        host.snapshot().state == SessionState::Pending
            && viewer.snapshot().state == SessionState::Pending
    })
    .await;
    (host, viewer)
}
async fn approve(host: &SessionEngine, viewer: &SessionEngine, control: bool) {
    host.accept(host.snapshot().request_id.unwrap(), control)
        .await
        .unwrap();
    until(|| {
        host.snapshot().state == SessionState::Active
            && viewer.snapshot().state == SessionState::Active
    })
    .await;
}
#[tokio::test]
async fn tls_viewing_requires_consent_transports_frame_and_revokes_on_stop() {
    let (host, viewer) = pair().await;
    assert!(matches!(
        host.publish_frame(jpeg()),
        Err(Error::InvalidState)
    ));
    assert!(matches!(
        host.accept("stale-request".into(), true).await,
        Err(Error::InvalidState)
    ));
    assert!(viewer
        .send_input(InputEvent::Move { x: 0.5, y: 0.5 })
        .await
        .is_err());
    approve(&host, &viewer, false).await;
    assert!(matches!(
        viewer.send_input(InputEvent::Move { x: 0.5, y: 0.5 }).await,
        Err(Error::PermissionDenied)
    ));
    let frame = jpeg();
    host.publish_frame(frame.clone()).unwrap();
    until(|| viewer.latest_frame().is_some()).await;
    assert_eq!(viewer.latest_frame().unwrap(), frame);
    assert!(host.take_input().is_empty());
    host.stop().await.unwrap();
    until(|| {
        matches!(
            viewer.snapshot().state,
            SessionState::Closed | SessionState::Error
        )
    })
    .await;
    assert!(!viewer.snapshot().control);
    assert!(viewer.latest_frame().is_none());
}
#[tokio::test]
async fn permitted_input_is_bounded_and_cleared_on_disconnect() {
    let (host, viewer) = pair().await;
    approve(&host, &viewer, true).await;
    assert!(matches!(
        viewer
            .send_input(InputEvent::Move {
                x: f64::NAN,
                y: 0.0
            })
            .await,
        Err(Error::InvalidInput)
    ));
    viewer
        .send_input(InputEvent::Key {
            key: Key::Control,
            down: true,
        })
        .await
        .unwrap();
    let mut received = Vec::new();
    until(|| {
        received.extend(host.take_input());
        !received.is_empty()
    })
    .await;
    assert!(matches!(
        received[0],
        InputEvent::Key {
            key: Key::Control,
            down: true
        }
    ));
    viewer
        .send_input(InputEvent::Move { x: 0.3, y: 0.8 })
        .await
        .unwrap();
    host.stop().await.unwrap();
    assert!(host.take_input().is_empty());
    assert!(!host.snapshot().control);
    until(|| viewer.snapshot().state != SessionState::Active).await;
    assert!(viewer
        .send_input(InputEvent::Key {
            key: Key::Control,
            down: false
        })
        .await
        .is_err());
}
#[tokio::test]
async fn rejection_invalidates_request_and_no_frames_are_available() {
    let (host, viewer) = pair().await;
    let request = host.snapshot().request_id.unwrap();
    host.reject(request.clone()).await.unwrap();
    assert!(host.accept(request, true).await.is_err());
    until(|| {
        matches!(
            viewer.snapshot().state,
            SessionState::Closed | SessionState::Error
        )
    })
    .await;
    assert!(viewer.latest_frame().is_none());
    assert!(!viewer.snapshot().control);
}
fn mutate(invite: &str, field: &str, value: String) -> String {
    let bytes = URL_SAFE_NO_PAD
        .decode(invite.strip_prefix("assi://").unwrap())
        .unwrap();
    let mut json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    json[field] = value.into();
    format!(
        "assi://{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json).unwrap())
    )
}
#[tokio::test]
async fn wrong_token_never_reaches_owner_consent() {
    let (host, invite) = host().await;
    let viewer = SessionEngine::new();
    viewer
        .connect(
            mutate(&invite, "token", URL_SAFE_NO_PAD.encode([0u8; 32])),
            "Viewer".into(),
        )
        .await
        .unwrap();
    until(|| viewer.snapshot().state == SessionState::Error).await;
    assert_eq!(host.snapshot().state, SessionState::Listening);
    assert!(host.snapshot().request_id.is_none());
    host.stop().await.unwrap();
}
#[tokio::test]
async fn different_certificate_is_not_trusted() {
    let (host, invite) = host().await;
    let viewer = SessionEngine::new();
    let other = rcgen::generate_simple_self_signed(vec!["assi.local".into()]).unwrap();
    viewer
        .connect(
            mutate(
                &invite,
                "certificate",
                URL_SAFE_NO_PAD.encode(other.cert.der()),
            ),
            "Viewer".into(),
        )
        .await
        .unwrap();
    until(|| viewer.snapshot().state == SessionState::Error).await;
    assert_eq!(host.snapshot().state, SessionState::Listening);
    assert!(host.snapshot().request_id.is_none());
    host.stop().await.unwrap();
}
#[tokio::test]
async fn invalid_invitation_and_frame_limits_are_rejected() {
    let (host, invite) = host().await;
    let viewer = SessionEngine::new();
    for endpoint in [
        "127.0.0.1:0",
        "0.0.0.0:12",
        "224.0.0.1:12",
        "example.org:12",
    ] {
        assert!(matches!(
            viewer
                .connect(
                    mutate(&invite, "endpoint", endpoint.into()),
                    "Viewer".into()
                )
                .await,
            Err(Error::InvalidInvitation)
        ));
    }
    assert!(viewer
        .connect("assi://".repeat(4096), "Viewer".into())
        .await
        .is_err());
    assert!(matches!(
        host.publish_frame(vec![0; 4 * 1024 * 1024 + 1]),
        Err(Error::InvalidFrame)
    ));
    assert!(matches!(
        host.publish_frame(vec![0xff, 0xd8, 0xff, 0xd9]),
        Err(Error::InvalidFrame)
    ));
    host.stop().await.unwrap();
}
#[tokio::test]
async fn previous_connection_cannot_clear_new_listener() {
    let (host, viewer) = pair().await;
    approve(&host, &viewer, true).await;
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    host.start_host(SocketAddr::new(ip, 0), ip, "New host".into())
        .await
        .unwrap();
    until(|| viewer.snapshot().state != SessionState::Active).await;
    assert_eq!(host.snapshot().state, SessionState::Listening);
    assert!(!host.snapshot().control);
    assert!(host.take_input().is_empty());
    host.stop().await.unwrap();
}
#[tokio::test]
async fn unanswered_consent_expires_and_cannot_be_accepted_later() {
    let (host, viewer) = pair().await;
    let request = host.snapshot().request_id.unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    tokio::task::yield_now().await;
    assert_eq!(host.snapshot().state, SessionState::Error);
    assert!(host.accept(request, true).await.is_err());
    assert!(!host.snapshot().control);
    tokio::time::resume();
    until(|| {
        matches!(
            viewer.snapshot().state,
            SessionState::Error | SessionState::Closed
        )
    })
    .await;
}
#[tokio::test]
async fn input_backlog_closes_session_instead_of_dropping_key_releases() {
    let (host, viewer) = pair().await;
    approve(&host, &viewer, true).await;
    let mut overflow = false;
    for _ in 0..1024 {
        if viewer
            .send_input(InputEvent::Key {
                key: Key::Shift,
                down: true,
            })
            .await
            .is_err()
        {
            overflow = true;
            break;
        }
    }
    assert!(overflow);
    assert!(!viewer.snapshot().control);
    until(|| host.snapshot().state != SessionState::Active).await;
    assert!(!host.snapshot().control);
    assert!(host.take_input().is_empty());
}
#[tokio::test]
async fn stopping_releases_listener_before_returning() {
    let (host, invite) = host().await;
    let raw = URL_SAFE_NO_PAD
        .decode(invite.strip_prefix("assi://").unwrap())
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let endpoint: SocketAddr = json["endpoint"].as_str().unwrap().parse().unwrap();
    host.stop().await.unwrap();
    host.start_host(endpoint, endpoint.ip(), "Restarted".into())
        .await
        .expect("stop must release the listening port");
    host.stop().await.unwrap();
}
#[tokio::test]
async fn release_all_travels_only_with_control_permission() {
    let (host, viewer) = pair().await;
    approve(&host, &viewer, true).await;
    viewer.send_input(InputEvent::ReleaseAll).await.unwrap();
    let mut events = Vec::new();
    until(|| {
        events.extend(host.take_input());
        !events.is_empty()
    })
    .await;
    assert!(matches!(events[0], InputEvent::ReleaseAll));
    host.stop().await.unwrap();
    let (host, viewer) = pair().await;
    approve(&host, &viewer, false).await;
    assert!(matches!(
        viewer.send_input(InputEvent::ReleaseAll).await,
        Err(Error::PermissionDenied)
    ));
    host.stop().await.unwrap();
}
#[test]
fn finite_keys_have_stable_wire_names() {
    assert!(matches!(
        serde_json::from_str::<Key>("\"a\"").unwrap(),
        Key::A
    ));
    assert!(matches!(
        serde_json::from_str::<Key>("\"9\"").unwrap(),
        Key::Digit9
    ));
    assert!(matches!(
        serde_json::from_str::<Key>("\"arrow_up\"").unwrap(),
        Key::ArrowUp
    ));
    for (key, name) in [
        (Key::F1, "f1"),
        (Key::F12, "f12"),
        (Key::CapsLock, "caps_lock"),
        (Key::Insert, "insert"),
    ] {
        assert_eq!(serde_json::to_value(key).unwrap(), serde_json::json!(name));
    }
    assert!(serde_json::from_str::<Key>("\"unknown\"").is_err());
    assert_eq!(
        serde_json::to_value(InputEvent::ReleaseAll).unwrap(),
        serde_json::json!({"type":"release_all"})
    );
}
#[tokio::test]
async fn host_backlog_terminates_when_platform_does_not_drain() {
    let (host, viewer) = pair().await;
    approve(&host, &viewer, true).await;
    for _ in 0..129 {
        viewer.send_input(InputEvent::ReleaseAll).await.unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    until(|| host.snapshot().state == SessionState::Error).await;
    assert!(!host.snapshot().control);
    assert!(host.take_input().is_empty());
    until(|| viewer.snapshot().state != SessionState::Active).await;
}
#[tokio::test]
async fn hostile_viewer_cannot_bypass_host_input_permission() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::{
        rustls::{
            self,
            pki_types::{CertificateDer, ServerName},
        },
        TlsConnector,
    };
    let (host, invite) = host().await;
    let raw = URL_SAFE_NO_PAD
        .decode(invite.strip_prefix("assi://").unwrap())
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let endpoint: SocketAddr = json["endpoint"].as_str().unwrap().parse().unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(
            URL_SAFE_NO_PAD
                .decode(json["certificate"].as_str().unwrap())
                .unwrap(),
        ))
        .unwrap();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let tcp = tokio::net::TcpStream::connect(endpoint).await.unwrap();
    let mut tls = TlsConnector::from(Arc::new(config))
        .connect(ServerName::try_from("assi.local").unwrap(), tcp)
        .await
        .unwrap();
    let hello = serde_json::to_vec(
        &serde_json::json!({"type":"hello","token":json["token"],"name":"Viewer"}),
    )
    .unwrap();
    tls.write_u32((hello.len() + 1) as u32).await.unwrap();
    tls.write_u8(1).await.unwrap();
    tls.write_all(&hello).await.unwrap();
    tls.flush().await.unwrap();
    let length = tls.read_u32().await.unwrap();
    let mut waiting = vec![0; length as usize];
    tls.read_exact(&mut waiting).await.unwrap();
    host.accept(host.snapshot().request_id.unwrap(), false)
        .await
        .unwrap();
    let length = tls.read_u32().await.unwrap();
    let mut accepted = vec![0; length as usize];
    tls.read_exact(&mut accepted).await.unwrap();
    let input = br#"{"type":"input","event":{"type":"key","key":"a","down":true}}"#;
    tls.write_u32((input.len() + 1) as u32).await.unwrap();
    tls.write_u8(1).await.unwrap();
    tls.write_all(input).await.unwrap();
    tls.flush().await.unwrap();
    until(|| host.snapshot().state == SessionState::Error).await;
    assert!(!host.snapshot().control);
    assert!(host.take_input().is_empty());
    host.stop().await.unwrap();
}
