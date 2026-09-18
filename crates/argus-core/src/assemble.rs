//! Assembly of observations from element candidates.

use std::collections::HashMap;

use argus_protocol::{
    Application, CandidateId, Element, ElementId, Observation, ObservationId, Relation, Timestamp,
    Window,
};

use crate::normalize::Normalized;

/// Builds an observation from the normalized candidates of a single source.
///
/// Element IDs are assigned in candidate order (`e_1`, `e_2`, ...), and the
/// candidates' parent links become the structural hierarchy. A candidate
/// whose parent is not among the candidates becomes a root. Relations become
/// observation relations.
pub fn assemble(
    id: ObservationId,
    timestamp: Timestamp,
    application: Option<Application>,
    window: Option<Window>,
    normalized: &Normalized,
) -> Observation {
    let candidates = normalized.candidates();
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
            sources: vec![candidate.meta.source],
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

    let relations = candidates
        .iter()
        .flat_map(|candidate| {
            candidate.relations.iter().filter_map(|relation| {
                Some(Relation {
                    kind: relation.kind,
                    from: ids.get(&candidate.id)?.clone(),
                    to: ids.get(&relation.target)?.clone(),
                    confidence: relation.confidence,
                })
            })
        })
        .collect();

    let mut observation = Observation::new(id, timestamp);
    observation.application = application;
    observation.window = window;
    observation.elements = elements;
    observation.relations = relations;
    observation
}

fn element_id(index: usize) -> ElementId {
    ElementId::new(format!("e_{}", index + 1)).expect("generated ids are non-empty")
}

#[cfg(test)]
mod tests {
    use argus_protocol::{
        Bounds, CandidateRelation, Confidence, ElementState, Region, RelationKind, Role, Score,
        Source, SourceCandidate, SourceMeta,
    };

    use super::*;
    use crate::normalize::normalize;

    fn candidate(id: u32, parent: Option<u32>, role: Role) -> SourceCandidate {
        SourceCandidate {
            id: CandidateId(id),
            parent: parent.map(CandidateId),
            role,
            name: None,
            value: None,
            description: None,
            region: Region::Screen(Bounds::new(0.0, 0.0, 10.0, 10.0).unwrap()),
            clip: None,
            state: ElementState::default(),
            confidence: Confidence::new(Score::CERTAIN),
            relations: Vec::new(),
            meta: SourceMeta::new(Source::Accessibility),
        }
    }

    fn observe(candidates: &[SourceCandidate]) -> Observation {
        let id = ObservationId::new("obs_1").unwrap();
        assemble(id, Timestamp(0), None, None, &normalize(candidates.to_vec()))
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
    fn relations_become_observation_relations() {
        let mut label = candidate(2, None, Role::Text);
        label.relations = vec![CandidateRelation {
            kind: RelationKind::LabelFor,
            target: CandidateId(1),
            confidence: None,
        }];
        let observation = observe(&[candidate(1, None, Role::TextBox), label]);
        observation.validate().unwrap();
        assert_eq!(observation.relations.len(), 1);
        assert_eq!(observation.relations[0].from.as_str(), "e_2");
        assert_eq!(observation.relations[0].to.as_str(), "e_1");
    }

    #[test]
    fn orphans_become_roots() {
        let observation = observe(&[candidate(1, Some(99), Role::Button)]);
        assert_eq!(observation.elements[0].parent, None);
        observation.validate().unwrap();
    }
}
