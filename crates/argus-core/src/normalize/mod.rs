//! Source normalization: the single gate every source's evidence passes
//! through before it can become part of an observation.
//!
//! Responsibilities:
//! - **coordinates** — frame pixels become global screen points;
//! - **visibility** — the source's clip area yields `visible` and
//!   `visible_bounds`;
//! - **text** — Unicode NFC, whitespace, invisible/deceptive characters,
//!   length limits (see [`text`]);
//! - **roles** — source-independent invariants (e.g. OCR alone never claims
//!   an interactive role);
//! - **states** — properties only where they are meaningful;
//! - **confidence** — no confidence for absent or unknown properties;
//! - **metadata** — native identity is carried along for debugging and
//!   tracking.
//!
//! Mapping native vocabularies (e.g. `AXButton`) to protocol roles stays in
//! each source adapter, because only the adapter knows that vocabulary.

mod text;

use std::collections::HashMap;

use argus_protocol::{
    Bounds, CandidateId, Confidence, ElementCandidate, ElementState, Region, Role, Source,
    SourceCandidate,
};

use self::text::{clean_label, clean_value};

/// Candidates that went through normalization.
///
/// Only [`normalize`] can create this type, and observations can only be
/// assembled from it, so no evidence reaches an observation un-normalized.
///
/// ```compile_fail
/// // Bypassing normalization does not compile.
/// let raw = argus_core::Normalized(Vec::new());
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Normalized(Vec<ElementCandidate>);

impl Normalized {
    /// The normalized candidates, parents before children.
    pub fn candidates(&self) -> &[ElementCandidate] {
        &self.0
    }
}

/// Normalizes the output of one source.
///
/// Candidates whose region cannot be grounded (e.g. non-finite coordinates)
/// are dropped; their children are attached to the nearest kept ancestor, and
/// relations pointing at them are removed.
pub fn normalize(candidates: Vec<SourceCandidate>) -> Normalized {
    let mut grounded: HashMap<CandidateId, Bounds> = HashMap::with_capacity(candidates.len());
    let mut dropped: HashMap<CandidateId, Option<CandidateId>> = HashMap::new();
    for candidate in &candidates {
        match global_bounds(candidate.region) {
            Some(bounds) => {
                grounded.insert(candidate.id, bounds);
            }
            None => {
                tracing::debug!(id = candidate.id.0, "dropping candidate that cannot be grounded");
                dropped.insert(candidate.id, candidate.parent);
            }
        }
    }

    let kept_ancestor = |mut parent: Option<CandidateId>| {
        let mut steps = 0;
        while let Some(id) = parent {
            match dropped.get(&id) {
                Some(grandparent) if steps < dropped.len() => {
                    parent = *grandparent;
                    steps += 1;
                }
                Some(_) => return None,
                None => return grounded.contains_key(&id).then_some(id),
            }
        }
        None
    };

    let normalized = candidates
        .into_iter()
        .filter_map(|candidate| {
            let bounds = *grounded.get(&candidate.id)?;
            let parent = kept_ancestor(candidate.parent);
            let mut candidate = normalize_candidate(candidate, bounds, parent);
            candidate.relations.retain(|relation| grounded.contains_key(&relation.target));
            Some(candidate)
        })
        .collect();
    Normalized(normalized)
}

fn normalize_candidate(
    candidate: SourceCandidate,
    bounds: Bounds,
    parent: Option<CandidateId>,
) -> ElementCandidate {
    let SourceCandidate {
        id,
        role,
        name,
        value,
        description,
        clip,
        state,
        confidence,
        relations,
        meta,
        ..
    } = candidate;

    let role = normalize_role(meta.source, role);
    let name = clean_label(name.as_deref());
    let description =
        clean_label(description.as_deref()).filter(|text| Some(text) != name.as_ref());
    let value = clean_value(value.as_deref());

    let mut state = normalize_state(role, state);
    let mut visible_bounds = None;
    if let Some(clip) = clip {
        match bounds.intersection(&clip) {
            None => state.visible = Some(false),
            Some(visible) if visible == bounds => state.visible = Some(true),
            Some(visible) => {
                state.visible = Some(true);
                visible_bounds = Some(visible);
            }
        }
    }

    let confidence = Confidence {
        role: confidence.role.filter(|_| role != Role::Unknown),
        name: confidence.name.filter(|_| name.is_some()),
        value: confidence.value.filter(|_| value.is_some()),
        state: confidence.state.filter(|_| !state.is_unknown()),
        ..confidence
    };

    ElementCandidate {
        id,
        parent,
        role,
        name,
        value,
        description,
        bounds,
        visible_bounds,
        state,
        confidence,
        relations,
        meta,
    }
}

