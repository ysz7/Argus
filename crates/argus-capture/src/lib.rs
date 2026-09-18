//! Frame acquisition for Argus.
//!
//! Responsible for obtaining pixels of screens and windows together with the
//! metadata needed to ground them (dimensions, scale factor, global
//! coordinates, timestamps).
//!
//! Boundaries:
//! - captured frames stay in memory and on the local machine;
//! - nothing here persists screenshots unless explicitly asked for debugging;
//! - no interpretation of pixels happens here (that is `argus-perception`).
