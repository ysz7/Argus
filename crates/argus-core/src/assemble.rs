//! Assembly of observations from element candidates.

use std::collections::HashMap;

use argus_protocol::{
    Application, CandidateId, Element, ElementCandidate, ElementId, Observation, ObservationId,
    Timestamp, Window,
};

/// Builds an observation from candidates of a single source.
///
/// Element IDs are assigned in candidate order (`e_1`, `e_2`, ...), and the
/// candidates' parent links become the structural hierarchy. A candidate
/// whose parent is not among `candidates` becomes a root.
pub fn assemble(
    id: ObservationId,
    timestamp: Timestamp,
    application: Option<Application>,
    window: Option<Window>,
    candidates: &[ElementCandidate],
) -> Observation {
    let ids: HashMap<CandidateId, ElementId> = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| (candidate.id, element_id(index)))
        .collect();

    let mut elements: Vec<Element> = candidates
        .iter()
        .map(|candidate| Element {
            id: ids[&candidate.id].clone(),
            role: candidate.role,
            name: candidate.name.clone(),
            value: candidate.value.clone(),
            description: candidate.description.clone(),
            bounds: candidate.bounds,
            visible_bounds: candidate.visible_bounds,
            state: candidate.state,
            confidence: candidate.confidence,
            sources: vec![candidate.source],
            parent: candidate.parent.and_then(|parent| ids.get(&parent).cloned()),
            children: Vec::new(),
        })
        .collect();

    let index: HashMap<ElementId, usize> =
        elements.iter().enumerate().map(|(i, element)| (element.id.clone(), i)).collect();
    for child in 0..elements.len() {
        if let Some(parent) = elements[child].parent.as_ref().map(|parent| index[parent]) {
            let child_id = elements[child].id.clone();
            elements[parent].children.push(child_id);
        }
    }

    let mut observation = Observation::new(id, timestamp);
    observation.application = application;
    observation.window = window;
    observation.elements = elements;
    observation
}

fn element_id(index: usize) -> ElementId {
    ElementId::new(format!("e_{}", index + 1)).expect("generated ids are non-empty")
}

#[cfg(test)]
mod tests {
    use argus_protocol::{Bounds, Confidence, ElementState, Role, Score, Source};

    use super::*;

    fn candidate(id: u32, parent: Option<u32>, role: Role) -> ElementCandidate {
        ElementCandidate {
            id: CandidateId(id),
            parent: parent.map(CandidateId),
            role,
            name: None,
            value: None,
            description: None,
            bounds: Bounds::new(0.0, 0.0, 10.0, 10.0).unwrap(),
            visible_bounds: None,
            state: ElementState::default(),
            confidence: Confidence::new(Score::CERTAIN),
            source: Source::Accessibility,
            native_role: Some("AXButton".to_owned()),
        }
    }

    fn observe(candidates: &[ElementCandidate]) -> Observation {
        let id = ObservationId::new("obs_1").unwrap();
        assemble(id, Timestamp(0), None, None, candidates)
    }

    #[test]
    fn builds_a_valid_hierarchy() {
        let observation = observe(&[
            candidate(10, None, Role::Window),
            candidate(11, Some(10), Role::Group),
            candidate(12, Some(11), Role::Button),
            candidate(13, Some(10), Role::Button),
        ]);
        observation.validate().unwrap();

        let ids: Vec<_> = observation.elements.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["e_1", "e_2", "e_3", "e_4"]);
        let root = &observation.elements[0];
        assert_eq!(root.children, [ElementId::new("e_2").unwrap(), ElementId::new("e_4").unwrap()]);
        assert_eq!(observation.elements[2].parent, Some(ElementId::new("e_2").unwrap()));
        assert_eq!(observation.elements[2].sources, [Source::Accessibility]);
    }

    #[test]
    fn orphans_become_roots() {
        let observation = observe(&[candidate(1, Some(99), Role::Button)]);
        assert_eq!(observation.elements[0].parent, None);
        observation.validate().unwrap();
    }
}
