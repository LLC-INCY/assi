use super::{float_to_pcm, Result, Shared};
use screencapturekit::{prelude::*, stream::delegate_trait::StreamCallbacks};
use std::{
    sync::{atomic::Ordering, Arc},
    thread,
    time::Duration,
};

pub(super) fn run(shared: Arc<Shared>) -> Result<()> {
    let content = SCShareableContent::get()
        .map_err(|e| format!("Нет доступа к системному звуку macOS: {e}"))?;
    if !shared.active.load(Ordering::SeqCst) {
        return Ok(());
    }
    let display = content
        .displays()
        .into_iter()
        .next()
        .ok_or("Не найден экран для системного звука")?;
    let filter = SCContentFilter::create()
        .with_display(&display)
        .with_excluding_windows(&[])
        .build()
        .map_err(|e| e.to_string())?;
    let config = SCStreamConfiguration::new()
        .with_width(2)
        .with_height(2)
        .with_captures_audio(true)
        .with_sample_rate(48_000)
        .with_channel_count(2)
        .with_excludes_current_process_audio(true);
    let errors = shared.clone();
    let delegate = StreamCallbacks::new().on_stop(move |error| {
        if errors.active.load(Ordering::SeqCst) {
            errors.error(error.unwrap_or_else(|| "Система остановила передачу звука".into()));
        }
    });
    let mut stream =
        SCStream::new_with_delegate(&filter, &config, delegate).map_err(|e| e.to_string())?;
    let samples = shared.clone();
    stream
        .add_output_handler(
            move |sample: CMSampleBuffer, kind: SCStreamOutputType| {
                if kind != SCStreamOutputType::Audio || !samples.active.load(Ordering::SeqCst) {
                    return;
                }
                match decode(&sample) {
                    Ok(bytes) => samples.push(&bytes),
                    Err(error) => samples.error(error),
                }
            },
            SCStreamOutputType::Audio,
        )
        .map_err(|e| e.to_string())?;
    if !shared.active.load(Ordering::SeqCst) {
        return Ok(());
    }
    stream
        .start_capture()
        .map_err(|e| format!("Не удалось включить системный звук: {e}"))?;
    while shared.active.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(20));
    }
    stream.stop_capture().map_err(|e| e.to_string())
}
fn decode(sample: &CMSampleBuffer) -> Result<Vec<u8>> {
    let format = sample
        .format_description()
        .ok_or("Неизвестный формат звука")?;
    if format.audio_sample_rate() != Some(48_000.0)
        || format.audio_channel_count() != Some(2)
        || format.audio_bits_per_channel() != Some(32)
        || !format.audio_is_float()
        || format.audio_is_big_endian()
    {
        return Err(
            "Неподдерживаемый формат системного звука (ожидается 48 кГц stereo float32)".into(),
        );
    }
    let count = usize::try_from(sample.num_samples()).map_err(|_| "Некорректный размер аудио")?;
    if count == 0 {
        return Ok(Vec::new());
    }
    if count > 48_000 {
        return Err("Системный аудиобуфер слишком большой".into());
    }
    let buffers = sample
        .audio_buffer_list()
        .map_err(|_| "Не удалось прочитать системный звук")?;
    let mut out = Vec::with_capacity(count * 4);
    match buffers.num_buffers() {
        1 => {
            let b = buffers.get(0).ok_or("Нет аудиобуфера")?;
            if b.number_channels != 2 || b.data().len() != count * 8 {
                return Err("Неполный stereo аудиобуфер".into());
            }
            for value in b.data().chunks_exact(4) {
                out.extend_from_slice(&float_to_pcm(f32::from_le_bytes(
                    value.try_into().map_err(|_| "Неверный PCM")?,
                )));
            }
        }
        2 => {
            let a = buffers.get(0).ok_or("Нет левого канала")?;
            let b = buffers.get(1).ok_or("Нет правого канала")?;
            if a.number_channels != 1
                || b.number_channels != 1
                || a.data().len() != count * 4
                || b.data().len() != count * 4
            {
                return Err("Неполный planar аудиобуфер".into());
            }
            for (left, right) in a.data().chunks_exact(4).zip(b.data().chunks_exact(4)) {
                for value in [left, right] {
                    out.extend_from_slice(&float_to_pcm(f32::from_le_bytes(
                        value.try_into().map_err(|_| "Неверный PCM")?,
                    )));
                }
            }
        }
        _ => return Err("Неподдерживаемое число аудиобуферов".into()),
    }
    Ok(out)
}
