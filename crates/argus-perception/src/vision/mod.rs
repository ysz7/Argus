//! Visual UI detection: finding controls in pixels, without accessibility.

mod heuristic;
mod raster;

use argus_protocol::{
    CandidateId, Confidence, ElementState, Frame, PixelRect, Region, Role, Source, SourceCandidate,
    SourceMeta,
};

pub use self::heuristic::HeuristicDetector;
use crate::Result;

/// A detector of UI elements in frames.
pub trait VisualPerceptionBackend {
    /// Finds UI elements in the frame.
    fn detect(&self, frame: &Frame) -> Result<Vec<VisualCandidate>>;
}

/// One detected UI element.
#[derive(Debug, Clone, PartialEq)]
pub struct VisualCandidate {
    /// Role hypothesis.
    pub role: Role,
    /// Where the element is, in frame pixels.
    pub rect: PixelRect,
    /// State hypotheses (e.g. `checked`).
    pub state: ElementState,
    /// Confidence of the element, its role and its state. Weak hypotheses
    /// stay weak: a role with confidence 0.48 is reported as 0.48.
    pub confidence: Confidence,
}

/// Converts detections in `frame` into source candidates.
pub fn vision_candidates(frame: &Frame, detections: &[VisualCandidate]) -> Vec<SourceCandidate> {
    let geometry = frame.geometry();
    detections
        .iter()
        .enumerate()
        .map(|(index, detection)| SourceCandidate {
            id: CandidateId(index as u32),
            parent: None,
            role: detection.role,
            name: None,
            value: None,
            text: None,
            description: None,
            region: Region::Frame { geometry, rect: detection.rect },
            clip: None,
            state: detection.state,
            confidence: detection.confidence,
            relations: Vec::new(),
            meta: SourceMeta {
                source: Source::Vision,
                native_role: Some("heuristic".to_owned()),
                native_id: None,
            },
        })
        .collect()
}
