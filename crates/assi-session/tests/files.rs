use assi_session::{SessionEngine, SessionState, TransferState};
use std::{sync::Arc, time::Duration};
async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
async fn pair() -> (Arc<SessionEngine>, Arc<SessionEngine>, String) {
    let host = SessionEngine::new();
    let viewer = SessionEngine::new();
    let invitation = host
        .start_host(
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1".parse().unwrap(),
            "Host".into(),
        )
        .await
        .unwrap();
    viewer.connect(invitation, "Viewer".into()).await.unwrap();
    until(|| host.snapshot().state == SessionState::Pending).await;
    let request = host.snapshot().request_id.unwrap();
    host.accept(request.clone(), false).await.unwrap();
    until(|| {
        viewer.snapshot().state == SessionState::Active
            && host.snapshot().state == SessionState::Active
    })
    .await;
    (host, viewer, request)
}
#[tokio::test]
async fn file_needs_recipient_approval_and_roundtrips_without_control() {
    let (host, viewer, request) = pair().await;
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.bin");
    let destination = dir.path().join("received.bin");
    let bytes = vec![42u8; 200_001];
    std::fs::write(&source, &bytes).unwrap();
    let id = host.offer_file(request.clone(), source).await.unwrap();
    until(|| viewer.snapshot().transfer.is_some()).await;
    assert!(!destination.exists());
    assert_eq!(
        viewer.snapshot().transfer.unwrap().state,
        TransferState::Offered
    );
    viewer
        .accept_file(request, id, destination.clone())
        .await
        .unwrap();
    until(|| {
        host.snapshot()
            .transfer
            .is_some_and(|x| x.state == TransferState::Completed)
    })
    .await;
    assert_eq!(std::fs::read(destination).unwrap(), bytes);
    host.stop().await.unwrap();
    viewer.stop().await.unwrap();
}
#[tokio::test]
async fn empty_file_and_reverse_direction_work() {
    let (host, viewer, request) = pair().await;
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("empty.txt");
    let target = dir.path().join("copy.txt");
    std::fs::write(&source, []).unwrap();
    let id = viewer.offer_file(request.clone(), source).await.unwrap();
    until(|| host.snapshot().transfer.is_some()).await;
    host.accept_file(request, id, target.clone()).await.unwrap();
    until(|| {
        viewer
            .snapshot()
            .transfer
            .is_some_and(|x| x.state == TransferState::Completed)
    })
    .await;
    assert_eq!(std::fs::metadata(target).unwrap().len(), 0);
    host.stop().await.unwrap();
    viewer.stop().await.unwrap();
}
#[tokio::test]
async fn reject_and_stale_request_do_not_create_files() {
    let (host, viewer, request) = pair().await;
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.txt");
    std::fs::write(&source, b"test").unwrap();
    assert!(host
        .offer_file("stale".into(), source.clone())
        .await
        .is_err());
    let id = host.offer_file(request.clone(), source).await.unwrap();
    until(|| viewer.snapshot().transfer.is_some()).await;
    viewer.reject_file(request, id).await.unwrap();
    until(|| {
        host.snapshot()
            .transfer
            .is_some_and(|x| x.state == TransferState::Rejected)
    })
    .await;
    host.stop().await.unwrap();
    viewer.stop().await.unwrap();
}
#[tokio::test]
async fn existing_destination_is_never_overwritten() {
    let (host, viewer, request) = pair().await;
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.txt");
    let target = dir.path().join("target.txt");
    std::fs::write(&source, b"new").unwrap();
    std::fs::write(&target, b"original").unwrap();
    let id = host.offer_file(request.clone(), source).await.unwrap();
    until(|| viewer.snapshot().transfer.is_some()).await;
    assert!(viewer
        .accept_file(request, id, target.clone())
        .await
        .is_err());
    assert_eq!(std::fs::read(target).unwrap(), b"original");
    host.stop().await.unwrap();
    viewer.stop().await.unwrap();
}

