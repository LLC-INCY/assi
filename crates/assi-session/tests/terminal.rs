use assi_session::{SessionEngine, SessionState, TerminalCommand};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};
async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(4), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("transition timed out");
}
#[tokio::test]
async fn terminal_permission_input_output_revoke_over_tls() {
    let host = SessionEngine::new();
    let viewer = SessionEngine::new();
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let invite = host
        .start_host(SocketAddr::new(ip, 0), ip, "Host".into())
        .await
        .unwrap();
    viewer.connect(invite, "Viewer".into()).await.unwrap();
    until(|| host.snapshot().state == SessionState::Pending).await;
    let id = host.snapshot().request_id.unwrap();
    host.accept(id.clone(), false).await.unwrap();
    until(|| {
        host.snapshot().state == SessionState::Active
            && viewer.snapshot().state == SessionState::Active
    })
    .await;
    assert!(viewer
        .terminal_input(id.clone(), b"pwd\r".to_vec())
        .is_err());
    viewer.terminal_request(id.clone()).unwrap();
    until(|| host.terminal_snapshot(id.clone()).unwrap().requested).await;
    host.terminal_allow(id.clone(), true).unwrap();
    until(|| viewer.terminal_snapshot(id.clone()).unwrap().enabled).await;
    viewer
        .terminal_input(id.clone(), b"pwd\r".to_vec())
        .unwrap();
    let mut received = Vec::new();
    until(|| {
        received.extend(host.take_terminal_commands(id.clone()).unwrap());
        !received.is_empty()
    })
    .await;
    assert!(matches!(&received[0], TerminalCommand::Input(bytes) if bytes == b"pwd\r"));
    host.terminal_publish(id.clone(), b"/tmp\r\n".to_vec())
        .unwrap();
    let mut output = Vec::new();
    until(|| {
        output.extend(viewer.terminal_snapshot(id.clone()).unwrap().output);
        !output.is_empty()
    })
    .await;
    assert_eq!(output, b"/tmp\r\n");
    host.terminal_allow(id.clone(), false).unwrap();
    until(|| !viewer.terminal_snapshot(id.clone()).unwrap().enabled).await;
    assert!(viewer
        .terminal_input(id.clone(), b"pwd\r".to_vec())
        .is_err());
    host.stop().await.unwrap();
    viewer.stop().await.unwrap();
    assert!(host.terminal_snapshot(id).is_err());
}
