//! Replay of recorded evidence (see `fusion_golden.rs` for how to record).

use std::path::{Path, PathBuf};

use argus_core::accessibility::{AxSnapshot, candidates};
use argus_core::{Fused, assemble, fuse, normalize};
use argus_protocol::{
    CandidateId, Observation, ObservationId, Region, SourceCandidate, SourceMeta, Timestamp,
};

pub(crate) fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Turns a single-source observation back into the candidates it came from.
fn replay(observation: &Observation) -> Vec<SourceCandidate> {
    observation
        .elements
        .iter()
        .enumerate()
        .map(|(index, element)| SourceCandidate {
            id: CandidateId(index as u32),
            parent: None,
            role: element.role,
            name: element.name.clone(),
            value: element.value.clone(),
            text: None,
            description: element.description.clone(),
            region: Region::Screen(element.bounds),
            clip: None,
            state: element.state,
            confidence: element.confidence,
            relations: Vec::new(),
            meta: SourceMeta::new(element.sources[0]),
        })
        .collect()
}

/// Fuses the recording in `dir` (one file per source) into an observation
/// with the given ID. Application and window are taken from the first pixel
/// recording.
pub(crate) fn fuse_recording(dir: &Path, id: &str) -> (Observation, Fused) {
    let read = |file: &str| std::fs::read_to_string(dir.join(file)).ok();

    let mut evidence = Vec::new();
    let mut header = None;
    if let Some(json) = read("accessibility.json") {
        let snapshot: AxSnapshot = serde_json::from_str(&json).unwrap();
        evidence.push(normalize(candidates(&snapshot)));
    }
    for file in ["ocr.json", "vision.json"] {
        if let Some(json) = read(file) {
            let observation: Observation = serde_json::from_str(&json).unwrap();
            evidence.push(normalize(replay(&observation)));
            header.get_or_insert((observation.application, observation.window));
        }
    }
    let (application, window) = header.unwrap_or_default();
    let fused = fuse(&evidence);
    let id = ObservationId::new(id).unwrap();
    (assemble(id, Timestamp(0), application, window, &fused), fused)
}