/// Converts a region to global screen points.
fn global_bounds(region: Region) -> Option<Bounds> {
    match region {
        Region::Screen(bounds) => Some(bounds),
        Region::Frame { geometry, rect } => {
            geometry.pixel_rect_to_bounds(rect.x, rect.y, rect.width, rect.height).ok()
        }
    }
}

/// Recognized text is evidence of text, not of a control: a word "Save"
/// found by OCR does not make a button. Interactive roles must come from
/// other evidence.
fn normalize_role(source: Source, role: Role) -> Role {
    match (source, role) {
        (Source::Ocr, Role::Text | Role::Unknown) => role,
        (Source::Ocr, other) => {
            tracing::debug!(?other, "OCR cannot establish interactive roles; using text");
            Role::Text
        }
        _ => role,
    }
}

/// Drops state properties that are meaningless for the role.
fn normalize_state(role: Role, mut state: ElementState) -> ElementState {
    let checkable =
        matches!(role, Role::Checkbox | Role::RadioButton | Role::MenuItem | Role::Unknown);
    if !checkable && state.checked.is_some() {
        tracing::debug!(?role, "dropping check state of a non-checkable role");
        state.checked = None;
    }
    state
}

#[cfg(test)]
mod tests {
    use argus_protocol::{
        CandidateRelation, CheckState, FrameGeometry, PixelRect, RelationKind, Score, SourceMeta,
    };

    use super::*;

    fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds {
        Bounds::new(x, y, width, height).unwrap()
    }

    fn candidate(id: u32, parent: Option<u32>, source: Source, role: Role) -> SourceCandidate {
        SourceCandidate {
            id: CandidateId(id),
            parent: parent.map(CandidateId),
            role,
            name: None,
            value: None,
            description: None,
            region: Region::Screen(bounds(0.0, 0.0, 10.0, 10.0)),
            clip: None,
            state: ElementState::default(),
            confidence: Confidence::new(Score::CERTAIN),
            relations: Vec::new(),
            meta: SourceMeta::new(source),
        }
    }

    fn one(candidate: SourceCandidate) -> ElementCandidate {
        let normalized = normalize(vec![candidate]);
        normalized.candidates()[0].clone()
    }

    #[test]
    fn converts_frame_pixels_to_global_points() {
        let geometry = FrameGeometry::new(bounds(-1440.0, 100.0, 400.0, 300.0), 2.0).unwrap();
        let mut ocr = candidate(0, None, Source::Ocr, Role::Text);
        ocr.region = Region::Frame {
            geometry,
            rect: PixelRect { x: 100.0, y: 40.0, width: 60.0, height: 20.0 },
        };
        assert_eq!(one(ocr).bounds, bounds(-1390.0, 120.0, 30.0, 10.0));
    }

    #[test]
    fn derives_visibility_from_clip() {
        let mut full = candidate(0, None, Source::Accessibility, Role::Button);
        full.clip = Some(bounds(0.0, 0.0, 100.0, 100.0));
        let full = one(full);
        assert_eq!((full.state.visible, full.visible_bounds), (Some(true), None));

        let mut partial = candidate(0, None, Source::Accessibility, Role::Button);
        partial.clip = Some(bounds(5.0, 0.0, 100.0, 100.0));
        let partial = one(partial);
        assert_eq!(partial.state.visible, Some(true));
        assert_eq!(partial.visible_bounds, Some(bounds(5.0, 0.0, 5.0, 10.0)));

        let mut hidden = candidate(0, None, Source::Accessibility, Role::Button);
        hidden.clip = Some(bounds(50.0, 50.0, 10.0, 10.0));
        hidden.state.visible = Some(true); // the clip overrides the source
        assert_eq!(one(hidden).state.visible, Some(false));

        let unknown = one(candidate(0, None, Source::Accessibility, Role::Button));
        assert_eq!(unknown.state.visible, None);
    }

