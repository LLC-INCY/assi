use assi_session::{
    ChatAuthor, Error, InputEvent, RemoteDisplay, SessionEngine, SessionState, VideoCodec,
    VideoFrame,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
async fn until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(4), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("state transition timed out");
}
async fn pair(control: bool) -> (Arc<SessionEngine>, Arc<SessionEngine>, String) {
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
    host.accept(id.clone(), control).await.unwrap();
    until(|| {
        host.snapshot().state == SessionState::Active
            && viewer.snapshot().state == SessionState::Active
    })
    .await;
    (host, viewer, id)
}
fn frame(value: u8) -> VideoFrame {
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut bytes)
        .encode(&[value, 30, 40], 1, 1, image::ExtendedColorType::Rgb8)
        .unwrap();
    VideoFrame {
        codec: VideoCodec::Jpeg,
        width: 1,
        height: 1,
        timestamp_us: u64::from(value),
        keyframe: true,
        sequence: 0,
        screen_epoch: 0,
        bytes,
    }
}
fn displays() -> Vec<RemoteDisplay> {
    vec![
        RemoteDisplay {
            id: "one".into(),
            name: "Main".into(),
            width: 1920,
            height: 1080,
            primary: true,
        },
        RemoteDisplay {
            id: "two".into(),
            name: "Other".into(),
            width: 1280,
            height: 720,
            primary: false,
        },
    ]
}
#[tokio::test]
async fn clipboard_requires_separate_host_consent_roundtrips_and_revokes() {
    let (host, viewer, id) = pair(true).await;
    assert!(matches!(
        viewer.send_clipboard(id.clone(), "secret".into()).await,
        Err(Error::PermissionDenied)
    ));
    assert!(matches!(
        viewer.set_clipboard_enabled(id.clone(), true).await,
        Err(Error::PermissionDenied)
    ));
    host.set_clipboard_enabled(id.clone(), true).await.unwrap();
    until(|| viewer.snapshot().clipboard_enabled).await;
    viewer
        .send_clipboard(id.clone(), "Привет\n世界".into())
        .await
        .unwrap();
    let mut text = None;
    until(|| {
        text = host.take_clipboard(id.clone()).unwrap().text;
        text.is_some()
    })
    .await;
    assert_eq!(text.as_deref(), Some("Привет\n世界"));
    assert!(host.take_clipboard(id.clone()).unwrap().text.is_none());
    host.send_clipboard(id.clone(), "reply".into())
        .await
        .unwrap();
    until(|| {
        text = viewer.take_clipboard(id.clone()).unwrap().text;
        text.is_some()
    })
    .await;
    assert_eq!(text.as_deref(), Some("reply"));
    assert!(host.send_clipboard("old".into(), "x".into()).await.is_err());
    assert!(host
        .send_clipboard(id.clone(), "x".repeat(4097))
        .await
        .is_err());
    assert!(host.send_clipboard(id.clone(), "\0".into()).await.is_err());
    host.set_clipboard_enabled(id.clone(), false).await.unwrap();
    until(|| !viewer.snapshot().clipboard_enabled).await;
    assert!(viewer.take_clipboard(id.clone()).unwrap().text.is_none());
    assert!(host
        .send_clipboard(id.clone(), "late".into())
        .await
        .is_err());
    host.stop().await.unwrap();
    assert!(host.take_clipboard(id).is_err());
}
#[tokio::test]
async fn chat_host_orders_both_authors_and_bounded_history() {
    let (host, viewer, id) = pair(false).await;
    viewer.send_chat(id.clone(), "Hello".into()).await.unwrap();
    until(|| host.chat(id.clone()).unwrap().messages.len() == 1).await;
    host.send_chat(id.clone(), "Reply".into()).await.unwrap();
    until(|| viewer.chat(id.clone()).unwrap().messages.len() == 2).await;
    let messages = viewer.chat(id.clone()).unwrap().messages;
    assert_eq!(messages[0].author, ChatAuthor::Viewer);
    assert_eq!(messages[1].author, ChatAuthor::Host);
    for n in 0..105 {
        host.send_chat(id.clone(), format!("Message {n}"))
            .await
            .unwrap();
        until(|| {
            viewer
                .chat(id.clone())
                .unwrap()
                .messages
                .last()
                .unwrap()
                .sequence
                == n + 3
        })
        .await;
    }
    let messages = host.chat(id.clone()).unwrap().messages;
    assert_eq!(messages.len(), 100);
    assert_eq!(messages.first().unwrap().sequence, 8);
    assert!(viewer
        .send_chat(id.clone(), "x".repeat(2049))
        .await
        .is_err());
    assert!(viewer.send_chat(id.clone(), "\u{1b}".into()).await.is_err());
    host.stop().await.unwrap();
    until(|| viewer.snapshot().state != SessionState::Active).await;
    assert!(viewer.chat(id).is_err());
}
#[tokio::test]
async fn video_negotiation_orders_frames_allows_jpeg_fallback_and_requires_keyframe_after_overflow()
{
    let (host, viewer, id) = pair(false).await;
    let h264 = VideoFrame {
        codec: VideoCodec::H264,
        bytes: vec![0, 0, 0, 1, 0x65, 0x80],
        ..frame(1)
    };
    assert!(matches!(
        host.publish_video(id.clone(), h264.clone()),
        Err(Error::PermissionDenied)
    ));
    viewer
        .set_video_codec(id.clone(), VideoCodec::H264)
        .await
        .unwrap();
    until(|| host.snapshot().video_codec == VideoCodec::H264).await;
    assert!(host.take_video_keyframe_request(id.clone()).unwrap());
    host.publish_video(id.clone(), h264).unwrap();
    let mut received = None;
    until(|| {
        received = viewer.latest_video(id.clone(), 0).unwrap();
        received.is_some()
    })
    .await;
    assert_eq!(received.unwrap().codec, VideoCodec::H264);
    host.publish_video(id.clone(), frame(2)).unwrap();
    until(|| viewer.latest_video(id.clone(), 1).unwrap().is_some()).await;
    assert_eq!(
        viewer.latest_video(id.clone(), 1).unwrap().unwrap().codec,
        VideoCodec::Jpeg
    );
    viewer
        .set_video_codec(id.clone(), VideoCodec::Jpeg)
        .await
        .unwrap();
    until(|| host.snapshot().video_codec == VideoCodec::Jpeg).await;
    assert!(host.publish_video("old".into(), frame(3)).is_err());
    for n in 3..11 {
        host.publish_video(id.clone(), frame(n)).unwrap();
    }
    assert!(matches!(
        host.publish_video(id.clone(), frame(11)),
        Err(Error::Backpressure)
    ));
    assert!(host.take_video_keyframe_request(id.clone()).unwrap());
    host.stop().await.unwrap();
}
#[tokio::test]
async fn monitor_change_blocks_old_input_until_new_frame_is_acknowledged() {
    let (host, viewer, id) = pair(true).await;
    host.set_displays(id.clone(), displays(), "one".into())
        .unwrap();
    until(|| viewer.snapshot().displays.len() == 2).await;
    viewer
        .request_display(id.clone(), "two".into())
        .await
        .unwrap();
    let mut requested = None;
    until(|| {
        requested = host.take_display_request(id.clone()).unwrap();
        requested.is_some()
    })
    .await;
    assert_eq!(requested.as_deref(), Some("two"));
    assert!(host
        .take_input()
        .iter()
        .all(|e| matches!(e, InputEvent::ReleaseAll)));
    let epoch = host.commit_display(id.clone(), "two".into()).unwrap();
    assert_eq!(epoch, 2);
    until(|| viewer.snapshot().screen_epoch == 2).await;
    viewer
        .send_input_at(id.clone(), 1, InputEvent::Move { x: 0.2, y: 0.2 })
        .await
        .unwrap();
    viewer
        .send_input_at(id.clone(), 2, InputEvent::Move { x: 0.2, y: 0.2 })
        .await
        .unwrap();
    assert!(host
        .take_input()
        .iter()
        .all(|e| matches!(e, InputEvent::ReleaseAll)));
    assert_eq!(viewer.snapshot().state, SessionState::Active);
    assert!(viewer.acknowledge_screen(id.clone(), 2).await.is_err());
    let stale = assi_session::AnnotationCommand::Add {
        stroke: assi_session::AnnotationStroke {
            id: uuid::Uuid::new_v4().to_string(),
            tool: assi_session::AnnotationTool::Pen,
            color: assi_session::AnnotationColor::Mint,
            width: 3,
            points: vec![assi_session::AnnotationPoint { x: 0.5, y: 0.5 }],
        },
    };
    viewer
        .annotate_at(id.clone(), 1, stale.clone())
        .await
        .unwrap();
    host.annotate_at(id.clone(), 1, stale).await.unwrap();
    assert!(host.annotations().strokes.is_empty());
    host.publish_video(
        id.clone(),
        VideoFrame {
            screen_epoch: 2,
            ..frame(1)
        },
    )
    .unwrap();
    until(|| viewer.latest_video(id.clone(), 0).unwrap().is_some()).await;
    viewer.acknowledge_screen(id.clone(), 2).await.unwrap();
    until(|| !host.snapshot().screen_pending).await;
    viewer
        .send_input_at(id.clone(), 2, InputEvent::Move { x: 0.2, y: 0.2 })
        .await
        .unwrap();
    let mut inputs = Vec::new();
    until(|| {
        inputs.extend(host.take_input());
        inputs.iter().any(|e| matches!(e, InputEvent::Move { .. }))
    })
    .await;
    host.stop().await.unwrap();
}
#[tokio::test]
async fn previous_protocol_invitation_is_explicitly_rejected() {
    let host = SessionEngine::new();
    let viewer = SessionEngine::new();
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let invite = host
        .start_host(SocketAddr::new(ip, 0), ip, "Host".into())
        .await
        .unwrap();
    let bytes = URL_SAFE_NO_PAD
        .decode(invite.strip_prefix("assi://").unwrap())
        .unwrap();
    let mut json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    json["version"] = serde_json::json!(2);
    let old = format!(
        "assi://{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json).unwrap())
    );
    assert!(matches!(
        viewer.connect(old, "Viewer".into()).await,
        Err(Error::InvalidInvitation)
    ));
    host.stop().await.unwrap();
}

#[tokio::test]
async fn h264_to_jpeg_fallback_discards_inflight_frames_without_disconnect() {
    let (host, viewer, id) = pair(false).await;
    viewer
        .set_video_codec(id.clone(), VideoCodec::H264)
        .await
        .unwrap();
    until(|| host.snapshot().video_codec == VideoCodec::H264).await;
    for n in 0..4 {
        host.publish_video(
            id.clone(),
            VideoFrame {
                codec: VideoCodec::H264,
                bytes: vec![0, 0, 0, 1, 0x65, n],
                ..frame(n)
            },
        )
        .unwrap();
    }
    viewer
        .set_video_codec(id.clone(), VideoCodec::Jpeg)
        .await
        .unwrap();
    until(|| host.snapshot().video_codec == VideoCodec::Jpeg).await;
    host.publish_video(id.clone(), frame(10)).unwrap();
    let mut got = None;
    until(|| {
        got = viewer.latest_video(id.clone(), 0).unwrap();
        got.is_some()
    })
    .await;
    assert_eq!(got.unwrap().codec, VideoCodec::Jpeg);
    assert_eq!(host.snapshot().state, SessionState::Active);
    assert_eq!(viewer.snapshot().state, SessionState::Active);
    host.stop().await.unwrap();
}
