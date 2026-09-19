/// Result alias for perception operations.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A perception backend failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// No backend is available on this platform.
    #[error("OCR is not supported on this platform")]
    Unsupported,

    /// The platform framework failed.
    #[error("{0}")]
    Platform(String),

    /// A frame could not be prepared for perception (e.g. an invalid crop).
    #[error("invalid frame: {0}")]
    InvalidFrame(#[from] argus_protocol::Error),
}

impl Error {
    /// Stable, machine-readable identifier of the error class.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Unsupported => "unsupported",
            Error::Platform(_) => "platform_error",
            Error::InvalidFrame(_) => "invalid_frame",
        }
    }
}
