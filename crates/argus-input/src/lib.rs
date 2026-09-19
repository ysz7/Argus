//! Mouse and keyboard execution for the Argus MCP adapter.
//!
//! The perception pipeline (`argus-core`) never acts. This crate is the
//! separate execution layer an agent reaches through the MCP adapter: it
//! posts synthetic mouse and keyboard events, nothing else. Which actions
//! are allowed (target application, forbidden key combinations) is decided
//! by the caller.
//!
//! Coordinates are global logical points with the origin at the top-left
//! of the primary display, the space of Argus bounds.
//!
//! The macOS backend requires the Accessibility permission for the process
//! that posts events.

mod keys;
#[cfg(target_os = "macos")]
mod macos;

pub use keys::{Combo, Modifiers, parse_combo};

/// Result alias of the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Why an action could not be performed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The platform has no input backend.
    #[error("input is not supported on this platform")]
    Unsupported,
    /// The process may not post events (macOS Accessibility permission).
    #[error("the Accessibility permission is required to act")]
    PermissionDenied,
    /// A key combination could not be understood.
    #[error("{0}")]
    BadKeys(String),
    /// The user moved the mouse while an action was running: it stopped.
    #[error("stopped: the user moved the mouse")]
    Interrupted,
    /// The platform refused an event.
    #[error("{0}")]
    Platform(String),
}

/// A point in global screen points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// Horizontal position.
    pub x: f64,
    /// Vertical position.
    pub y: f64,
}

impl Point {
    /// A point at (`x`, `y`).
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Whether `other` is farther than `tolerance` points away on any axis.
    pub fn moved_from(&self, other: Point, tolerance: f64) -> bool {
        (self.x - other.x).abs() > tolerance || (self.y - other.y).abs() > tolerance
    }
}

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Button {
    /// The primary button.
    #[default]
    Left,
    /// The secondary button (context menus).
    Right,
    /// The middle button.
    Middle,
}

/// Posts mouse and keyboard events.
#[derive(Debug)]
pub struct Input {
    #[cfg(target_os = "macos")]
    backend: macos::MacInput,
}

impl Input {
    /// The input backend of this platform.
    pub fn new() -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            Ok(Self { backend: macos::MacInput::new()? })
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(Error::Unsupported)
        }
    }
}

#[cfg(target_os = "macos")]
impl Input {
    /// Whether the process may post events.
    pub fn has_permission(&self) -> bool {
        macos::trusted()
    }

    /// The current mouse position.
    pub fn mouse(&self) -> Point {
        self.backend.mouse()
    }

    /// Clicks `count` times (2 = double click) at `point`, holding
    /// `modifiers`.
    pub fn click(
        &self,
        point: Point,
        button: Button,
        count: u32,
        modifiers: Modifiers,
    ) -> Result<()> {
        self.backend.click(point, button, count.clamp(1, 3), modifiers)
    }

    /// Moves the mouse to `point`.
    pub fn move_to(&self, point: Point) -> Result<()> {
        self.backend.move_to(point)
    }

    /// Drags with the left button from `from` to `to`.
    pub fn drag(&self, from: Point, to: Point) -> Result<()> {
        self.backend.drag(from, to)
    }

    /// Scrolls at `point` by `dx`, `dy` lines (positive `dy` scrolls up).
    pub fn scroll(&self, point: Point, dx: i32, dy: i32) -> Result<()> {
        self.backend.scroll(point, dx, dy)
    }

    /// Types `text` into the focused element. Stops with
    /// [`Error::Interrupted`] if the user moves the mouse meanwhile.
    pub fn type_text(&self, text: &str) -> Result<()> {
        self.backend.type_text(text)
    }

    /// Presses a key combination `repeat` times.
    pub fn press(&self, combo: &Combo, repeat: u32) -> Result<()> {
        self.backend.press(combo, repeat.max(1))
    }

    /// Brings the application with `pid` to the front. `false` if it could
    /// not be activated.
    pub fn activate(&self, pid: u32) -> bool {
        self.backend.activate(pid)
    }
}
