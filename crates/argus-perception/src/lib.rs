//! Pixel-based perception backends for Argus: OCR and visual UI detection.
//!
//! Boundaries:
//! - backends report evidence (text, regions, role hypotheses) with honest
//!   confidence; they do not merge sources (that is `argus-fusion`);
//! - inference runs locally; no frame is ever sent to a remote service.
