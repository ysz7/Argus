//! Argus orchestration pipeline.
//!
//! Wires capture, accessibility, perception, fusion and tracking into a single
//! observation pipeline:
//!
//! ```text
//! capture / accessibility → candidates → normalization → fusion
//!     → scene graph → tracking → Observation
//! ```
//!
//! Boundaries: Argus describes the interface. It never plans, decides what to
//! click, drives the mouse or keyboard, or executes actions.

#![forbid(unsafe_code)]

pub mod error;

pub use error::{Error, Permission, Result};
