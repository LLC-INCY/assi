//! Capture and input have independent schedules. Encoding never holds the input lock.
use super::*;
#[cfg(not(target_os = "android"))]
pub(super) struct Preview {
    request: String,
    epoch: u64,
    pixels: Arc<assi_platform::CapturedRgba>,
    jpeg: Option<Vec<u8>>,
}
impl Remote {
    pub(super) async fn input_tick(&self, request: Option<String>) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            let _ = request;
            Ok(())
        }
        #[cfg(not(target_os = "android"))]
        {
            let engine = self.engine.clone();
            let platform = self.platform.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let mut adapter = platform.lock().map_err(|_| "Ошибка состояния ввода")?;
                for event in engine.take_input() {
                    let now = engine.snapshot();
                    if now.state != SessionState::Active || now.request_id != request {
                        break;
                    }
                    if matches!(event, InputEvent::ReleaseAll) {
                        adapter.release_inputs().map_err(|e| e.to_string())?;
                        continue;
                    }
                    if !now.control || now.screen_pending {
                        continue;
                    }
                    let value = serde_json::to_value(event).map_err(|_| "Ошибка ввода")?;
                    let event =
                        serde_json::from_value(value).map_err(|_| "Неподдерживаемая клавиша")?;
                    match adapter.apply_input(event) {
                        // Geometry is established by the next capture, not by an expensive
                        // extra capture per key. Until then input is deliberately ignored.
                        Err(assi_platform::PlatformError::CaptureRequired) => {}
                        result => result.map_err(|e| e.to_string())?,
                    }
                }
                Ok(())
            })
            .await
            .map_err(|_| "Системный ввод прерван")?
        }
    }
    pub(super) async fn pump_video(self: Arc<Self>) {
        let clock = std::time::Instant::now();
        let mut previous = None;
        let mut force_keyframe = true;
        while !self.closed.load(Ordering::SeqCst) {
            let start = std::time::Instant::now();
            let snapshot = self.engine.snapshot();
            if snapshot.state != SessionState::Active || snapshot.role != Role::Host {
                previous = None;
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            let Some(id) = snapshot.request_id.clone() else {
                continue;
            };
            let identity = (id.clone(), snapshot.screen_epoch);
            let changed = previous.as_ref() != Some(&identity);
            force_keyframe |= changed;
            if changed {
                #[cfg(not(target_os = "android"))]
                if let Ok(mut encoder) = self.encoder.lock() {
                    encoder.reset();
                }
                previous = Some(identity);
            }
            force_keyframe |= self
                .engine
                .take_video_keyframe_request(id.clone())
                .unwrap_or(false);
            let h264 = snapshot.video_codec == assi_session::VideoCodec::H264;
            let fps = if h264 {
                24
            } else {
                self.capture_fps.load(Ordering::Relaxed).clamp(1, 30)
            };
            let result = self
                .video_tick(
                    &snapshot,
                    clock.elapsed().as_micros() as u64,
                    force_keyframe,
                )
                .await;
            match result {
                Ok(()) => force_keyframe = false,
                Err(assi_session::Error::Backpressure) => force_keyframe = true,
                Err(assi_session::Error::InvalidState) => force_keyframe = true,
                Err(error) => {
                    self.fail_video(&snapshot, error.to_string()).await;
                }
            }
            tokio::time::sleep(Duration::from_millis(1000 / fps).saturating_sub(start.elapsed()))
                .await;
        }
    }
    async fn fail_video(&self, snapshot: &Snapshot, error: String) {
        // Encoding finishes outside lifecycle; only its original session/configuration may fail.
        let _guard = self.lifecycle.lock().await;
        let current = self.engine.snapshot();
        if current.state == SessionState::Active
            && current.request_id == snapshot.request_id
            && current.screen_epoch == snapshot.screen_epoch
            && current.video_codec == snapshot.video_codec
        {
            *self.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
            let _ = self.engine.stop().await;
            let _ = self.release().await;
            let _ = self.restore_fullscreen();
        }
    }
    #[cfg(not(target_os = "android"))]
    async fn video_tick(
        &self,
        snapshot: &Snapshot,
        timestamp: u64,
        force: bool,
    ) -> assi_session::Result<()> {
        let id = snapshot
            .request_id
            .clone()
            .ok_or(assi_session::Error::InvalidState)?;
        // No in-flight capture may cross monitor selection or lifecycle reset.
        let guard = self.lifecycle.lock().await;
        let now = self.engine.snapshot();
        if now.state != SessionState::Active
            || now.request_id != snapshot.request_id
            || now.screen_epoch != snapshot.screen_epoch
        {
            return Err(assi_session::Error::InvalidState);
        }
        if let Err(error) = self.apply_display_request(&id).await {
            *self.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
            let _ = self.engine.stop().await;
            let _ = self.release().await;
            return Err(assi_session::Error::InvalidState);
        }
        let captured_epoch = self.engine.snapshot().screen_epoch;
        let force = force || captured_epoch != snapshot.screen_epoch;
        let platform = self.platform.clone();
        let (raw, quality) = tauri::async_runtime::spawn_blocking(move || {
            let mut adapter = platform.lock().map_err(|_| assi_session::Error::State)?;
            let quality = adapter.quality();
            adapter
                .capture_rgba()
                .map(|raw| (Arc::new(raw), quality))
                .map_err(|_| assi_session::Error::InvalidFrame)
        })
        .await
        .map_err(|_| assi_session::Error::State)??;
        *self
            .preview
            .lock()
            .map_err(|_| assi_session::Error::State)? = Some(Preview {
            request: id.clone(),
            epoch: captured_epoch,
            pixels: raw.clone(),
            jpeg: None,
        });
        drop(guard);
        let encoder = self.encoder.clone();
        let codec = match snapshot.video_codec {
            assi_session::VideoCodec::Jpeg => assi_video::VideoCodec::Jpeg,
            assi_session::VideoCodec::H264 => assi_video::VideoCodec::H264,
        };
        let encoded = tauri::async_runtime::spawn_blocking(move || {
            let result = encoder
                .lock()
                .map_err(|_| assi_session::Error::State)?
                .encode_rgba(
                    raw.width,
                    raw.height,
                    &raw.bytes,
                    timestamp,
                    assi_video::EncoderOptions {
                        codec,
                        max_edge: quality.max_edge(),
                        jpeg_quality: quality.jpeg_quality(),
                        fps: 24,
                        bitrate: 4_000_000,
                    },
                    force,
                );
            match result {
                Ok(frame) => Ok(Some(frame)),
                Err(assi_video::VideoError::Skipped) => Ok(None),
                Err(_) => Err(assi_session::Error::InvalidFrame),
            }
        })
        .await
        .map_err(|_| assi_session::Error::State)??;
        let Some(encoded) = encoded else {
            return if force {
                Err(assi_session::Error::Backpressure)
            } else {
                Ok(())
            };
        };
        let now = self.engine.snapshot();
        if now.state != SessionState::Active
            || now.request_id.as_deref() != Some(&id)
            || now.screen_epoch != captured_epoch
            || now.video_codec != snapshot.video_codec
        {
            return Err(assi_session::Error::InvalidState);
        }
        self.engine.publish_video(
            id,
            assi_session::VideoFrame {
                codec: match encoded.codec {
                    assi_video::VideoCodec::Jpeg => assi_session::VideoCodec::Jpeg,
                    assi_video::VideoCodec::H264 => assi_session::VideoCodec::H264,
                },
                keyframe: encoded.keyframe,
                width: encoded.width,
                height: encoded.height,
                timestamp_us: encoded.timestamp_us,
                sequence: 0,
                screen_epoch: captured_epoch,
                bytes: encoded.bytes,
            },
        )
    }
    #[cfg(target_os = "android")]
    async fn video_tick(
        &self,
        snapshot: &Snapshot,
        _timestamp: u64,
        _force: bool,
    ) -> assi_session::Result<()> {
        let _guard = self.lifecycle.lock().await;
        let id = snapshot
            .request_id
            .as_deref()
            .ok_or(assi_session::Error::InvalidState)?;
        let app = self.app.as_ref().ok_or(assi_session::Error::InvalidState)?;
        if let Some(jpeg) = assi_capture::capture_jpeg(app, id)
            .await
            .map_err(|_| assi_session::Error::InvalidFrame)?
        {
            let now = self.engine.snapshot();
            if now.state == SessionState::Active && now.request_id == snapshot.request_id {
                self.engine.publish_frame(jpeg)?;
            }
        }
        Ok(())
    }
    #[cfg(not(target_os = "android"))]
    async fn apply_display_request(&self, id: &str) -> Result<(), String> {
        let engine = self.engine.clone();
        let platform = self.platform.clone();
        let request = id.to_owned();
        let (primary, changed) = tauri::async_runtime::spawn_blocking(move || {
            let mut adapter = platform.lock().map_err(|_| "Ошибка монитора")?;
            let displays = Platform::displays().map_err(|e| e.to_string())?;
            let selected = adapter
                .selected_display()
                .or_else(|| displays.iter().find(|d| d.primary).map(|d| d.id.clone()))
                .ok_or("Монитор не найден")?;
            if engine.snapshot().displays.is_empty() {
                engine
                    .set_displays(
                        request.clone(),
                        displays
                            .iter()
                            .map(|d| assi_session::RemoteDisplay {
                                id: d.id.clone(),
                                name: d.name.clone(),
                                width: d.width,
                                height: d.height,
                                primary: d.primary,
                            })
                            .collect(),
                        selected,
                    )
                    .map_err(|e| e.to_string())?;
            }
            let Some(target) = engine
                .take_display_request(request.clone())
                .map_err(|e| e.to_string())?
            else {
                return Ok::<_, String>((true, false));
            };
            let Some(display) = displays.iter().find(|d| d.id == target) else {
                engine.reject_display(request).map_err(|e| e.to_string())?;
                return Err("Запрошенный монитор отключён".into());
            };
            adapter.release_inputs().map_err(|e| e.to_string())?;
            if let Err(error) = adapter.select_display(target.clone()) {
                engine.reject_display(request).map_err(|e| e.to_string())?;
                return Err(error.to_string());
            }
            engine
                .commit_display(request, target)
                .map_err(|e| e.to_string())?;
            Ok((display.primary, true))
        })
        .await
        .map_err(|_| "Выбор экрана прерван")??;
        if changed {
            self.overlay_display_supported
                .store(primary, Ordering::SeqCst);
            if !primary {
                if let Some(app) = self.app.as_ref().filter(|_| self.is_main()) {
                    let _ = crate::overlay::hide(app);
                }
            }
        }
        Ok(())
    }
    pub(super) async fn preview_jpeg(&self) -> Result<Vec<u8>, String> {
        #[cfg(target_os = "android")]
        {
            Ok(self.engine.latest_frame().unwrap_or_default())
        }
        #[cfg(not(target_os = "android"))]
        {
            let now = self.engine.snapshot();
            if now.role != Role::Host {
                return Ok(self.engine.latest_frame().unwrap_or_default());
            }
            let (request, epoch, raw) = {
                let guard = self.preview.lock().map_err(|_| "Ошибка превью")?;
                let Some(preview) = guard.as_ref() else {
                    return Ok(Vec::new());
                };
                if preview.request != now.request_id.clone().unwrap_or_default()
                    || preview.epoch != now.screen_epoch
                {
                    return Ok(Vec::new());
                }
                if let Some(bytes) = &preview.jpeg {
                    return Ok(bytes.clone());
                }
                (
                    preview.request.clone(),
                    preview.epoch,
                    preview.pixels.clone(),
                )
            };
            let raw_cache = raw.clone();
            let jpeg = tauri::async_runtime::spawn_blocking(move || {
                assi_video::VideoEncoder::new().encode_rgba(
                    raw.width,
                    raw.height,
                    &raw.bytes,
                    0,
                    assi_video::EncoderOptions {
                        max_edge: 1280,
                        ..Default::default()
                    },
                    true,
                )
            })
            .await
            .map_err(|_| "Превью прервано")?
            .map_err(|e| e.to_string())?;
            let now = self.engine.snapshot();
            if now.state != SessionState::Active
                || now.request_id.as_deref() != Some(&request)
                || now.screen_epoch != epoch
            {
                return Ok(Vec::new());
            }
            if let Ok(mut cache) = self.preview.lock() {
                if let Some(preview) = cache.as_mut() {
                    if preview.request == request
                        && preview.epoch == epoch
                        && Arc::ptr_eq(&preview.pixels, &raw_cache)
                    {
                        preview.jpeg = Some(jpeg.bytes.clone());
                    }
                }
            }
            Ok(jpeg.bytes)
        }
    }
}
#[tauri::command]
pub fn session_video(
    remote: crate::sessions::RemoteSession,
    request_id: String,
    after_sequence: u64,
) -> Result<Option<assi_session::VideoFrame>, String> {
    remote
        .engine
        .latest_video(request_id, after_sequence)
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn session_video_codec(
    remote: crate::sessions::RemoteSession,
    request_id: String,
    codec: assi_session::VideoCodec,
) -> Result<(), String> {
    remote
        .engine
        .set_video_codec(request_id, codec)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn session_video_keyframe(
    remote: crate::sessions::RemoteSession,
    request_id: String,
) -> Result<(), String> {
    remote
        .engine
        .request_video_keyframe(request_id)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn session_screen_ack(
    remote: crate::sessions::RemoteSession,
    request_id: String,
    screen_epoch: u64,
) -> Result<(), String> {
    remote
        .engine
        .acknowledge_screen(request_id, screen_epoch)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn session_display(
    remote: crate::sessions::RemoteSession,
    request_id: String,
    display: String,
) -> Result<(), String> {
    remote
        .engine
        .request_display(request_id, display)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn connected() -> (Arc<Remote>, Arc<SessionEngine>) {
        let host = Remote::new(None);
        let viewer = SessionEngine::new();
        let invitation = host
            .engine
            .start_host(
                "127.0.0.1:0".parse().unwrap(),
                "127.0.0.1".parse().unwrap(),
                "Host".into(),
            )
            .await
            .unwrap();
        viewer.connect(invitation, "Viewer".into()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while host.engine.snapshot().state != SessionState::Pending {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        host.engine
            .accept(host.engine.snapshot().request_id.unwrap(), false)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while viewer.snapshot().state != SessionState::Active {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        (host, viewer)
    }
    #[tokio::test]
    async fn old_encoder_failure_cannot_stop_changed_codec_or_replacement_request() {
        let (host, viewer) = connected().await;
        let old = host.engine.snapshot();
        viewer
            .set_video_codec(
                old.request_id.clone().unwrap(),
                assi_session::VideoCodec::H264,
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while host.engine.snapshot().video_codec != assi_session::VideoCodec::H264 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        host.fail_video(&old, "old encoder".into()).await;
        assert_eq!(host.engine.snapshot().state, SessionState::Active);
        assert!(host.error.lock().unwrap().is_none());
        let mut wrong = host.engine.snapshot();
        wrong.request_id = Some("replaced".into());
        host.fail_video(&wrong, "old session".into()).await;
        assert_eq!(host.engine.snapshot().state, SessionState::Active);
        host.shutdown().await.unwrap();
        viewer.stop().await.unwrap();
    }
    #[tokio::test]
    async fn current_encoder_failure_revokes_current_session_without_native_capture() {
        let (host, viewer) = connected().await;
        host.fail_video(&host.engine.snapshot(), "encoder failed".into())
            .await;
        assert_eq!(host.engine.snapshot().state, SessionState::Closed);
        assert_eq!(
            host.error.lock().unwrap().as_deref(),
            Some("encoder failed")
        );
        viewer.stop().await.unwrap();
    }
}
