use thiserror::Error;

/// Result alias for protocol value construction.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A protocol value violates its invariants.
#[derive(Debug, Clone, PartialEq, Error)]
#[non_exhaustive]
pub enum Error {
    /// A confidence score is outside `0.0..=1.0` or not finite.
    #[error("confidence score must be a finite number in 0.0..=1.0, got {0}")]
    InvalidScore(f32),

    /// Bounds contain a non-finite number or a negative size.
    #[error(
        "bounds must be finite with non-negative size, got x={x} y={y} width={width} height={height}"
    )]
    InvalidBounds {
        /// Left edge.
        x: f32,
        /// Top edge.
        y: f32,
        /// Width.
        width: f32,
        /// Height.
        height: f32,
    },

    /// A frame or pixel buffer is internally inconsistent.
    #[error("invalid frame: {0}")]
    InvalidFrame(String),

    /// An identifier is empty.
    #[error("{kind} must not be empty")]
    EmptyId {
        /// Which identifier type was empty.
        kind: &'static str,
    },
}
