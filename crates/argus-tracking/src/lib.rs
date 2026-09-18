//! Element identity and change tracking across observations.
//!
//! Boundaries:
//! - identity is a scored hypothesis, not an absolute truth; ambiguous matches
//!   must be reported as such;
//! - produces stable element IDs and observation deltas, nothing else.

#![forbid(unsafe_code)]
