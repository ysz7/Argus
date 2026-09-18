//! Live tests against real macOS applications.
//!
//! They need a GUI session and the Accessibility permission for the process
//! running the tests, and they launch Calculator in the background, so they
//! are ignored by default:
//!
//! ```bash
//! cargo test -p argus-accessibility -- --ignored
//! ```

#![cfg(target_os = "macos")]

use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

use argus_accessibility::{
    AccessibilityBackend, AppTarget, AxSnapshot, Error, MacAccessibilityBackend, candidates,
};
use argus_protocol::Role;

const REASON: &str = "requires a macOS GUI session with Accessibility permission";

/// Launches Calculator in the background (`-g` keeps the current application
/// in front) and waits until its window can be read.
fn calculator_snapshot(backend: &MacAccessibilityBackend) -> AxSnapshot {
    let status = Command::new("open").args(["-g", "-a", "Calculator"]).status().unwrap();
    assert!(status.success());

    let target = AppTarget::Name("com.apple.calculator".to_owned());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match backend.snapshot(&target) {
            Ok(snapshot) => return snapshot,
            // During a cold launch the app, its window and its accessibility
            // support appear one after another.
            Err(Error::ApplicationNotFound(_) | Error::NoWindow | Error::Platform(_))
                if Instant::now() < deadline =>
            {
                sleep(Duration::from_millis(200));
            }
            Err(error) => panic!("{REASON}: {error}"),
        }
    }
}

#[test]
#[ignore = "requires a macOS GUI session with Accessibility permission"]
fn reads_calculator_keypad() {
    let snapshot = calculator_snapshot(&MacAccessibilityBackend::new());
    assert_eq!(snapshot.application.bundle_id.as_deref(), Some("com.apple.calculator"));
    assert_eq!(snapshot.window.role, "AXWindow");
    assert!(!snapshot.truncated);

    let candidates = candidates(&snapshot);
    let digits = candidates
        .iter()
        .filter(|c| c.role == Role::Button)
        .filter(|c| {
            c.name
                .as_deref()
                .is_some_and(|name| name.len() == 1 && name.as_bytes()[0].is_ascii_digit())
        })
        .count();
    assert_eq!(digits, 10, "{candidates:#?}");
}

#[test]
#[ignore = "requires a macOS GUI session with Accessibility permission"]
fn reports_unknown_applications() {
    let result = MacAccessibilityBackend::new()
        .snapshot(&AppTarget::Name("no-such-application-argus".to_owned()));
    assert!(matches!(result, Err(Error::ApplicationNotFound(_))), "{result:?}");
}
