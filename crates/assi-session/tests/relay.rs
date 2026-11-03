use assi_relay::{Identity, Limits, Server};
use assi_session::{SessionEngine, SessionState, TransferState};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::time::Duration;
async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn nested_tls_relay_requires_consent_and_transfers_file() {
    let server = Server::bind(
        "127.0.0.1:0".parse().unwrap(),
        "127.0.0.1".parse().unwrap(),
        Identity::generate().unwrap(),
        Limits::default(),
    )
    .await
    .unwrap();
    let host = SessionEngine::new();
    let viewer = SessionEngine::new();
    let invitation = host
        .start_host_relay(server.config(), "Host".into())
        .await
        .unwrap();
    viewer.connect(invitation, "Viewer".into()).await.unwrap();
    until(|| host.snapshot().state == SessionState::Pending).await;
    assert!(host.latest_frame().is_none());
    let request = host.snapshot().request_id.unwrap();
    host.accept(request.clone(), false).await.unwrap();
    until(|| {
        viewer.snapshot().state == SessionState::Active
            && host.snapshot().state == SessionState::Active
    })
    .await;
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
        .encode(&[0, 70, 20], 1, 1, image::ExtendedColorType::Rgb8)
        .unwrap();
    host.publish_frame(jpeg.clone()).unwrap();
    until(|| viewer.latest_frame().is_some()).await;
    assert_eq!(viewer.latest_frame().unwrap(), jpeg);
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("test.txt");
    std::fs::write(&source, b"nested TLS file").unwrap();
    let id = host.offer_file(request.clone(), source).await.unwrap();
    until(|| viewer.snapshot().transfer.is_some()).await;
    let target = dir.path().join("received.txt");
    viewer
        .accept_file(request, id, target.clone())
        .await
        .unwrap();
    until(|| {
        host.snapshot()
            .transfer
            .is_some_and(|x| x.state == TransferState::Completed)
    })
    .await;
    assert_eq!(std::fs::read(target).unwrap(), b"nested TLS file");
    host.stop().await.unwrap();
    viewer.stop().await.unwrap();
    server.shutdown().await.unwrap();
}
#[tokio::test]
async fn relay_cannot_replace_inner_certificate() {
    let server = Server::bind(
        "127.0.0.1:0".parse().unwrap(),
        "127.0.0.1".parse().unwrap(),
        Identity::generate().unwrap(),
        Limits::default(),
    )
    .await
    .unwrap();
    let host = SessionEngine::new();
    let viewer = SessionEngine::new();
    let invitation = host
        .start_host_relay(server.config(), "Host".into())
        .await
        .unwrap();
    let mut parsed: serde_json::Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(invitation.strip_prefix("assi://").unwrap())
            .unwrap(),
    )
    .unwrap();
    let unrelated = rcgen::generate_simple_self_signed(vec!["assi.local".into()]).unwrap();
    parsed["certificate"] = URL_SAFE_NO_PAD.encode(unrelated.cert.der()).into();
    let forged = format!(
        "assi://{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&parsed).unwrap())
    );
    viewer.connect(forged, "Viewer".into()).await.unwrap();
    until(|| viewer.snapshot().state == SessionState::Error).await;
    assert_ne!(host.snapshot().state, SessionState::Pending);
    host.stop().await.unwrap();
    viewer.stop().await.unwrap();
    server.shutdown().await.unwrap();
}
