use super::{Result, Shared};
use std::{
    sync::{atomic::Ordering, Arc},
    thread,
    time::Duration,
};
use windows::Win32::{Media::Audio::*, System::Com::*};
struct Com;
impl Drop for Com {
    fn drop(&mut self) {
        /* SAFETY: created only after successful CoInitializeEx on this worker. */
        unsafe {
            CoUninitialize();
        }
    }
}
struct Running(IAudioClient);
impl Drop for Running {
    fn drop(&mut self) {
        /* SAFETY: owns a started COM client on its original worker. */
        let _ = unsafe { self.0.Stop() };
    }
}
struct Packet<'a> {
    client: &'a IAudioCaptureClient,
    frames: u32,
}
impl Drop for Packet<'_> {
    fn drop(&mut self) {
        /* SAFETY: exactly one ReleaseBuffer per successful GetBuffer, before next read. */
        let _ = unsafe { self.client.ReleaseBuffer(self.frames) };
    }
}
pub(super) fn run(shared: Arc<Shared>) -> Result<()> {
    // SAFETY: this dedicated thread initializes COM once and retains all interfaces
    // here; the Com guard is dropped after all audio interfaces.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(|e| e.to_string())?;
    let _com = Com;
    // SAFETY: class/interface identifiers are supplied by windows-rs; no aggregation.
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|e| e.to_string())?;
    // Explicit Render endpoint is essential: Capture would record a microphone.
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
        .map_err(|e| e.to_string())?;
    let client: IAudioClient =
        unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(|e| e.to_string())?;
    let format = WAVEFORMATEX {
        wFormatTag: 1,
        nChannels: 2,
        nSamplesPerSec: 48_000,
        nAvgBytesPerSec: 192_000,
        nBlockAlign: 4,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    // SAFETY: PCM format and byte alignment match the parser, pointer remains valid
    // throughout Initialize. Shared-mode conversion handles hardware formats.
    unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK
                | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
            1_000_000,
            0,
            &format,
            None,
        )
    }
    .map_err(|e| format!("Не удалось включить WASAPI loopback: {e}"))?;
    let capture: IAudioCaptureClient = unsafe { client.GetService() }.map_err(|e| e.to_string())?;
    if !shared.active.load(Ordering::SeqCst) {
        return Ok(());
    }
    unsafe { client.Start() }.map_err(|e| e.to_string())?;
    let _running = Running(client);
    while shared.active.load(Ordering::SeqCst) {
        for _ in 0..32 {
            if !shared.active.load(Ordering::SeqCst) {
                break;
            }
            let available = unsafe { capture.GetNextPacketSize() }.map_err(|e| e.to_string())?;
            if available == 0 {
                break;
            }
            let (mut ptr, mut frames, mut flags) = (std::ptr::null_mut(), 0, 0);
            // SAFETY: pointers refer to initialized local out parameters; buffer
            // lifetime is held by the following Packet guard until it is copied.
            unsafe { capture.GetBuffer(&mut ptr, &mut frames, &mut flags, None, None) }
                .map_err(|e| e.to_string())?;
            let _packet = Packet {
                client: &capture,
                frames,
            };
            if frames > 48_000 {
                return Err("Системный аудиобуфер слишком большой".into());
            }
            if frames == 0 {
                continue;
            }
            let length = frames as usize * 4;
            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                shared.push(&vec![0; length]);
            } else {
                if ptr.is_null() {
                    return Err("Система вернула пустой аудиобуфер".into());
                }
                // SAFETY: WASAPI guarantees frames*nBlockAlign readable bytes for
                // a non-silent buffer until ReleaseBuffer. Silence never reads ptr.
                shared.push(unsafe { std::slice::from_raw_parts(ptr, length) });
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
