//! Assembly of observations from fused elements.

use argus_protocol::{
    Application, Element, ElementId, Observation, ObservationId, Relation, Timestamp, Window,
};

use crate::fuse::Fused;

/// Builds an observation from fused elements.
///
/// Element IDs are assigned in fusion order (`e_1`, `e_2`, ...), which lists
/// parents before children; parent links become the structural hierarchy.
pub fn assemble(
    id: ObservationId,
    timestamp: Timestamp,
    application: Option<Application>,
    window: Option<Window>,
    fused: &Fused,
) -> Observation {
    let fused = &fused.0;
    let ids: Vec<ElementId> = (0..fused.elements.len()).map(element_id).collect();

    let mut elements: Vec<Element> = fused
        .elements
        .iter()
        .enumerate()
        .map(|(index, element)| Element {
            id: ids[index].clone(),
            role: element.role,
            name: element.name.clone(),
            value: element.value.clone(),
            text: element.text.clone(),
            description: element.description.clone(),
            bounds: element.bounds,
            visible_bounds: element.visible_bounds,
            state: element.state,
            confidence: element.confidence,
            sources: element.sources.clone(),
            parent: element.parent.map(|parent| ids[parent].clone()),
            children: Vec::new(),
        })
        .collect();

    for (child, element) in fused.elements.iter().enumerate() {
        if let Some(parent) = element.parent {
            elements[parent].children.push(ids[child].clone());
        }
    }

    let relations = fused
        .relations
        .iter()
        .map(|relation| Relation {
            kind: relation.kind,
            from: ids[relation.from].clone(),
            to: ids[relation.to].clone(),
            confidence: relation.confidence,
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
        Bounds, CandidateId, CandidateRelation, Confidence, ElementState, Region, RelationKind,
        Role, Score, Source, SourceCandidate, SourceMeta,
    };

    use super::*;
    use crate::fuse::fuse;
    use crate::normalize::normalize;

    fn candidate(id: u32, parent: Option<u32>, role: Role) -> SourceCandidate {
        SourceCandidate {
            id: CandidateId(id),
            parent: parent.map(CandidateId),
            role,
            name: None,
            value: None,
            text: None,
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
        assemble(id, Timestamp(0), None, None, &fuse(&[normalize(candidates.to_vec())]))
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
