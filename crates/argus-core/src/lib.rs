//! Argus orchestration pipeline.
//!
//! Wires capture, accessibility, perception, fusion and tracking into a single
//! observation pipeline:
//!
//! ```text
//! sources ─SourceCandidate→ normalize ─Normalized→ (fusion → scene graph
//!     → tracking) → assemble → Observation
//! ```
//!
//! [`assemble`] accepts only [`Normalized`] candidates, which only
//! [`normalize`] can produce: no source reaches an observation without
//! normalization.
//!
//! Boundaries: Argus describes the interface. It never plans, decides what to
//! click, drives the mouse or keyboard, or executes actions.

#![forbid(unsafe_code)]

mod assemble;
pub mod error;
mod normalize;
mod observe;

/// Native accessibility trees (re-exported so front ends depend only on the core).
pub use argus_accessibility as accessibility;
/// Frame acquisition (re-exported so front ends depend only on the core).
pub use argus_capture as capture;
pub use assemble::assemble;
pub use error::{Error, Permission, Result};
pub use normalize::{Normalized, normalize};
pub use observe::Observer;
