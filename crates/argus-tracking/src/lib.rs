//! Element identity across observations.
//!
//! A [`Tracker`] gives the elements of successive observations of one
//! application IDs that persist: an element that is still there keeps its ID,
//! a new element gets an ID that was never used before.
//!
//! # Policy
//!
//! Identity is a scored hypothesis. Every pair of a remembered element and a
//! current element that could be the same object is scored in `0..=1` from:
//!
//! - **geometry**: position and size relative to the window (moving the
//!   window moves nothing), tolerant of a point or two of jitter; aligned
//!   edges count (right-aligned text that grows keeps its right edge), and the
//!   width of text, which follows its content, is not compared. The
//!   element may also have moved with the content around it (the median
//!   displacement of nearby *anchors*, leaf elements with a name unique in
//!   both observations, e.g. a scrolled list) or with its parent. Far from
//!   any of these places only a distinctive element (unique name or native
//!   identifier) is recognized;
//! - **name**: equal, similar, unknown or different;
//! - **parent**: whether the parent is the element the remembered parent
//!   became;
//! - **content**: the names inside an unnamed container (rows, groups);
//! - **sole child**: being the only element of its role in the same parent
//!   (a calculator's display keeps its identity while its text changes);
//! - **order**: for structured elements with nothing else to tell them apart
//!   (unnamed, empty), the position among same-role siblings, from the first
//!   or from the last;
//! - **native identity**: a platform identifier (`AXIdentifier`) shared by
//!   both elements; one unique in both observations makes the pair near
//!   certain. Different identifiers are no evidence against, because
//!   applications encode state in them (`StandardInputView;value:42`).
//!
//! Structured roles never change; a role change of a pixel-only element (a
//! visual hypothesis) lowers the score. Pairs scoring at least 0.5 are
//! accepted best first, one to one.
//!
//! # Ambiguity
//!
//! When swapping two assignments would be almost as good (identical buttons
//! that moved, a list scrolled by one row), the identity is not certain. The
//! identity confidence is `score × (0.5 + 0.5 × margin / 0.25)`, where
//! `margin` is how much worse the best swap would be (capped at 0.25): a
//! coin-toss assignment is reported as at most 0.5, never as fact.
//!
//! Elements that disappear are remembered for [`MEMORY`] observations, so an
//! element one source missed for a moment gets its ID back.
//!
//! Boundaries:
//! - identity is a scored hypothesis, not an absolute truth; ambiguous matches
//!   are reported as such;
//! - produces stable element IDs (and, later, observation deltas), nothing
//!   else.

#![forbid(unsafe_code)]

mod matching;
mod profile;
mod tracker;

pub use tracker::{MEMORY, Tracker, TrackingReport, UNCERTAIN};

#[cfg(test)]
mod tests;
