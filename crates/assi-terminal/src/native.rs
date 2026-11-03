use crate::{valid_size, Error, Result, TerminalBatch, MAX_BUFFER, MAX_CHUNK};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::Duration,
};
enum Command {
    Input(Vec<u8>),
    Resize(u16, u16),
}
#[derive(Default)]
struct Output {
    bytes: VecDeque<u8>,
    closed: bool,
    error: bool,
}
struct Active {
    request: String,
    tx: SyncSender<Command>,
    stop: Arc<AtomicBool>,
    output: Arc<Mutex<Output>>,
    worker: JoinHandle<()>,
}
#[derive(Default)]
pub struct TerminalManager {
    active: Option<Active>,
    retiring: Option<JoinHandle<()>>,
}
fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}
impl TerminalManager {
    pub fn supported() -> bool {
        true
    }
    /// Only call after a local owner action granting this exact session access.
    pub fn start_approved(&mut self, request: &str, cols: u16, rows: u16) -> Result<()> {
        let shell = std::env::var_os("SHELL")
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_absolute() && p.is_file())
            .unwrap_or_else(|| "/bin/sh".into());
        let mut command = CommandBuilder::new(shell);
        command.arg("-i");
        command.env("TERM", "dumb");
        self.start_command(request, cols, rows, command)
    }
    fn start_command(
        &mut self,
        request: &str,
        cols: u16,
        rows: u16,
        command: CommandBuilder,
    ) -> Result<()> {
        if request.is_empty() || request.len() > 128 || !valid_size(cols, rows) {
            return Err(Error::InvalidInput);
        }
        if self.active.is_some() || self.retiring.as_ref().is_some_and(|w| !w.is_finished()) {
            return Err(Error::Busy);
        }
        if let Some(worker) = self.retiring.take() {
            let _ = worker.join();
        }
        let pair = native_pty_system()
            .openpty(size(cols, rows))
            .map_err(|_| Error::Io)?;
        let fd = pair.master.as_raw_fd().ok_or(Error::Unsupported)?;
        // SAFETY: master owns fd throughout worker lifetime; fcntl changes flags only.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                return Err(Error::Io);
            }
        }
        let mut reader = pair.master.try_clone_reader().map_err(|_| Error::Io)?;
        let mut writer = pair.master.take_writer().map_err(|_| Error::Io)?;
        let mut child = pair.slave.spawn_command(command).map_err(|_| Error::Io)?;
        drop(pair.slave);
        let (tx, rx) = mpsc::sync_channel(16);
        let stop = Arc::new(AtomicBool::new(false));
        let output = Arc::new(Mutex::new(Output::default()));
        let worker_stop = stop.clone();
        let worker_output = output.clone();
        let worker = std::thread::spawn(move || {
            let mut pending = VecDeque::new();
            let mut failed = false;
            let mut buffer = [0; MAX_CHUNK];
            while !worker_stop.load(Ordering::Acquire) {
                for _ in 0..16 {
                    match rx.try_recv() {
                        Ok(Command::Input(bytes)) => {
                            if pending.len() + bytes.len() > MAX_BUFFER {
                                failed = true;
                                break;
                            }
                            pending.extend(bytes);
                        }
                        Ok(Command::Resize(cols, rows)) => {
                            if worker_stop.load(Ordering::Acquire) {
                                break;
                            }
                            if pair.master.resize(size(cols, rows)).is_err() {
                                failed = true;
                                break;
                            }
                        }
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            worker_stop.store(true, Ordering::Release);
                            break;
                        }
                    }
                }
                if failed || worker_stop.load(Ordering::Acquire) {
                    break;
                }
                if !pending.is_empty() {
                    match writer.write(pending.make_contiguous()) {
                        Ok(n) if n > 0 => {
                            pending.drain(..n);
                        }
                        Err(e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                || e.kind() == std::io::ErrorKind::Interrupted => {}
                        _ => {
                            failed = true;
                            break;
                        }
                    }
                }
                // Bound work per tick, even for a command producing endless output.
                let mut eof = false;
                for _ in 0..8 {
                    match reader.read(&mut buffer) {
                        Ok(0) => {
                            eof = true;
                            break;
                        }
                        Ok(n) => {
                            let mut out = worker_output.lock().unwrap_or_else(|e| e.into_inner());
                            if out.bytes.len() + n > MAX_BUFFER {
                                failed = true;
                                break;
                            }
                            out.bytes.extend(&buffer[..n]);
                        }
                        Err(e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                || e.kind() == std::io::ErrorKind::Interrupted =>
                        {
                            break
                        }
                        Err(e) if e.raw_os_error() == Some(libc::EIO) => {
                            eof = true;
                            break;
                        }
                        Err(_) => {
                            failed = true;
                            break;
                        }
                    }
                }
                if eof || failed {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            // End the foreground terminal process group as well as the shell.
            if let Some(group) = pair.master.process_group_leader().filter(|p| *p > 1) {
                // SAFETY: group is queried from our owned PTY, not supplied remotely.
                unsafe {
                    libc::kill(-group, libc::SIGKILL);
                }
            }
            if let Some(pid) = child.process_id().filter(|p| *p > 1) {
                // SAFETY: child is still owned and not reaped elsewhere. Ignore ESRCH.
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGKILL);
                }
            }
            let _ = child.wait();
            drop(writer);
            drop(reader);
            drop(pair.master);
            let mut out = worker_output.lock().unwrap_or_else(|e| e.into_inner());
            out.closed = true;
            out.error = failed;
        });
        self.active = Some(Active {
            request: request.into(),
            tx,
            stop,
            output,
            worker,
        });
        Ok(())
    }
    fn active(&self, request: &str) -> Result<&Active> {
        self.active
            .as_ref()
            .filter(|a| a.request == request)
            .ok_or(Error::InvalidState)
    }
    pub fn write(&self, request: &str, bytes: Vec<u8>) -> Result<()> {
        if bytes.is_empty() || bytes.len() > MAX_CHUNK {
            return Err(Error::InvalidInput);
        }
        self.active(request)?
            .tx
            .try_send(Command::Input(bytes))
            .map_err(|_| Error::Busy)
    }
    pub fn resize(&self, request: &str, cols: u16, rows: u16) -> Result<()> {
        if !valid_size(cols, rows) {
            return Err(Error::InvalidInput);
        }
        self.active(request)?
            .tx
            .try_send(Command::Resize(cols, rows))
            .map_err(|_| Error::Busy)
    }
    pub fn drain(&self, request: &str) -> Result<TerminalBatch> {
        let mut out = self.active(request)?.output.lock().map_err(|_| Error::Io)?;
        let len = out.bytes.len().min(MAX_CHUNK);
        let bytes = out.bytes.drain(..len).collect();
        Ok(TerminalBatch {
            bytes,
            closed: out.closed && out.bytes.is_empty(),
            error: out.error,
        })
    }
    /// True only once the old worker has killed/reaped its shell and closed the PTY.
    pub fn is_stopped(&self) -> bool {
        self.active.is_none()
            && self
                .retiring
                .as_ref()
                .is_none_or(|worker| worker.is_finished())
    }
    /// Invoke from a blocking worker, never while holding an async executor thread.
    pub fn finish_stop(&mut self, timeout: Duration) -> bool {
        self.stop();
        let started = std::time::Instant::now();
        while !self.is_stopped() {
            if started.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        if let Some(worker) = self.retiring.take() {
            let _ = worker.join();
        }
        true
    }
    pub fn stop(&mut self) {
        if let Some(active) = self.active.take() {
            active.stop.store(true, Ordering::Release);
            self.retiring = Some(active.worker);
        }
    }
}
impl Drop for TerminalManager {
    fn drop(&mut self) {
        self.stop();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_child_output_and_stale_session_rejected() {
        let mut manager = TerminalManager::default();
        let mut cmd = CommandBuilder::new("/bin/echo");
        cmd.arg("assi-pty-test");
        manager.start_command("request", 80, 24, cmd).unwrap();
        assert!(matches!(
            manager.write("old", vec![1]),
            Err(Error::InvalidState)
        ));
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut bytes = Vec::new();
        loop {
            let batch = manager.drain("request").unwrap();
            bytes.extend(batch.bytes);
            if batch.closed {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(String::from_utf8_lossy(&bytes).contains("assi-pty-test"));
        manager.stop();
        assert!(manager.drain("request").is_err());
    }
    #[test]
    fn invalid_sizes_and_chunks_fail_before_io() {
        let manager = TerminalManager::default();
        assert!(!valid_size(0, 24));
        assert!(!valid_size(80, 500));
        assert!(matches!(
            manager.write("x", vec![0; MAX_CHUNK + 1]),
            Err(Error::InvalidInput)
        ));
    }
    #[test]
    fn stop_terminates_sleeping_child_promptly() {
        let mut manager = TerminalManager::default();
        let mut cmd = CommandBuilder::new("/bin/sleep");
        cmd.arg("10");
        manager.start_command("request", 80, 24, cmd).unwrap();
        manager.stop();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !manager.retiring.as_ref().unwrap().is_finished() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
