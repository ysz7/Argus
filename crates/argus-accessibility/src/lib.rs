//! Native accessibility adapters for Argus.
//!
//! Reads the platform accessibility tree (macOS Accessibility first) and turns
//! raw native nodes into [`ElementCandidate`]s:
//!
//! ```text
//! platform API → AxNode tree (raw) → adapter → ElementCandidate
//! ```
//!
//! Boundaries:
//! - platform-specific roles and attributes never leak into observations;
//!   raw [`AxNode`]s are exposed only for debugging;
//! - read-only: this crate never performs accessibility actions (press,
//!   set value, focus, ...).
//!
//! [`ElementCandidate`]: argus_protocol::ElementCandidate

mod adapter;
mod error;
#[cfg(target_os = "macos")]
mod macos;
mod node;
mod roles;

pub use adapter::candidates;
pub use error::{Error, Result};
#[cfg(target_os = "macos")]
pub use macos::MacAccessibilityBackend;
pub use node::{AxFrame, AxNode, AxRange, AxRun, AxSnapshot, AxValue};

/// Which application to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppTarget {
    /// The application that currently receives keyboard input.
    Frontmost,
    /// The application with this process identifier.
    Pid(u32),
    /// The running application whose name or bundle identifier matches
    /// (case-insensitive).
    Name(String),
}

/// A source of native accessibility trees.
pub trait AccessibilityBackend {
    /// Whether the process may read other applications' accessibility trees.
    fn has_permission(&self) -> bool;

    /// Reads the focused window of `target` (falling back to its main or
    /// first window).
    fn snapshot(&self, target: &AppTarget) -> Result<AxSnapshot>;
}

/// The accessibility backend for the current platform.
pub fn default_backend() -> Result<Box<dyn AccessibilityBackend>> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(MacAccessibilityBackend::new()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(Error::Unsupported)
    }
}
