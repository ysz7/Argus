//! Frame acquisition for Argus.
//!
//! Responsible for obtaining pixels of screens and windows together with the
//! metadata needed to ground them (dimensions, scale factor, global
//! coordinates, timestamps).
//!
//! Boundaries:
//! - captured frames stay in memory and on the local machine;
//! - nothing here persists screenshots;
//! - no interpretation of pixels happens here (that is `argus-perception`).
//!
//! The macOS backend requires macOS 14 or later (ScreenCaptureKit
//! screenshots) and the Screen Recording permission.

mod error;
#[cfg(target_os = "macos")]
mod macos;

use argus_protocol::{Application, Bounds, Frame};
use serde::Serialize;

pub use error::{Error, Result};
#[cfg(target_os = "macos")]
pub use macos::MacCaptureBackend;

/// Platform identifier of a display.
pub type DisplayId = u32;

/// Platform identifier of a window.
pub type WindowId = u32;

/// A connected display.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DisplayInfo {
    /// Platform display identifier.
    pub id: DisplayId,
    /// Display area in global screen points.
    pub bounds: Bounds,
    /// Physical pixels per point (e.g. `2.0` on Retina displays).
    pub scale_factor: f32,
    /// Whether this is the primary display (origin of the global space).
    pub is_main: bool,
}

/// An on-screen window.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WindowInfo {
    /// Platform window identifier.
    pub id: WindowId,
    /// Window title. Requires the Screen Recording permission on macOS.
    pub title: Option<String>,
    /// Application owning the window.
    pub application: Application,
    /// Window frame in global screen points.
    pub bounds: Bounds,
    /// Window layer; `0` is the layer of normal application windows.
    pub layer: i64,
}

/// What to capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureTarget {
    /// The frontmost window of the frontmost application.
    FrontmostWindow,
    /// A specific window.
    Window(WindowId),
    /// The primary display.
    MainDisplay,
    /// A specific display.
    Display(DisplayId),
}

/// A source of frames and of the screen layout needed to ground them.
pub trait CaptureBackend {
    /// Whether the process may capture screen contents.
    fn has_permission(&self) -> bool;

    /// Connected displays.
    fn displays(&self) -> Result<Vec<DisplayInfo>>;

    /// On-screen windows, front to back.
    fn windows(&self) -> Result<Vec<WindowInfo>>;

    /// The application that currently receives keyboard input.
    fn frontmost_application(&self) -> Result<Option<Application>>;

    /// The frontmost normal window of the frontmost application.
    ///
    /// Applications often own untitled auxiliary windows (toolbars, overlays)
    /// in front of their main window, so the first titled window is preferred.
    /// Titles are only visible with the Screen Recording permission; without
    /// it the first window is used.
    fn frontmost_window(&self) -> Result<WindowInfo> {
        let app_pid = self.frontmost_application()?.and_then(|app| app.pid);
        let candidates: Vec<WindowInfo> = self
            .windows()?
            .into_iter()
            .filter(|window| window.layer == 0)
            .filter(|window| app_pid.is_none() || window.application.pid == app_pid)
            .collect();
        let titled = candidates.iter().position(|window| window.title.is_some());
        candidates.into_iter().nth(titled.unwrap_or(0)).ok_or(Error::NoFrontmostWindow)
    }

    /// Captures one frame of `target`.
    fn capture(&self, target: CaptureTarget) -> Result<Frame>;
}

/// The capture backend for the current platform.
pub fn default_backend() -> Result<Box<dyn CaptureBackend>> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(MacCaptureBackend::new()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(Error::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeBackend {
        frontmost: Option<Application>,
        windows: Vec<WindowInfo>,
    }

    impl CaptureBackend for FakeBackend {
        fn has_permission(&self) -> bool {
            true
        }

        fn displays(&self) -> Result<Vec<DisplayInfo>> {
            Ok(Vec::new())
        }

        fn windows(&self) -> Result<Vec<WindowInfo>> {
            Ok(self.windows.clone())
        }

        fn frontmost_application(&self) -> Result<Option<Application>> {
            Ok(self.frontmost.clone())
        }

        fn capture(&self, _target: CaptureTarget) -> Result<Frame> {
            Err(Error::Unsupported)
        }
    }

    fn app(pid: u32) -> Application {
        Application { name: None, bundle_id: None, pid: Some(pid) }
    }

    fn window(id: WindowId, pid: u32, layer: i64, title: Option<&str>) -> WindowInfo {
        WindowInfo {
            id,
            title: title.map(str::to_owned),
            application: app(pid),
            bounds: Bounds::new(0.0, 0.0, 100.0, 100.0).unwrap(),
            layer,
        }
    }

    #[test]
    fn picks_first_titled_window_of_frontmost_app() {
        let backend = FakeBackend {
            frontmost: Some(app(2)),
            windows: vec![
                window(1, 1, 0, Some("Other app")),
                window(2, 2, 25, Some("Status item")),
                window(3, 2, 0, None),
                window(4, 2, 0, Some("Main")),
                window(5, 2, 0, Some("Behind")),
            ],
        };
        assert_eq!(backend.frontmost_window().unwrap().id, 4);
    }

    #[test]
    fn falls_back_to_first_window_without_titles() {
        let backend = FakeBackend {
            frontmost: Some(app(2)),
            windows: vec![window(1, 1, 0, None), window(3, 2, 0, None), window(4, 2, 0, None)],
        };
        assert_eq!(backend.frontmost_window().unwrap().id, 3);
    }

    #[test]
    fn reports_missing_window() {
        let backend =
            FakeBackend { frontmost: Some(app(2)), windows: vec![window(1, 1, 0, Some("Other"))] };
        assert!(matches!(backend.frontmost_window(), Err(Error::NoFrontmostWindow)));
    }
}
