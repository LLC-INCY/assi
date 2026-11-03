use std::collections::HashSet;

use enigo::{Axis, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use image::{codecs::jpeg::JpegEncoder, DynamicImage, RgbaImage};
use xcap::Monitor;

use crate::{
    CaptureQuality, CapturedRgba, DisplayInfo, InputEvent, MouseButton, PlatformError, Result,
    MAX_FRAME_BYTES,
};

#[derive(Default)]
pub(super) struct Desktop {
    // Native monitor handles (HMONITOR on Windows) never cross worker threads.
    monitor: Option<MonitorSnapshot>,
    selected_id: Option<u32>,
    pub(super) quality: CaptureQuality,
    input: Option<Enigo>,
    keys: HashSet<Key>,
    buttons: HashSet<MouseButton>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MonitorSnapshot {
    id: u32,
    name: String,
    rect: Rect,
}

impl MonitorSnapshot {
    fn read(monitor: &Monitor) -> Result<Self> {
        Ok(Self {
            id: monitor
                .id()
                .map_err(|_| PlatformError::MonitorUnavailable)?,
            name: monitor
                .name()
                .map_err(|_| PlatformError::MonitorUnavailable)?,
            rect: Rect::read(monitor)?,
        })
    }

    fn validate(&self, current: &Self) -> Result<()> {
        if self == current {
            Ok(())
        } else {
            Err(PlatformError::MonitorChanged)
        }
    }

    fn resolve(&self) -> Result<Monitor> {
        let monitors = Monitor::all().map_err(|_| PlatformError::MonitorUnavailable)?;
        let monitor = unique_monitor(monitors, self.id, |monitor| {
            monitor.id().map_err(|_| PlatformError::MonitorUnavailable)
        })?;
        self.validate(&Self::read(&monitor)?)?;
        Ok(monitor)
    }
}

// Never fall back to another/primary display after capture established a binding.
// Reject ambiguous IDs rather than choosing an arbitrary native handle.
fn unique_monitor<T>(
    monitors: impl IntoIterator<Item = T>,
    expected_id: u32,
    mut id: impl FnMut(&T) -> Result<u32>,
) -> Result<T> {
    let mut selected = None;
    for monitor in monitors {
        if id(&monitor)? == expected_id && selected.replace(monitor).is_some() {
            return Err(PlatformError::MonitorChanged);
        }
    }
    selected.ok_or(PlatformError::MonitorChanged)
}

impl Rect {
    fn read(monitor: &Monitor) -> Result<Self> {
        let error = |_| PlatformError::MonitorUnavailable;
        let rect = Self {
            x: monitor.x().map_err(error)?,
            y: monitor.y().map_err(error)?,
            width: monitor.width().map_err(error)?,
            height: monitor.height().map_err(error)?,
        };
        // xcap normalizes X11 geometry by Xft scale; XTEST (enigo's
        // X11 backend) consumes root-window pixels. CoreGraphics already uses
        // points and Windows xcap returns pixels, so neither is rescaled.
        #[cfg(target_os = "linux")]
        let rect = rect.x11_pixels(f64::from(monitor.scale_factor().map_err(error)?))?;
        if rect.width == 0
            || rect.height == 0
            || i64::from(rect.x) + i64::from(rect.width) > i64::from(i32::MAX)
            || i64::from(rect.y) + i64::from(rect.height) > i64::from(i32::MAX)
        {
            return Err(PlatformError::MonitorUnavailable);
        }
        Ok(rect)
    }

    #[cfg(any(target_os = "linux", test))]
    fn x11_pixels(self, scale: f64) -> Result<Self> {
        let values = [
            f64::from(self.x),
            f64::from(self.y),
            f64::from(self.width),
            f64::from(self.height),
        ]
        .map(|value| (value * scale).round());
        if !scale.is_finite()
            || scale <= 0.0
            || values.iter().any(|value| {
                !value.is_finite() || *value < f64::from(i32::MIN) || *value > f64::from(i32::MAX)
            })
        {
            return Err(PlatformError::MonitorUnavailable);
        }
        Ok(Self {
            x: values[0] as i32,
            y: values[1] as i32,
            width: values[2] as u32,
            height: values[3] as u32,
        })
    }

    fn point(self, x: f64, y: f64) -> Result<(i32, i32)> {
        if !x.is_finite()
            || !y.is_finite()
            || !(0.0..=1.0).contains(&x)
            || !(0.0..=1.0).contains(&y)
        {
            return Err(PlatformError::InvalidCoordinates);
        }
        Ok((
            (f64::from(self.x) + x * f64::from(self.width - 1)).round() as i32,
            (f64::from(self.y) + y * f64::from(self.height - 1)).round() as i32,
        ))
    }
}

pub(super) fn is_wayland() -> bool {
    cfg!(target_os = "linux")
        && (std::env::var_os("WAYLAND_DISPLAY").is_some()
            || std::env::var("XDG_SESSION_TYPE")
                .is_ok_and(|value| value.eq_ignore_ascii_case("wayland")))
}

fn display_info(monitor: &Monitor) -> Result<DisplayInfo> {
    let rect = Rect::read(monitor)?;
    Ok(DisplayInfo {
        id: monitor
            .id()
            .map_err(|_| PlatformError::MonitorUnavailable)?
            .to_string(),
        name: monitor
            .name()
            .unwrap_or_else(|_| "Экран".into())
            .chars()
            .filter(|c| !c.is_control())
            .take(128)
            .collect(),
        width: rect.width,
        height: rect.height,
        primary: monitor
            .is_primary()
            .map_err(|_| PlatformError::MonitorUnavailable)?,
    })
}

pub(super) fn displays() -> Result<Vec<DisplayInfo>> {
    Monitor::all()
        .map_err(|_| PlatformError::MonitorUnavailable)?
        .iter()
        .take(32)
        .map(display_info)
        .collect()
}

fn parse_display_id(id: &str) -> Result<u32> {
    if id.is_empty() || id.len() > 10 || !id.bytes().all(|c| c.is_ascii_digit()) {
        return Err(PlatformError::InvalidDisplay);
    }
    id.parse().map_err(|_| PlatformError::InvalidDisplay)
}

impl Desktop {
    pub(super) fn select_display(&mut self, id: String) -> Result<()> {
        let id = parse_display_id(&id)?;
        let monitor = Monitor::all()
            .map_err(|_| PlatformError::MonitorUnavailable)?
            .into_iter()
            .find(|monitor| monitor.id().ok() == Some(id))
            .ok_or(PlatformError::InvalidDisplay)?;
        Rect::read(&monitor)?;
        // Keep the prior display when input release fails; never silently redirect input.
        self.release_inputs()?;
        self.selected_id = Some(id);
        Ok(())
    }

    pub(super) fn selected_display(&self) -> Option<String> {
        self.selected_id
            .or_else(|| self.monitor.as_ref().map(|monitor| monitor.id))
            .map(|id| id.to_string())
    }

    pub(super) fn capture_jpeg(&mut self) -> Result<Vec<u8>> {
        let frame = self.capture_rgba()?;
        let image = RgbaImage::from_raw(frame.width, frame.height, frame.bytes)
            .ok_or(PlatformError::EncodeFailed)?;
        encode_frame(image, self.quality)
    }

    pub(super) fn capture_rgba(&mut self) -> Result<CapturedRgba> {
        let monitor = match self.monitor.as_ref() {
            Some(snapshot) => snapshot.resolve()?,
            None => {
                let monitors = Monitor::all().map_err(|_| PlatformError::CaptureUnavailable)?;
                if let Some(id) = self.selected_id {
                    unique_monitor(monitors, id, |monitor| {
                        monitor.id().map_err(|_| PlatformError::MonitorUnavailable)
                    })?
                } else {
                    monitors
                        .into_iter()
                        .find(|monitor| monitor.is_primary().unwrap_or(false))
                        .ok_or(PlatformError::NoMonitor)?
                }
            }
        };
        let snapshot = MonitorSnapshot::read(&monitor)?;
        if let Some(expected) = self.monitor.as_ref() {
            expected.validate(&snapshot)?;
        }
        let image = monitor
            .capture_image()
            .map_err(|_| PlatformError::CaptureUnavailable)?;
        // A display reconfiguration during capture must not establish new input
        // coordinates for an image from the previous configuration.
        snapshot.validate(&MonitorSnapshot::read(&monitor)?)?;
        let frame = captured_rgba(image)?;
        self.monitor = Some(snapshot);
        Ok(frame)
    }

    pub(super) fn apply_input(&mut self, event: InputEvent) -> Result<()> {
        if is_wayland() {
            return Err(PlatformError::WaylandControlUnsupported);
        }
        let snapshot = self
            .monitor
            .as_ref()
            .ok_or(PlatformError::CaptureRequired)?;
        // Resolve on this worker; only portable identity/geometry is retained.
        let _monitor = snapshot.resolve()?;
        let rect = snapshot.rect;
        // Validate before creating the native input connection.
        if let InputEvent::Wheel { x, y } = &event {
            validate_scroll(*x, *y)?;
        }
        if let InputEvent::Text { text } = &event {
            validate_text(text)?;
        }
        let point = match &event {
            InputEvent::Move { x, y } => Some(rect.point(*x, *y)?),
            _ => None,
        };
        let key = match &event {
            InputEvent::Key { key, .. } => Some(parse_key(key)?),
            _ => None,
        };
        if self.input.is_none() {
            self.input = Some(
                Enigo::new(&Settings {
                    open_prompt_to_get_permissions: false,
                    release_keys_when_dropped: true,
                    ..Settings::default()
                })
                .map_err(|_| PlatformError::InputUnavailable)?,
            );
        }
        // Text is literal input, not a chord. Release held remote modifiers/buttons
        // first so a stale Alt/Control cannot turn Unicode text into shortcuts.
        if matches!(&event, InputEvent::Text { .. }) {
            self.release_held()?;
        }
        let input = self.input.as_mut().ok_or(PlatformError::InputUnavailable)?;
        let error = |_| PlatformError::InputUnavailable;
        match event {
            InputEvent::Text { text } => {
                input.text(&text).map_err(error)?;
            }
            InputEvent::Move { .. } => {
                let (x, y) = point.ok_or(PlatformError::InvalidCoordinates)?;
                input.move_mouse(x, y, Coordinate::Abs).map_err(error)?;
            }
            InputEvent::Wheel { x, y } => {
                if x != 0 {
                    input.scroll(x, Axis::Horizontal).map_err(error)?;
                }
                if y != 0 {
                    input.scroll(y, Axis::Vertical).map_err(error)?;
                }
            }
            InputEvent::Button { button, down } => {
                // Record before dispatch: even a partially failed native press
                // must be released on shutdown.
                if down {
                    self.buttons.insert(button);
                }
                input
                    .button(native_button(button), direction(down))
                    .map_err(error)?;
                if !down {
                    self.buttons.remove(&button);
                }
            }
            InputEvent::Key { down, .. } => {
                let key = key.ok_or(PlatformError::InvalidKey)?;
                if down {
                    self.keys.insert(key);
                }
                input.key(key, direction(down)).map_err(error)?;
                if !down {
                    self.keys.remove(&key);
                }
            }
        }
        Ok(())
    }

    fn release_held(&mut self) -> Result<()> {
        let mut failed = false;
        if let Some(input) = self.input.as_mut() {
            for key in self.keys.drain() {
                failed |= input.key(key, Direction::Release).is_err();
            }
            for button in self.buttons.drain() {
                failed |= input
                    .button(native_button(button), Direction::Release)
                    .is_err();
            }
        }
        if failed {
            Err(PlatformError::InputUnavailable)
        } else {
            Ok(())
        }
    }
    pub(super) fn release_inputs(&mut self) -> Result<()> {
        let result = self.release_held();
        self.input = None;
        self.monitor = None;
        result
    }
}

fn captured_rgba(image: RgbaImage) -> Result<CapturedRgba> {
    let (width, height) = image.dimensions();
    if width == 0
        || height == 0
        || width > 16_384
        || height > 16_384
        || u64::from(width) * u64::from(height) > 33_554_432
    {
        return Err(PlatformError::FrameTooLarge);
    }
    Ok(CapturedRgba {
        width,
        height,
        bytes: image.into_raw(),
    })
}

impl Drop for Desktop {
    fn drop(&mut self) {
        let _ = self.release_inputs();
    }
}

fn validate_text(text: &str) -> Result<()> {
    if text.is_empty()
        || text.len() > 4096
        || text
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        Err(PlatformError::InvalidText)
    } else {
        Ok(())
    }
}

fn validate_scroll(x: i32, y: i32) -> Result<()> {
    if !(-100..=100).contains(&x) || !(-100..=100).contains(&y) {
        Err(PlatformError::InvalidScroll)
    } else {
        Ok(())
    }
}

fn direction(down: bool) -> Direction {
    if down {
        Direction::Press
    } else {
        Direction::Release
    }
}

fn native_button(button: MouseButton) -> enigo::Button {
    match button {
        MouseButton::Left => enigo::Button::Left,
        MouseButton::Right => enigo::Button::Right,
        MouseButton::Middle => enigo::Button::Middle,
    }
}

fn parse_key(key: &str) -> Result<Key> {
    Ok(match key {
        "Enter" | "enter" => Key::Return,
        "Escape" | "escape" => Key::Escape,
        "Backspace" | "backspace" => Key::Backspace,
        "Tab" | "tab" => Key::Tab,
        "Space" | "space" | " " => Key::Space,
        "Delete" | "delete" => Key::Delete,
        "ArrowLeft" | "arrow_left" => Key::LeftArrow,
        "ArrowRight" | "arrow_right" => Key::RightArrow,
        "ArrowUp" | "arrow_up" => Key::UpArrow,
        "ArrowDown" | "arrow_down" => Key::DownArrow,
        "Home" | "home" => Key::Home,
        "End" | "end" => Key::End,
        "PageUp" | "page_up" => Key::PageUp,
        "PageDown" | "page_down" => Key::PageDown,
        "Control" | "control" => Key::Control,
        "Shift" | "shift" => Key::Shift,
        "Alt" | "alt" => Key::Alt,
        "Meta" | "meta" => Key::Meta,
        #[cfg(not(target_os = "macos"))]
        "Insert" | "insert" => Key::Insert,
        // Extended Mac keyboards use Help at the PC Insert key position.
        #[cfg(target_os = "macos")]
        "Insert" | "insert" => Key::Help,
        "CapsLock" | "caps_lock" => Key::CapsLock,
        "F1" | "f1" => Key::F1,
        "F2" | "f2" => Key::F2,
        "F3" | "f3" => Key::F3,
        "F4" | "f4" => Key::F4,
        "F5" | "f5" => Key::F5,
        "F6" | "f6" => Key::F6,
        "F7" | "f7" => Key::F7,
        "F8" | "f8" => Key::F8,
        "F9" | "f9" => Key::F9,
        "F10" | "f10" => Key::F10,
        "F11" | "f11" => Key::F11,
        "F12" | "f12" => Key::F12,
        // A finite ASCII printable alphabet. Never accepts raw keycodes, scripts,
        // arbitrary text, or system commands.
        value if value.len() == 1 && value.as_bytes()[0].is_ascii_graphic() => {
            Key::Unicode(char::from(value.as_bytes()[0]))
        }
        _ => return Err(PlatformError::InvalidKey),
    })
}

fn encode_frame(image: RgbaImage, quality: CaptureQuality) -> Result<Vec<u8>> {
    if image.width() == 0 || image.height() == 0 {
        return Err(PlatformError::EncodeFailed);
    }
    let mut image = DynamicImage::ImageRgba8(image);
    if image.width().max(image.height()) > quality.max_edge() {
        image = image.resize(
            quality.max_edge(),
            quality.max_edge(),
            image::imageops::FilterType::Triangle,
        );
    }
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, quality.jpeg_quality())
        .encode_image(&image.to_rgb8())
        .map_err(|_| PlatformError::EncodeFailed)?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(PlatformError::FrameTooLarge);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_can_move_between_serialized_workers() {
        fn assert_send<T: Send>() {}
        assert_send::<crate::Platform>();
    }

    #[test]
    fn captured_monitor_resolution_never_falls_back_or_accepts_ambiguous_id() {
        assert_eq!(unique_monitor([3, 1, 2], 1, |id| Ok(*id)), Ok(1));
        assert_eq!(
            unique_monitor([2, 3], 1, |id| Ok(*id)),
            Err(PlatformError::MonitorChanged)
        );
        assert_eq!(
            unique_monitor([1, 2, 1], 1, |id| Ok(*id)),
            Err(PlatformError::MonitorChanged)
        );
        assert_eq!(
            unique_monitor([1], 1, |_| Err(PlatformError::MonitorUnavailable)),
            Err(PlatformError::MonitorUnavailable)
        );
    }

    #[test]
    fn captured_monitor_binding_rejects_identity_and_geometry_changes() {
        let expected = MonitorSnapshot {
            id: 7,
            name: "display-7".into(),
            rect: Rect {
                x: -1920,
                y: 0,
                width: 1920,
                height: 1080,
            },
        };
        assert_eq!(expected.validate(&expected), Ok(()));
        let mut changes = vec![expected.clone(); 6];
        changes[0].id = 8;
        changes[1].name = "replacement".into();
        changes[2].rect.x = 0;
        changes[3].rect.y = 20;
        changes[4].rect.width = 2560;
        changes[5].rect.height = 1440;
        for changed in changes {
            assert_eq!(
                expected.validate(&changed),
                Err(PlatformError::MonitorChanged)
            );
        }
        let mut platform = crate::Platform::new();
        platform.inner.monitor = Some(expected);
        assert_eq!(platform.selected_display().as_deref(), Some("7"));
        platform.release_inputs().unwrap();
        assert!(platform.selected_display().is_none());
        assert!(platform.inner.input.is_none());
    }

    #[test]
    fn normalized_coordinates_use_monitor_origin_and_last_pixel() {
        let rect = Rect {
            x: -1920,
            y: 20,
            width: 1920,
            height: 1080,
        };
        assert_eq!(rect.point(0.0, 0.0).unwrap(), (-1920, 20));
        assert_eq!(rect.point(1.0, 1.0).unwrap(), (-1, 1099));
        assert_eq!(rect.point(0.5, 0.5).unwrap(), (-961, 560));
    }

    #[test]
    fn rejects_non_finite_and_out_of_bounds_pointer_values() {
        let rect = Rect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.01, 1.01] {
            assert_eq!(
                rect.point(value, 0.0),
                Err(PlatformError::InvalidCoordinates)
            );
            assert_eq!(
                rect.point(0.0, value),
                Err(PlatformError::InvalidCoordinates)
            );
        }
        assert_eq!(rect.point(1.0, 1.0).unwrap(), (0, 0));
    }

    #[test]
    fn x11_scaled_geometry_restores_native_pixels() {
        let rect = Rect {
            x: -1280,
            y: 0,
            width: 1280,
            height: 720,
        };
        assert_eq!(
            rect.x11_pixels(1.5).unwrap(),
            Rect {
                x: -1920,
                y: 0,
                width: 1920,
                height: 1080
            }
        );
        assert_eq!(
            rect.x11_pixels(f64::NAN),
            Err(PlatformError::MonitorUnavailable)
        );
        assert_eq!(
            rect.x11_pixels(-1.0),
            Err(PlatformError::MonitorUnavailable)
        );
    }

    #[test]
    fn keys_are_finite_and_do_not_accept_text_or_raw_codes() {
        for key in [
            "Enter",
            "Control",
            "ArrowLeft",
            "F12",
            "a",
            "Z",
            "9",
            "!",
            "enter",
            "arrow_left",
            "page_down",
        ] {
            assert!(parse_key(key).is_ok(), "{key}");
        }
        for key in ["", "hello", "raw:123", "Other", "F13", "\n", "ж", "🚀"] {
            assert_eq!(parse_key(key), Err(PlatformError::InvalidKey));
        }
    }

    #[test]
    fn synthetic_frame_is_jpeg_and_scaled_with_aspect_ratio() {
        let data = encode_frame(RgbaImage::new(2400, 1200), CaptureQuality::Balanced).unwrap();
        assert_eq!(&data[..2], &[0xff, 0xd8]);
        let decoded = image::load_from_memory(&data).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1920, 960));
        assert!(data.len() < MAX_FRAME_BYTES);
    }

    #[test]
    fn small_frame_is_not_upscaled_and_empty_frame_fails() {
        let data = encode_frame(RgbaImage::new(32, 16), CaptureQuality::Balanced).unwrap();
        let decoded = image::load_from_memory(&data).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (32, 16));
        assert_eq!(
            encode_frame(RgbaImage::new(0, 0), CaptureQuality::Balanced),
            Err(PlatformError::EncodeFailed)
        );
    }

    #[test]
    fn construction_and_release_are_lazy_without_os_access() {
        let mut platform = crate::Platform::new();
        assert!(platform.inner.monitor.is_none());
        assert!(platform.inner.input.is_none());
        platform.release_inputs().unwrap();
        assert!(platform.inner.monitor.is_none());
        assert!(platform.inner.input.is_none());
    }

    #[test]
    fn input_before_capture_never_initializes_native_connection() {
        let mut platform = crate::Platform::new();
        let result = platform.apply_input(InputEvent::Move { x: 0.5, y: 0.5 });
        assert!(matches!(
            result,
            Err(PlatformError::CaptureRequired | PlatformError::WaylandControlUnsupported)
        ));
        assert!(platform.inner.input.is_none());
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::*;
    #[test]
    fn scroll_is_bounded_without_opening_native_input() {
        assert!(validate_scroll(-100, 100).is_ok());
        assert!(validate_scroll(0, 0).is_ok());
        for (x, y) in [(101, 0), (0, -101), (i32::MIN, 0), (0, i32::MAX)] {
            assert_eq!(validate_scroll(x, y), Err(PlatformError::InvalidScroll));
        }
    }
}

