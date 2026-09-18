/// Result alias for accessibility operations.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Reading the accessibility tree failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The Accessibility permission is not granted.
    #[error(
        "accessibility permission is not granted; allow the terminal (or app) running argus \
         in System Settings → Privacy & Security → Accessibility"
    )]
    PermissionDenied,

    /// Accessibility is not implemented for this platform.
    #[error("accessibility is not supported on this platform")]
    Unsupported,

    /// No running application matches the target.
    #[error("no running application matches {0}")]
    ApplicationNotFound(String),

    /// The application exposes no window.
    #[error("application has no accessible window")]
    NoWindow,

    /// An operating-system API failed.
    #[error("{0}")]
    Platform(String),
}

impl Error {
    /// Stable, machine-readable identifier of the error class.
    pub fn code(&self) -> &'static str {
        match self {
            Error::PermissionDenied => "permission_denied",
            Error::Unsupported => "unsupported",
            Error::ApplicationNotFound(_) | Error::NoWindow => "not_found",
            Error::Platform(_) => "platform_error",
        }
    }
}
