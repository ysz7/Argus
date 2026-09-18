//! Error strategy for Argus.
//!
//! - Every library crate owns a typed error enum built with `thiserror` and a
//!   `Result<T, E = Error>` alias. Errors describe *what* failed in terms the
//!   caller can act on; they are `#[non_exhaustive]` so variants can be added.
//! - `argus-core` aggregates the errors of the crates it orchestrates into
//!   [`Error`] via `#[from]` conversions, so the pipeline has a single error
//!   type.
//! - Binaries (`argus-cli`) use `anyhow` at the top level to attach context and
//!   report errors to the user.
//! - Every [`Error`] exposes a stable machine-readable [`Error::code`], used
//!   for exit codes, API responses and diagnostics.
//! - Error messages must never contain screen content (recognized text,
//!   element names, pixels); they may contain identifiers and metadata.

use std::fmt;

/// Result alias used across the Argus pipeline.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Top-level error of the Argus pipeline.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A capability is not available on the current platform or build.
    #[error("{feature} is not supported on this platform")]
    Unsupported {
        /// Human-readable name of the missing capability.
        feature: &'static str,
    },

    /// The operating system denied a permission Argus needs.
    #[error("{0} permission is not granted")]
    PermissionDenied(Permission),

    /// Frame acquisition failed.
    #[error(transparent)]
    Capture(#[from] argus_capture::Error),

    /// Reading the accessibility tree failed.
    #[error(transparent)]
    Accessibility(#[from] argus_accessibility::Error),
}

impl Error {
    /// Stable, machine-readable identifier of the error class.
    ///
    /// Codes are part of the public contract and must not be renamed.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Unsupported { .. } => "unsupported",
            Error::PermissionDenied(_) => "permission_denied",
            Error::Capture(error) => error.code(),
            Error::Accessibility(error) => error.code(),
        }
    }
}

/// Operating-system permissions Argus may require.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Permission {
    /// Permission to capture screen contents.
    ScreenRecording,
    /// Permission to read the accessibility tree of other applications.
    Accessibility,
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Permission::ScreenRecording => "screen recording",
            Permission::Accessibility => "accessibility",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(Error::Unsupported { feature: "screen capture" }.code(), "unsupported");
        assert_eq!(Error::PermissionDenied(Permission::Accessibility).code(), "permission_denied");
    }

    #[test]
    fn messages_are_human_readable() {
        assert_eq!(
            Error::PermissionDenied(Permission::ScreenRecording).to_string(),
            "screen recording permission is not granted"
        );
        assert_eq!(
            Error::Unsupported { feature: "screen capture" }.to_string(),
            "screen capture is not supported on this platform"
        );
    }
}
