//! Argus Observation Protocol.
//!
//! This crate defines the public, platform-independent language Argus uses to
//! describe a user interface: observations, elements, roles, bounds, states,
//! confidence, sources and relations.
//!
//! Boundaries:
//! - depends on no other Argus crate;
//! - contains no platform code, no I/O and no perception logic;
//! - every type here is part of the serialization contract and must change
//!   deliberately (see `spec/ARGUS_PROTOCOL.md`).

#![forbid(unsafe_code)]
