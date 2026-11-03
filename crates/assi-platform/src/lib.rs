//! Lazy native adapters. The caller must obtain local owner consent for the active
//! session before invoking capture or input. This crate does not grant consent.

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
mod desktop;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_FRAME_EDGE: u32 = 2560;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputEvent {
    Move { x: f64, y: f64 },
    Wheel { x: i32, y: i32 },
    Button { button: MouseButton, down: bool },
    Key { key: String, down: bool },
    Text { text: String },
}

/// Stable native monitor id within the current OS display configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DisplayInfo {
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

/// Owned approved screen pixels. Encode after releasing the Platform mutex so
/// compression does not hold up keyboard/mouse injection.
pub struct CapturedRgba {
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureQuality {
    Low,
    #[default]
    Balanced,
    High,
}
impl CaptureQuality {
    pub fn max_fps(self) -> u8 {
        match self {
            Self::Low => 6,
            Self::Balanced => 8,
            Self::High => 12,
        }
    }
    pub fn max_edge(self) -> u32 {
        match self {
            Self::Low => 1280,
            Self::Balanced => 1920,
            Self::High => MAX_FRAME_EDGE,
        }
    }
    pub fn jpeg_quality(self) -> u8 {
        match self {
            Self::Low => 55,
            Self::Balanced => 70,
            Self::High => 85,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    /// Compiled adapter availability; this does not imply OS permission.
    pub capture: bool,
    pub control: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PlatformError {
    #[error("Эта платформа пока не поддерживает передачу экрана и удалённый ввод")]
    Unsupported,
    #[error("Управление в Wayland пока недоступно; используйте сеанс Linux X11")]
    WaylandControlUnsupported,
    #[error("Экран не найден; подключите монитор и начните сеанс заново")]
    NoMonitor,
    #[error("Не удалось прочитать параметры экрана; начните сеанс заново")]
    MonitorUnavailable,
    #[error("Конфигурация монитора изменилась; начните сеанс заново")]
    MonitorChanged,
    #[error("Нет доступа к экрану. Проверьте разрешение на запись экрана в настройках системы и начните сеанс заново")]
    CaptureUnavailable,
    #[error("Нет доступа к управлению. Проверьте разрешение «Универсальный доступ» на macOS или доступ к рабочему столу в Windows/Linux")]
    InputUnavailable,
    #[error("Сначала необходимо получить кадр выбранного экрана")]
    CaptureRequired,
    #[error("Координаты должны быть конечными числами от 0 до 1")]
    InvalidCoordinates,
    #[error("Прокрутка превышает допустимое число шагов")]
    InvalidScroll,
    #[error("Эта клавиша не поддерживается")]
    InvalidKey,
    #[error("Текст должен содержать от 1 до 4096 байт UTF-8 без управляющих символов, кроме переноса строки и табуляции")]
    InvalidText,
    #[error("Выбранный экран недоступен; обновите список экранов")]
    InvalidDisplay,
    #[error("Не удалось подготовить кадр экрана")]
    EncodeFailed,
    #[error("Кадр экрана превышает допустимый размер")]
    FrameTooLarge,
}

pub type Result<T> = std::result::Result<T, PlatformError>;

#[derive(Default)]
pub struct Platform {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    inner: desktop::Desktop,
}

impl Platform {
    /// Pure construction: does not enumerate monitors, open input devices, capture,
    /// check permission or display an OS permission prompt.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn capabilities() -> Capabilities {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            let wayland = desktop::is_wayland();
            Capabilities {
                capture: true,
                control: !wayland,
                reason: wayland.then(|| PlatformError::WaylandControlUnsupported.to_string()),
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Capabilities {
                capture: false,
                control: false,
                reason: Some(PlatformError::Unsupported.to_string()),
            }
        }
    }

    /// Enumerates only when requested; the caller must restrict this to an approved host session.
    pub fn displays() -> Result<Vec<DisplayInfo>> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            desktop::displays()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Err(PlatformError::Unsupported)
        }
    }

    /// Changes the monitor only after validating it exists, and releases all held input.
    pub fn select_display(&mut self, id: String) -> Result<()> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            self.inner.select_display(id)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = id;
            Err(PlatformError::Unsupported)
        }
    }

    pub fn selected_display(&self) -> Option<String> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            self.inner.selected_display()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            None
        }
    }

    /// Pure configuration: does not initialize capture or input.
    pub fn set_quality(&mut self, quality: CaptureQuality) {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            self.inner.quality = quality;
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = quality;
        }
    }

    pub fn quality(&self) -> CaptureQuality {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            self.inner.quality
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            CaptureQuality::default()
        }
    }

    /// Call only after explicit local owner acceptance for this active session.
    pub fn capture_jpeg(&mut self) -> Result<Vec<u8>> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            self.inner.capture_jpeg()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Err(PlatformError::Unsupported)
        }
    }

    /// Establishes the same monitor geometry as capture_jpeg, without compression.
    /// Call only after owner consent. No input device is initialized by this method.
    pub fn capture_rgba(&mut self) -> Result<CapturedRgba> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            self.inner.capture_rgba()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Err(PlatformError::Unsupported)
        }
    }

    /// Requires a captured monitor and independently granted session control.
    pub fn apply_input(&mut self, event: InputEvent) -> Result<()> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            self.inner.apply_input(event)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = event;
            Err(PlatformError::Unsupported)
        }
    }

    /// Best effort release of every held key/button, even if an individual release
    /// fails. Invalidates the captured monitor so a new session starts fresh.
    pub fn release_inputs(&mut self) -> Result<()> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            self.inner.release_inputs()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Ok(())
        }
    }
}