#[tokio::test]
async fn cancel_during_transfer_keeps_session_active_and_removes_partial() {
    let (host, viewer, request) = pair().await;
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.bin");
    std::fs::write(&source, vec![71u8; 8 * 1024 * 1024]).unwrap();
    let output = tempfile::tempdir().unwrap();
    let target = output.path().join("received.bin");
    let id = host.offer_file(request.clone(), source).await.unwrap();
    until(|| viewer.snapshot().transfer.is_some()).await;
    viewer
        .accept_file(request.clone(), id.clone(), target.clone())
        .await
        .unwrap();
    until(|| {
        viewer
            .snapshot()
            .transfer
            .is_some_and(|x| x.transferred > 0)
    })
    .await;
    viewer.cancel_file(request, id).await.unwrap();
    until(|| {
        host.snapshot()
            .transfer
            .is_some_and(|x| x.state == TransferState::Cancelled)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(viewer.snapshot().state, SessionState::Active);
    assert_eq!(host.snapshot().state, SessionState::Active);
    assert!(!target.exists());
    assert_eq!(std::fs::read_dir(output.path()).unwrap().count(), 0);
    host.stop().await.unwrap();
    viewer.stop().await.unwrap();
}
#[tokio::test]
async fn stop_waits_for_partial_cleanup() {
    let (host, viewer, request) = pair().await;
    let source_dir = tempfile::tempdir().unwrap();
    let source = source_dir.path().join("source.bin");
    std::fs::write(&source, vec![1u8; 8 * 1024 * 1024]).unwrap();
    let output = tempfile::tempdir().unwrap();
    let id = host.offer_file(request.clone(), source).await.unwrap();
    until(|| viewer.snapshot().transfer.is_some()).await;
    viewer
        .accept_file(request, id, output.path().join("received.bin"))
        .await
        .unwrap();
    until(|| {
        viewer
            .snapshot()
            .transfer
            .is_some_and(|x| x.transferred > 0)
    })
    .await;
    viewer.stop().await.unwrap();
    assert_eq!(std::fs::read_dir(output.path()).unwrap().count(), 0);
    host.stop().await.unwrap();
}
#[tokio::test]
async fn destination_created_after_accept_is_preserved() {
    let (host, viewer, request) = pair().await;
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.bin");
    let destination = dir.path().join("target.bin");
    std::fs::write(&source, vec![5u8; 1024 * 1024]).unwrap();
    let id = host.offer_file(request.clone(), source).await.unwrap();
    until(|| viewer.snapshot().transfer.is_some()).await;
    viewer
        .accept_file(request, id, destination.clone())
        .await
        .unwrap();
    std::fs::write(&destination, b"do not overwrite").unwrap();
    until(|| {
        viewer
            .snapshot()
            .transfer
            .is_some_and(|x| x.state == TransferState::Error)
    })
    .await;
    assert_eq!(std::fs::read(destination).unwrap(), b"do not overwrite");
    host.stop().await.unwrap();
    viewer.stop().await.unwrap();
}
#[tokio::test]
async fn changed_source_size_cancels_without_publishing() {
    let (host, viewer, request) = pair().await;
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.txt");
    let destination = dir.path().join("target.txt");
    std::fs::write(&source, b"original").unwrap();
    let id = host
        .offer_file(request.clone(), source.clone())
        .await
        .unwrap();
    until(|| viewer.snapshot().transfer.is_some()).await;
    std::fs::write(source, b"longer replacement").unwrap();
    viewer
        .accept_file(request, id, destination.clone())
        .await
        .unwrap();
    until(|| {
        host.snapshot()
            .transfer
            .is_some_and(|x| x.state == TransferState::Error)
    })
    .await;
    until(|| {
        viewer
            .snapshot()
            .transfer
            .is_some_and(|x| x.state == TransferState::Cancelled)
    })
    .await;
    assert!(!destination.exists());
    host.stop().await.unwrap();
    viewer.stop().await.unwrap();
}

#[tokio::test]
async fn paused_file_resumes_from_checkpoint_after_explicit_receiver_approval() {
    let (host, viewer, request) = pair().await;
    let source_dir = tempfile::tempdir().unwrap();
    let source = source_dir.path().join("resume.bin");
    let contents = vec![59u8; 1024 * 1024];
    std::fs::write(&source, &contents).unwrap();
    let output = tempfile::tempdir().unwrap();
    let destination = output.path().join("received.bin");
    let id = host.offer_file(request.clone(), source).await.unwrap();
    until(|| viewer.snapshot().transfer.is_some()).await;
    viewer
        .accept_file(request.clone(), id.clone(), destination.clone())
        .await
        .unwrap();
    until(|| {
        viewer
            .snapshot()
            .transfer
            .is_some_and(|t| t.transferred > 0)
    })
    .await;
    host.pause_file(request.clone(), id.clone()).await.unwrap();
    until(|| {
        host.snapshot().transfer.is_some_and(|t| t.pause_ready)
            && viewer.snapshot().transfer.is_some_and(|t| t.pause_ready)
    })
    .await;
    let offset = viewer.snapshot().transfer.unwrap().transferred;
    assert!(offset > 0 && offset < contents.len() as u64);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(viewer.snapshot().transfer.unwrap().transferred, offset);
    assert!(!destination.exists());
    assert!(host.resume_file("stale".into(), id.clone()).await.is_err());
    host.resume_file(request.clone(), id.clone()).await.unwrap();
    until(|| {
        viewer
            .snapshot()
            .transfer
            .is_some_and(|t| t.resume_requested)
    })
    .await;
    assert_eq!(
        viewer.snapshot().transfer.unwrap().state,
        TransferState::Paused
    );
    viewer
        .resume_file(request.clone(), id.clone())
        .await
        .unwrap();
    until(|| {
        host.snapshot()
            .transfer
            .is_some_and(|t| t.state == TransferState::Completed)
    })
    .await;
    assert_eq!(std::fs::read(destination).unwrap(), contents);
    assert_eq!(host.snapshot().state, SessionState::Active);
    host.stop().await.unwrap();
    viewer.stop().await.unwrap();
}

#[tokio::test]
async fn paused_file_cancel_and_disconnect_remove_partial_and_reject_old_resume() {
    for cancel in [true, false] {
        let (host, viewer, request) = pair().await;
        let input = tempfile::tempdir().unwrap();
        let source = input.path().join("pause.bin");
        std::fs::write(&source, vec![77u8; 1024 * 1024]).unwrap();
        let output = tempfile::tempdir().unwrap();
        let destination = output.path().join("received.bin");
        let id = host.offer_file(request.clone(), source).await.unwrap();
        until(|| viewer.snapshot().transfer.is_some()).await;
        viewer
            .accept_file(request.clone(), id.clone(), destination.clone())
            .await
            .unwrap();
        until(|| {
            viewer
                .snapshot()
                .transfer
                .is_some_and(|t| t.transferred > 0)
        })
        .await;
        viewer
            .pause_file(request.clone(), id.clone())
            .await
            .unwrap();
        until(|| viewer.snapshot().transfer.is_some_and(|t| t.pause_ready)).await;
        if cancel {
            viewer
                .cancel_file(request.clone(), id.clone())
                .await
                .unwrap();
            until(|| {
                host.snapshot()
                    .transfer
                    .is_some_and(|t| t.state == TransferState::Cancelled)
            })
            .await;
            assert_eq!(viewer.snapshot().state, SessionState::Active);
        } else {
            host.stop().await.unwrap();
            until(|| viewer.snapshot().state != SessionState::Active).await;
            viewer.stop().await.unwrap();
        }
        assert_eq!(std::fs::read_dir(output.path()).unwrap().count(), 0);
        assert!(viewer.resume_file(request, id).await.is_err());
        host.stop().await.unwrap();
        viewer.stop().await.unwrap();
    }
}
