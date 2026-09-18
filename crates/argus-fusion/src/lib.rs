//! Fusion of evidence from multiple sources into single Argus elements.
//!
//! Boundaries:
//! - deterministic, testable heuristics; no LLM-based fusion;
//! - conflicts between sources are surfaced, never silently hidden;
//! - weak evidence is never promoted to high confidence.

#![forbid(unsafe_code)]
