use assi_session::{AudioPacket, Error, InputEvent, Key, SessionEngine, SessionState};
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
    .expect("transition timed out");
}
async fn pair(control: bool) -> (Arc<SessionEngine>, Arc<SessionEngine>, String) {
    let host = SessionEngine::new();
    let viewer = SessionEngine::new();
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let invitation = host
        .start_host(SocketAddr::new(ip, 0), ip, "Host".into())
        .await
        .unwrap();
    viewer.connect(invitation, "Viewer".into()).await.unwrap();
    until(|| host.snapshot().state == SessionState::Pending).await;
    let id = host.snapshot().request_id.unwrap();
    host.accept(id.clone(), control).await.unwrap();
    until(|| {
        host.snapshot().state == SessionState::Active
            && viewer.snapshot().state == SessionState::Active
    })
    .await;
    (host, viewer, id)
}
fn audio(value: u8, channels: u8) -> AudioPacket {
    AudioPacket {
        sample_rate: 48_000,
        channels,
        pcm: vec![value; 960 * usize::from(channels) * 2],
    }
}
#[tokio::test]
async fn host_enables_audio_independently_from_control_and_revokes_both_queues() {
    let (host, viewer, id) = pair(false).await;
    assert!(!host.snapshot().audio_enabled);
    assert!(matches!(
        host.publish_audio(id.clone(), audio(1, 2)),
        Err(Error::PermissionDenied)
    ));
    assert!(matches!(
        viewer.set_audio_enabled(id.clone(), true).await,
        Err(Error::PermissionDenied)
    ));
    host.set_audio_enabled(id.clone(), true).await.unwrap();
    host.publish_audio(id.clone(), audio(2, 2)).unwrap();
    let mut received = Vec::new();
    until(|| {
        received.extend(viewer.take_audio(id.clone()).unwrap());
        !received.is_empty()
    })
    .await;
    assert_eq!(received, vec![audio(2, 2)]);
    assert!(viewer.snapshot().audio_enabled);
    assert!(!viewer.snapshot().control);
    for _ in 0..15 {
        host.publish_audio(id.clone(), audio(3, 1)).unwrap();
    }
    host.set_audio_enabled(id.clone(), false).await.unwrap();
    until(|| viewer.snapshot().audio_epoch == 2).await;
    let disabled = viewer.take_audio_batch(id.clone()).unwrap();
    assert_eq!(disabled.request_id, id);
    assert_eq!(disabled.audio_epoch, 2);
    assert!(!disabled.audio_enabled);
    assert!(disabled.packets.is_empty());
    assert!(!viewer.snapshot().audio_enabled);
    host.set_audio_enabled(id.clone(), true).await.unwrap();
    host.publish_audio(id.clone(), audio(4, 1)).unwrap();
    let mut received = Vec::new();
    until(|| {
        received.extend(viewer.take_audio(id.clone()).unwrap());
        !received.is_empty()
    })
    .await;
    assert!(received.iter().all(|p| p == &audio(4, 1)));
    host.stop().await.unwrap();
    until(|| viewer.snapshot().state != SessionState::Active).await;
    assert!(!viewer.snapshot().audio_enabled);
    assert!(viewer.take_audio(id).is_err());
}
#[tokio::test]
async fn audio_is_bounded_and_stale_or_invalid_packets_rejected() {
    let (host, viewer, id) = pair(false).await;
    host.set_audio_enabled(id.clone(), true).await.unwrap();
    assert!(host.publish_audio("stale".into(), audio(1, 1)).is_err());
    assert!(host.take_audio(id.clone()).is_err());
    for packet in [
        AudioPacket {
            sample_rate: 44_100,
            ..audio(1, 1)
        },
        AudioPacket {
            channels: 3,
            ..audio(1, 1)
        },
        AudioPacket {
            pcm: vec![0; 1921],
            ..audio(1, 1)
        },
        AudioPacket {
            pcm: vec![0; 3841],
            ..audio(1, 2)
        },
    ] {
        assert!(matches!(
            host.publish_audio(id.clone(), packet),
            Err(Error::InvalidAudio)
        ));
    }
    for n in 0..100 {
        host.publish_audio(id.clone(), audio(n, 1)).unwrap();
    }
    // Allow packets to arrive without draining; playback backlog is never >200ms.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let packets = viewer.take_audio(id).unwrap();
    assert!(!packets.is_empty() && packets.len() <= 10);
    assert_eq!(packets.last().unwrap().pcm[0], 99);
    host.stop().await.unwrap();
}
#[tokio::test]
async fn unicode_text_and_function_keys_require_control_and_reach_host() {
    let (host, viewer, id) = pair(true).await;
    for key in [Key::F1, Key::F12, Key::Insert, Key::CapsLock] {
        viewer
            .send_input(InputEvent::Key { key, down: true })
            .await
            .unwrap();
    }
    let text = "Привет 🌍\n中文\tтекст";
    viewer
        .send_input(InputEvent::Text { text: text.into() })
        .await
        .unwrap();
    let mut received = Vec::new();
    until(|| {
        received.extend(host.take_input());
        received.len() == 5
    })
    .await;
    assert!(matches!(&received[4],InputEvent::Text{text:value} if value==text));
    for text in [
        "".into(),
        "x".repeat(4097),
        "\0".into(),
        "\r".into(),
        "\u{1b}".into(),
        "\t".repeat(4096),
    ] {
        assert!(matches!(
            viewer.send_input(InputEvent::Text { text }).await,
            Err(Error::InvalidInput)
        ));
    }
    host.stop().await.unwrap();
    assert!(host.set_audio_enabled(id, true).await.is_err());
    let (host, viewer, _) = pair(false).await;
    assert!(matches!(
        viewer
            .send_input(InputEvent::Text {
                text: "hello".into()
            })
            .await,
        Err(Error::PermissionDenied)
    ));
    host.stop().await.unwrap();
}
