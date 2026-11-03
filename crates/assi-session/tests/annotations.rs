use assi_session::{
    AnnotationColor, AnnotationCommand, AnnotationPoint, AnnotationStroke, AnnotationTool, Error,
    SessionEngine, SessionState,
};
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
fn stroke() -> AnnotationStroke {
    AnnotationStroke {
        id: uuid::Uuid::new_v4().to_string(),
        tool: AnnotationTool::Pen,
        color: AnnotationColor::Mint,
        width: 3,
        points: vec![
            AnnotationPoint { x: 0.2, y: 0.4 },
            AnnotationPoint { x: 0.8, y: 0.6 },
        ],
    }
}
async fn pair() -> (Arc<SessionEngine>, Arc<SessionEngine>, String) {
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
    assert!(matches!(
        host.annotate(id.clone(), AnnotationCommand::Clear).await,
        Err(Error::InvalidState)
    ));
    host.accept(id.clone(), false).await.unwrap();
    until(|| {
        viewer.snapshot().state == SessionState::Active
            && host.snapshot().state == SessionState::Active
    })
    .await;
    (host, viewer, id)
}
#[tokio::test]
async fn shared_board_is_bidirectional_view_only_ordered_and_author_scoped() {
    let (host, viewer, id) = pair().await;
    let host_stroke = stroke();
    let viewer_stroke = stroke();
    let (a, b) = tokio::join!(
        host.annotate(
            id.clone(),
            AnnotationCommand::Add {
                stroke: host_stroke.clone()
            }
        ),
        viewer.annotate(
            id.clone(),
            AnnotationCommand::Add {
                stroke: viewer_stroke.clone()
            }
        )
    );
    a.unwrap();
    b.unwrap();
    until(|| host.annotations().revision == 2 && viewer.annotations().revision == 2).await;
    assert_eq!(host.annotations(), viewer.annotations());
    assert_eq!(host.annotations().strokes.len(), 2);
    viewer
        .annotate(id.clone(), AnnotationCommand::Undo)
        .await
        .unwrap();
    until(|| viewer.annotations().revision == 3).await;
    assert_eq!(host.annotations(), viewer.annotations());
    assert_eq!(host.annotations().strokes[0].stroke.id, host_stroke.id);
    host.annotate(id, AnnotationCommand::Clear).await.unwrap();
    until(|| viewer.annotations().revision == 4).await;
    assert!(viewer.annotations().strokes.is_empty());
    host.stop().await.unwrap();
    until(|| viewer.snapshot().state != SessionState::Active).await;
    assert_eq!(host.annotations().revision, 0);
    assert!(viewer.annotations().request_id.is_none());
}
#[tokio::test]
async fn annotations_reject_stale_requests_and_unbounded_or_invalid_shapes() {
    let (host, viewer, id) = pair().await;
    assert!(matches!(
        viewer
            .annotate("old".into(), AnnotationCommand::Clear)
            .await,
        Err(Error::InvalidState)
    ));
    for invalid in [
        {
            let mut s = stroke();
            s.points[0].x = f64::NAN;
            s
        },
        {
            let mut s = stroke();
            s.points[0].y = 1.1;
            s
        },
        {
            let mut s = stroke();
            s.points = vec![AnnotationPoint { x: 0.5, y: 0.5 }; 129];
            s
        },
        {
            let mut s = stroke();
            s.width = 13;
            s
        },
        {
            let mut s = stroke();
            s.tool = AnnotationTool::Rectangle;
            s.points.pop();
            s
        },
        {
            let mut s = stroke();
            s.id = "not-id".into();
            s
        },
    ] {
        assert!(matches!(
            viewer
                .annotate(id.clone(), AnnotationCommand::Add { stroke: invalid })
                .await,
            Err(Error::InvalidAnnotation)
        ));
    }
    assert_eq!(host.annotations().revision, 0);
    host.stop().await.unwrap();
}
#[tokio::test]
async fn host_preview_and_viewer_receive_same_original_frame() {
    let (host, viewer, _) = pair().await;
    let mut frame = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut frame)
        .encode(&[20, 80, 90], 1, 1, image::ExtendedColorType::Rgb8)
        .unwrap();
    host.publish_frame(frame.clone()).unwrap();
    until(|| viewer.latest_frame().is_some()).await;
    assert_eq!(host.latest_frame(), Some(frame.clone()));
    assert_eq!(viewer.latest_frame(), Some(frame));
    host.stop().await.unwrap();
    assert!(host.latest_frame().is_none());
}
#[tokio::test]
async fn wheel_requires_control_and_rejects_extreme_values() {
    let (host, viewer, _) = pair().await;
    assert!(matches!(
        viewer
            .send_input(assi_session::InputEvent::Wheel { x: 0, y: 3 })
            .await,
        Err(Error::PermissionDenied)
    ));
    host.stop().await.unwrap();
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let invite = host
        .start_host(SocketAddr::new(ip, 0), ip, "Host".into())
        .await
        .unwrap();
    viewer.connect(invite, "Viewer".into()).await.unwrap();
    until(|| host.snapshot().state == SessionState::Pending).await;
    host.accept(host.snapshot().request_id.unwrap(), true)
        .await
        .unwrap();
    until(|| viewer.snapshot().state == SessionState::Active).await;
    for (x, y) in [(101, 0), (0, -101), (i32::MIN, i32::MAX)] {
        assert!(matches!(
            viewer
                .send_input(assi_session::InputEvent::Wheel { x, y })
                .await,
            Err(Error::InvalidInput)
        ));
    }
    viewer
        .send_input(assi_session::InputEvent::Wheel { x: -2, y: 3 })
        .await
        .unwrap();
    let mut input = Vec::new();
    until(|| {
        input.extend(host.take_input());
        !input.is_empty()
    })
    .await;
    assert!(matches!(
        input[0],
        assi_session::InputEvent::Wheel { x: -2, y: 3 }
    ));
    host.stop().await.unwrap();
}
