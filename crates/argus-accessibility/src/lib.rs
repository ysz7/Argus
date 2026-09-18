//! Native accessibility adapters for Argus.
//!
//! Reads the platform accessibility tree (macOS Accessibility first) and turns
//! raw native nodes into source candidates.
//!
//! Boundaries:
//! - platform-specific roles and attributes never leak into the public
//!   protocol; they are mapped by the normalization pipeline;
//! - read-only: this crate never performs accessibility actions (press,
//!   set value, focus, ...).