#[cfg(test)]
mod tools_tests {
    use super::*;

    #[test]
    fn literal_unicode_text_is_bounded_without_os_access() {
        for text in [
            "Привет, мир!",
            "日本語 🚀",
            "line\nnext\tcolumn",
            &"x".repeat(4096),
        ] {
            assert_eq!(validate_text(text), Ok(()));
        }
        for text in [
            "",
            "\0",
            "line\rnext",
            "\u{1b}[31m",
            "\u{7f}",
            &"я".repeat(2049),
        ] {
            assert_eq!(validate_text(text), Err(PlatformError::InvalidText));
        }
    }

    #[test]
    fn extended_keyboard_names_match_wire_snake_case() {
        for key in [
            "f1",
            "f2",
            "f3",
            "f4",
            "f5",
            "f6",
            "f7",
            "f8",
            "f9",
            "f10",
            "f11",
            "f12",
            "insert",
            "caps_lock",
        ] {
            assert!(parse_key(key).is_ok(), "{key}");
        }
        assert_eq!(parse_key("f13"), Err(PlatformError::InvalidKey));
    }

    #[test]
    fn monitor_ids_reject_untrusted_invalid_forms_before_enumeration() {
        assert_eq!(parse_display_id("0"), Ok(0));
        assert_eq!(parse_display_id("4294967295"), Ok(u32::MAX));
        for id in [
            "",
            "-1",
            "1.0",
            "primary",
            " 1",
            "4294967296",
            "00000000001",
        ] {
            assert_eq!(parse_display_id(id), Err(PlatformError::InvalidDisplay));
        }
        let mut platform = crate::Platform::new();
        assert_eq!(
            platform.select_display("invalid".into()),
            Err(PlatformError::InvalidDisplay)
        );
        assert!(platform.selected_display().is_none());
        assert!(platform.inner.monitor.is_none());
        assert!(platform.inner.input.is_none());
    }

    #[test]
    fn quality_configuration_is_lazy_and_keeps_wire_limits() {
        let mut platform = crate::Platform::new();
        assert_eq!(platform.quality(), CaptureQuality::Balanced);
        for quality in [
            CaptureQuality::Low,
            CaptureQuality::Balanced,
            CaptureQuality::High,
        ] {
            platform.set_quality(quality);
            assert_eq!(platform.quality(), quality);
            assert!(quality.max_edge() <= 4096);
            assert!(quality.max_edge().pow(2) <= 8_388_608);
            assert!(quality.max_fps() <= 12);
            assert!(platform.inner.monitor.is_none());
            assert!(platform.inner.input.is_none());
        }
    }

    #[test]
    fn quality_changes_encoded_resolution_without_upscaling() {
        let source = RgbaImage::new(1400, 700);
        let low =
            image::load_from_memory(&encode_frame(source.clone(), CaptureQuality::Low).unwrap())
                .unwrap();
        let high =
            image::load_from_memory(&encode_frame(source, CaptureQuality::High).unwrap()).unwrap();
        assert_eq!((low.width(), low.height()), (1280, 640));
        assert_eq!((high.width(), high.height()), (1400, 700));
    }
}
