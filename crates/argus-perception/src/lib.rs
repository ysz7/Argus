//! Pixel-based perception backends for Argus: OCR and visual UI detection.
//!
//! Boundaries:
//! - backends report evidence (text, regions, role hypotheses) with honest
//!   confidence; they do not merge sources (that is `argus-fusion`);
//! - recognized text is evidence of text only, never of a control's role;
//! - inference runs locally; no frame is ever sent to a remote service.

mod change;
mod error;
mod incremental;
#[cfg(target_os = "macos")]
mod macos;
mod ocr;
mod vision;

pub use change::{FrameChange, frame_changes};
pub use error::{Error, Result};
pub use incremental::{Update, update_detections, update_ocr};
#[cfg(target_os = "macos")]
pub use macos::VisionOcr;
pub use ocr::{OcrBackend, OcrRegion, ocr_candidates};
pub use vision::{HeuristicDetector, VisualCandidate, VisualPerceptionBackend, vision_candidates};

/// The OCR backend for the current platform.
pub fn default_ocr_backend() -> Result<Box<dyn OcrBackend>> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(VisionOcr::new()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(Error::Unsupported)
    }
}
