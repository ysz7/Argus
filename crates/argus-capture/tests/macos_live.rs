//! Live tests against the real macOS window server.
//!
//! They need a logged-in GUI session and the Screen Recording permission for
//! the process running the tests, so they are ignored by default:
//!
//! ```bash
//! cargo test -p argus-capture -- --ignored
//! ```

#![cfg(target_os = "macos")]

use argus_capture::{CaptureBackend, CaptureTarget, MacCaptureBackend};

const REASON: &str = "requires a macOS GUI session with Screen Recording permission";

#[test]
#[ignore = "requires a macOS GUI session with Screen Recording permission"]
fn lists_exactly_one_main_display_at_origin() {
    let displays = MacCaptureBackend::new().displays().expect(REASON);
    let main: Vec<_> = displays.iter().filter(|display| display.is_main).collect();
    assert_eq!(main.len(), 1, "{displays:?}");
    assert_eq!((main[0].bounds.x(), main[0].bounds.y()), (0.0, 0.0));
    assert!(displays.iter().all(|display| display.scale_factor >= 1.0));
}

#[test]
#[ignore = "requires a macOS GUI session with Screen Recording permission"]
fn main_display_frame_matches_display_geometry() {
    let backend = MacCaptureBackend::new();
    let display = backend.displays().expect(REASON).into_iter().find(|d| d.is_main).unwrap();
    let frame = backend.capture(CaptureTarget::MainDisplay).expect(REASON);

    assert_eq!(frame.bounds(), display.bounds);
    assert_eq!(frame.scale_factor(), display.scale_factor);
    assert_eq!(frame.width() as f32, display.bounds.width() * display.scale_factor);
    assert_eq!(frame.height() as f32, display.bounds.height() * display.scale_factor);
}

#[test]
#[ignore = "requires a macOS GUI session with Screen Recording permission"]
fn frontmost_window_frame_is_grounded_at_window_position() {
    let backend = MacCaptureBackend::new();
    let window = backend.frontmost_window().expect(REASON);
    let frame = backend.capture(CaptureTarget::Window(window.id)).expect(REASON);

    assert_eq!(frame.bounds(), window.bounds);
    let (x, y) = frame.pixel_to_point(0.0, 0.0);
    assert_eq!((x, y), (window.bounds.x(), window.bounds.y()));
}

#[test]
#[ignore = "requires a macOS GUI session with Screen Recording permission"]
fn frame_ids_are_unique() {
    let backend = MacCaptureBackend::new();
    let first = backend.capture(CaptureTarget::MainDisplay).expect(REASON);
    let second = backend.capture(CaptureTarget::MainDisplay).expect(REASON);
    assert_ne!(first.id(), second.id());
    assert!(second.timestamp() >= first.timestamp());
}
