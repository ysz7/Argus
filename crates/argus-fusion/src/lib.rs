//! Fusion of evidence from multiple sources into single Argus elements.
//!
//! Every source describes the same interface in its own way: accessibility
//! reports a structured tree, OCR reports lines of text, visual detection
//! reports control-shaped regions. Fusion turns their normalized candidates
//! into one set of elements in which every real object appears once.
//!
//! # Policy
//!
//! The policy is deterministic and applied in a fixed order:
//!
//! 1. **Structured sources** (accessibility) are taken as they are. They define
//!    the tree and are authoritative for every property they report.
//! 2. **Text** (OCR): a line lying inside a visible element that carries text
//!    (button, tab, text, text box, ...) is evidence *about* that element: it
//!    confirms the element's name or value, names an unnamed element, or
//!    disagrees with it. The smallest such element wins; an element whose text
//!    matches the line is preferred. Any other line becomes a new `text`
//!    element.
//! 3. **Visual detections**, largest first. A detection is
//!    - a *part* of an element when it is a glyph (`icon`/`unknown`) drawn
//!      on a text, or anything drawn inside a structured control that is not
//!      the control itself (controls are atomic);
//!    - the *same* object as an element when their boxes overlap well
//!      (IoU ≥ 0.5 with compatible roles, ≥ 0.75 otherwise) or when a
//!      compatible detection lies inside a control (e.g. the box of a
//!      checkbox whose accessibility frame includes its label);
//!    - otherwise a new element. Recognized text on one line inside a new
//!      button-like element becomes its name; other pixel-derived elements
//!      inside it become its children.
//!
//! New elements are attached to the smallest element that contains them.
//! Pixel sources never restructure the accessibility tree.
//!
//! # Conflicts
//!
//! Disagreements are recorded as [`Conflict`]s, never dropped. The more
//! authoritative source wins (accessibility over pixels, OCR text over a
//! glyph hypothesis) and the confidence of the kept property is lowered to
//! `kept × (1 − rejected / 2)`. Agreement never raises confidence above the
//! strongest single source: sources are not independent, and weak evidence
//! must not be inflated.
//!
//! Two differences are *not* conflicts that lower confidence:
//! - a visual role that is consistent with another source's role at the
//!   resolution of visual detection (a detector sees "button" where
//!   accessibility knows "link");
//! - visible text on a control that differs from its accessible name (an icon
//!   button named "Point" showing ","). It is recorded as
//!   [`Property::VisibleText`] for inspection.
//!
//! Boundaries:
//! - deterministic, testable heuristics; no LLM-based fusion;
//! - conflicts between sources are surfaced, never silently hidden;
//! - weak evidence is never promoted to high confidence.

#![forbid(unsafe_code)]

mod evidence;
mod fuse;
mod geometry;
mod order;
mod roles;
mod text;

pub use evidence::{Claim, Conflict, Contribution, ElementEvidence, Link, Property};
pub use fuse::{FusedElement, FusedRelation, Fusion, fuse};
