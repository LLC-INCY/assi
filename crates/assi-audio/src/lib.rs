//! Opt-in system output capture; never substitutes a microphone.
//! Construction, queue access and tests do not open an audio device.
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;
pub const FRAME_BYTES: usize = 960 * CHANNELS as usize * 2;
#[cfg(any(target_os = "macos", target_os = "windows", test))]
const QUEUE_FRAMES: usize = 10;
pub type Result<T> = std::result::Result<T, String>;

#[derive(Default)]
struct Buffer {
    partial: Vec<u8>,
    frames: VecDeque<Vec<u8>>,
    error: Option<String>,
}
#[derive(Default)]
struct Shared {
    active: AtomicBool,
    buffer: Mutex<Buffer>,
}
impl Shared {
    fn error(&self, error: String) {
        self.active.store(false, Ordering::SeqCst);
        if let Ok(mut buffer) = self.buffer.lock() {
            if buffer.error.is_none() {
                buffer.error = Some(error);
            }
            buffer.frames.clear();
            buffer.partial.clear();
        }
    }
    // Native callbacks must not wait behind the UI/IPC consumer.
    #[cfg(any(target_os = "macos", target_os = "windows", test))]
    fn push(&self, bytes: &[u8]) {
        if !self.active.load(Ordering::SeqCst)
            || bytes.len() > 384_000
            || !bytes.len().is_multiple_of(4)
        {
            return;
        }
        if let Ok(mut buffer) = self.buffer.try_lock() {
            if !self.active.load(Ordering::SeqCst) {
                return;
            }
            for chunk in bytes.chunks(FRAME_BYTES) {
                buffer.partial.extend_from_slice(chunk);
                while buffer.partial.len() >= FRAME_BYTES {
                    let rest = buffer.partial.split_off(FRAME_BYTES);
                    let frame = std::mem::replace(&mut buffer.partial, rest);
                    if buffer.frames.len() == QUEUE_FRAMES {
                        buffer.frames.pop_front();
                    }
                    buffer.frames.push_back(frame);
                }
            }
        }
    }
}

pub struct AudioCapture {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}
impl Default for AudioCapture {
    fn default() -> Self {
        Self {
            shared: Arc::new(Shared::default()),
            worker: None,
        }
    }
}
impl AudioCapture {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn supported() -> bool {
        cfg!(any(target_os = "macos", target_os = "windows"))
    }
    /// Dispatches capture startup without waiting for OS callbacks. Call only
    /// after owner consent. Permission/startup failures arrive via take_error().
    pub fn start(&mut self) -> Result<()> {
        if !Self::supported() {
            return Err("Захват системного звука на этой платформе пока недоступен".into());
        }
        if self.worker.is_some() && self.shared.active.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.stop();
        if self.worker.is_some() {
            return Err(
                "Предыдущая передача звука ещё завершается; повторите через несколько секунд"
                    .into(),
            );
        }
        // New epoch: delayed callbacks from a stopped worker cannot feed this queue.
        self.shared = Arc::new(Shared::default());
        self.shared.active.store(true, Ordering::SeqCst);
        let shared = self.shared.clone();
        self.worker = Some(
            thread::Builder::new()
                .name("assi-system-audio".into())
                .spawn(move || {
                    #[cfg(target_os = "macos")]
                    let outcome = macos::run(shared.clone());
                    #[cfg(target_os = "windows")]
                    let outcome = windows::run(shared.clone());
                    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                    let outcome: Result<()> = Err("Захват системного звука недоступен".into());
                    if let Err(error) = outcome {
                        shared.error(error);
                    }
                })
                .map_err(|e| {
                    self.shared.active.store(false, Ordering::SeqCst);
                    e.to_string()
                })?,
        );
        Ok(())
    }

