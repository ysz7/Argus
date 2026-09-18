//! Local Argus service.
//!
//! Exposes observations to external programs over a local API.
//!
//! Boundaries (security defaults):
//! - listens on localhost only by default;
//! - no cloud connectivity, no persistent screenshots, no screen-content
//!   telemetry;
//! - serves observations; never accepts actions to execute.

#![forbid(unsafe_code)]
