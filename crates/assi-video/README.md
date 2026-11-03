# assi-video

Approved RGBA pixels → software H.264 Annex B or JPEG. The crate never captures
screens, requests permissions, opens audio devices, or transmits anything.

`VideoEncoder::encode_rgba(width, height, pixels, timestamp_us, options, force_keyframe)`
returns an `EncodedFrame`. Its `codec` is authoritative: an unavailable or failed
H.264 backend falls back to independent JPEG frames. A failed backend is not retried
on every frame; changing options or resetting starts it afresh.

`VideoError::Skipped` is a normal rate-control outcome: keep the prior displayed
image and capture again on schedule. Do not terminate the session or assign a
transport sequence to a skipped capture. If the skipped frame was requested as a
keyframe, preserve that request for the next capture.

## Integration

1. Capture with `assi_platform::Platform::capture_rgba` while holding the platform
   lock, then release it. Compression uses a separate encoder lock so it cannot
   hold up input injection.
2. Default to JPEG. Enable H.264 only after the viewer reports decoder capability.
3. Reset the encoder between sessions and screen epochs. Preserve ordered frames
   through the transport. On an overflow or missing reference, discard dependent
   frames and request a fresh keyframe rather than decoding stale deltas.
4. The browser helper `AssiVideoDecoder` in `session-video.js` probes WebCodecs,
   reads the codec profile from the SPS, draws to a supplied canvas and closes
   every `VideoFrame`. Its bounded pending state resets on loss, resize, screen
   epoch change or decoder backpressure. Decoder errors must select JPEG in the UI.

## Limits and implementation

- Source RGBA: at most 16,384 per edge and 33,554,432 pixels; exact byte count.
- Output: maximum configured edge 2,560; at most 4 MiB per encoded frame.
- Configuration: 1–30 FPS, 128 kbit/s–12 Mbit/s target, JPEG quality 1–100.
- H.264: OpenH264 0.9.9 built from source, Baseline profile, level 5.0, two CPU
  threads, screen-content mode, periodic IDR every two seconds plus forced recovery.
  Rate control may skip frames; bitrate is a target, not a per-packet bandwidth cap.
- Odd dimensions are resampled to even I420 dimensions, preserving the full screen
  for normalized input coordinates. Sources below 16 pixels in either dimension
  use JPEG.
- H.264 encoder builds on macOS/Windows/Linux; other targets use JPEG. The software
  encoder is not a hardware encoder. The browser may choose a hardware decoder,
  but neither support nor acceleration is assumed without runtime capability checks.
- No automatic bandwidth estimator, hardware encoder, native GPU capture, audio
  muxing or recording container is implemented in this crate.

Tests encode and decode synthetic images, check keyframes/resizing and validate
dimensions/options. They do not verify physical display capture or real network
smoothness. Keep a two-device test in release acceptance.

Primary API references: [OpenH264 Rust encoder](https://docs.rs/openh264/0.9.9/openh264/encoder/struct.Encoder.html),
[WebCodecs AVC registration](https://www.w3.org/TR/webcodecs-avc-codec-registration/).
OpenH264 Rust bindings and bundled source have their own license notices in the
dependency distribution; source compilation is used, not a downloaded Cisco binary.
