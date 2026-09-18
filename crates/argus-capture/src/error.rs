use crate::{DisplayId, WindowId};

/// Result alias for capture operations.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A capture operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The Screen Recording permission is not granted.
    #[error(
        "screen recording permission is not granted; allow the terminal (or app) running \
         argus in System Settings → Privacy & Security → Screen & System Audio Recording"
    )]
    PermissionDenied,

    /// Screen capture is not implemented for this platform.
    #[error("screen capture is not supported on this platform")]
    Unsupported,

    /// No display with this identifier is connected.
    #[error("display {0} not found")]
    DisplayNotFound(DisplayId),

    /// No on-screen window with this identifier exists.
    #[error("window {0} not found or not on screen")]
    WindowNotFound(WindowId),

    /// The frontmost application has no on-screen window.
    #[error("the frontmost application has no on-screen window")]
    NoFrontmostWindow,

    /// The operating system did not answer in time.
    #[error("timed out waiting for {0}")]
    Timeout(&'static str),

    /// An operating-system API failed.
    #[error("{0}")]
    Platform(String),

    /// The captured image is inconsistent with its metadata.
    #[error(transparent)]
    InvalidFrame(#[from] argus_protocol::Error),
}

impl Error {
    /// Stable, machine-readable identifier of the error class.
    pub fn code(&self) -> &'static str {
        match self {
            Error::PermissionDenied => "permission_denied",
            Error::Unsupported => "unsupported",
            Error::DisplayNotFound(_) | Error::WindowNotFound(_) | Error::NoFrontmostWindow => {
                "not_found"
            }
            Error::Timeout(_) => "timeout",
            Error::Platform(_) => "platform_error",
            Error::InvalidFrame(_) => "invalid_frame",
        }
    }
}