    /// Immediately revokes callback publication. OS teardown runs on the capture
    /// worker. An unfinished worker is retained to prevent overlapping starts;
    /// dropping this adapter detaches it, but its revoked queue cannot publish.
    pub fn stop(&mut self) {
        self.shared.active.store(false, Ordering::SeqCst);
        if let Ok(mut buffer) = self.shared.buffer.lock() {
            buffer.frames.clear();
            buffer.partial.clear();
        }
        if let Some(worker) = self.worker.take() {
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                self.worker = Some(worker);
            }
        }
    }
    pub fn is_running(&self) -> bool {
        self.shared.active.load(Ordering::SeqCst)
    }
    /// Returns at most 200ms of PCM. Oldest complete frames are dropped on backpressure.
    pub fn drain(&mut self) -> Vec<Vec<u8>> {
        if !self.is_running() {
            return Vec::new();
        }
        self.shared
            .buffer
            .try_lock()
            .map(|mut b| b.frames.drain(..).collect())
            .unwrap_or_default()
    }
    pub fn take_error(&mut self) -> Option<String> {
        self.shared
            .buffer
            .lock()
            .ok()
            .and_then(|mut b| b.error.take())
    }
}
impl Drop for AudioCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(any(target_os = "macos", test))]
fn float_to_pcm(sample: f32) -> [u8; 2] {
    let value = if sample.is_finite() {
        (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16
    } else {
        0
    };
    value.to_le_bytes()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn construction_and_stop_are_lazy() {
        let mut capture = AudioCapture::new();
        assert!(!capture.is_running());
        assert!(capture.drain().is_empty());
        capture.stop();
        assert!(capture.take_error().is_none());
    }
    #[test]
    fn partial_callback_buffers_form_exact_frames() {
        let shared = Shared::default();
        shared.active.store(true, Ordering::SeqCst);
        shared.push(&vec![1; 400]);
        shared.push(&vec![2; FRAME_BYTES - 400]);
        let b = shared.buffer.lock().unwrap();
        assert_eq!(b.frames.len(), 1);
        assert_eq!(b.frames[0].len(), FRAME_BYTES);
        assert_eq!(b.frames[0][399], 1);
        assert_eq!(b.frames[0][400], 2);
        assert!(b.partial.is_empty());
    }
    #[test]
    fn queue_drops_oldest_and_revocation_discards_late_callbacks() {
        let shared = Shared::default();
        shared.active.store(true, Ordering::SeqCst);
        for n in 0..20 {
            shared.push(&vec![n; FRAME_BYTES]);
        }
        {
            let b = shared.buffer.lock().unwrap();
            assert_eq!(b.frames.len(), QUEUE_FRAMES);
            assert_eq!(b.frames[0][0], 10);
        }
        shared.active.store(false, Ordering::SeqCst);
        shared.push(&vec![99; FRAME_BYTES]);
        assert_eq!(shared.buffer.lock().unwrap().frames.back().unwrap()[0], 19);
    }
    #[test]
    fn clipping_and_nonfinite_samples_are_safe() {
        assert_eq!(i16::from_le_bytes(float_to_pcm(2.0)), 32767);
        assert_eq!(i16::from_le_bytes(float_to_pcm(-2.0)), -32767);
        assert_eq!(float_to_pcm(f32::NAN), [0, 0]);
        assert_eq!(float_to_pcm(f32::INFINITY), [0, 0]);
    }
    #[test]
    fn malformed_native_buffers_do_not_grow_queue() {
        let s = Shared::default();
        s.active.store(true, Ordering::SeqCst);
        s.push(&[0, 1, 2]);
        s.push(&vec![0; 384_004]);
        assert!(s.buffer.lock().unwrap().frames.is_empty());
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    #[test]
    fn stop_revokes_pending_worker_without_waiting_and_blocks_restart() {
        let (send, receive) = std::sync::mpsc::channel();
        let mut capture = AudioCapture::new();
        capture.shared.active.store(true, Ordering::SeqCst);
        capture.worker = Some(thread::spawn(move || {
            receive.recv().unwrap();
        }));
        capture.stop();
        assert!(!capture.is_running());
        assert!(capture.worker.is_some());
        assert!(capture.start().is_err());
        assert!(capture.drain().is_empty());
        send.send(()).unwrap();
        capture.worker.take().unwrap().join().unwrap();
    }
}