    #[test]
    fn cleans_text() {
        let mut raw = candidate(0, None, Source::Accessibility, Role::Button);
        raw.name = Some("  Save\u{200B}  as ".to_owned());
        raw.description = Some("Save as".to_owned());
        raw.value = Some(String::new());
        let clean = one(raw);
        assert_eq!(clean.name.as_deref(), Some("Save as"));
        assert_eq!(clean.description, None, "a description equal to the name is redundant");
        assert_eq!(clean.value.as_deref(), Some(""));
    }

    #[test]
    fn ocr_cannot_claim_interactive_roles() {
        let mut ocr = candidate(0, None, Source::Ocr, Role::Button);
        ocr.name = Some("Save".to_owned());
        ocr.confidence.role = Some(Score::new(0.9).unwrap());
        let text = one(ocr);
        assert_eq!(text.role, Role::Text);

        // Vision may propose buttons; its confidence is kept as reported.
        let mut vision = candidate(0, None, Source::Vision, Role::Checkbox);
        vision.confidence.role = Some(Score::new(0.48).unwrap());
        let vision = one(vision);
        assert_eq!(vision.role, Role::Checkbox);
        assert_eq!(vision.confidence.role, Some(Score::new(0.48).unwrap()));
    }

    #[test]
    fn check_state_only_on_checkable_roles() {
        let mut button = candidate(0, None, Source::Accessibility, Role::Button);
        button.state.checked = Some(CheckState::Checked);
        assert_eq!(one(button).state.checked, None);

        let mut checkbox = candidate(0, None, Source::Accessibility, Role::Checkbox);
        checkbox.state.checked = Some(CheckState::Mixed);
        assert_eq!(one(checkbox).state.checked, Some(CheckState::Mixed));
    }

    #[test]
    fn no_confidence_for_absent_properties() {
        let mut raw = candidate(0, None, Source::Accessibility, Role::Unknown);
        raw.name = Some("   ".to_owned());
        raw.confidence = Confidence {
            role: Some(Score::CERTAIN),
            name: Some(Score::CERTAIN),
            value: Some(Score::CERTAIN),
            state: Some(Score::CERTAIN),
            ..Confidence::new(Score::CERTAIN)
        };
        let clean = one(raw);
        assert_eq!(clean.confidence, Confidence::new(Score::CERTAIN));
    }

    #[test]
    fn ungroundable_candidates_are_dropped_and_children_reattached() {
        let geometry = FrameGeometry::new(bounds(0.0, 0.0, 100.0, 100.0), 1.0).unwrap();
        let mut broken = candidate(1, Some(0), Source::Vision, Role::Group);
        broken.region = Region::Frame {
            geometry,
            rect: PixelRect { x: f32::NAN, y: 0.0, width: 10.0, height: 10.0 },
        };
        let mut root = candidate(0, None, Source::Vision, Role::Window);
        root.relations = vec![CandidateRelation {
            kind: RelationKind::Contains,
            target: CandidateId(1),
            confidence: None,
        }];
        let child = candidate(2, Some(1), Source::Vision, Role::Button);

        let normalized = normalize(vec![root, broken, child]);
        let ids: Vec<_> = normalized.candidates().iter().map(|c| c.id.0).collect();
        assert_eq!(ids, [0, 2]);
        assert_eq!(normalized.candidates()[1].parent, Some(CandidateId(0)));
        assert!(normalized.candidates()[0].relations.is_empty());
    }

    #[test]
    fn keeps_source_metadata() {
        let mut raw = candidate(0, None, Source::Accessibility, Role::Button);
        raw.meta.native_role = Some("AXButton".to_owned());
        raw.meta.native_id = Some("Seven".to_owned());
        assert_eq!(one(raw.clone()).meta, raw.meta);
    }
}
