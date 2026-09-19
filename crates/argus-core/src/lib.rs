//! Argus orchestration pipeline.
//!
//! Wires capture, accessibility, perception, fusion and tracking into a single
//! observation pipeline:
//!
//! ```text
//! sources ─SourceCandidate→ normalize ─Normalized→ fuse (+ scene graph)
//!     ─Fused→ assemble → Observation → track (stable IDs)
//! ```
//!
//! [`assemble`] accepts only [`Fused`] elements, which only [`fuse`] can
//! produce from [`Normalized`] candidates, which only [`normalize`] can
//! produce: no source reaches an observation without normalization and
//! fusion.
//!
//! Boundaries: Argus describes the interface. It never plans, decides what to
//! click, drives the mouse or keyboard, or executes actions.

#![forbid(unsafe_code)]

mod assemble;
pub mod error;
mod fuse;
mod normalize;
mod observe;

/// Native accessibility trees (re-exported so front ends depend only on the core).
pub use argus_accessibility as accessibility;
/// Frame acquisition (re-exported so front ends depend only on the core).
pub use argus_capture as capture;
pub use assemble::assemble;
pub use error::{Error, Permission, Result};
pub use fuse::{Fused, fuse};
pub use normalize::{Normalized, normalize};
pub use observe::{Inspection, Observer};

/// Element identity across observations (re-exported so front ends depend
/// only on the core).
pub mod tracking {
    pub use argus_tracking::{MEMORY, Tracker, TrackingReport, UNCERTAIN};
}

/// Fusion evidence types (re-exported so front ends depend only on the core).
pub mod fusion {
    pub use argus_fusion::{Claim, Conflict, Contribution, ElementEvidence, Link, Property};
}
