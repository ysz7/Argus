//! Parsing rules: forward compatibility, rejection of invalid values and
//! referential validation.

use std::path::PathBuf;

use argus_protocol::{
    ElementId, Observation, PROTOCOL_VERSION, RelationKind, Role, Source, ValidationError,
};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/protocol")
}

fn parse(json: &str) -> serde_json::Result<Observation> {
    serde_json::from_str(json)
}

fn read(path: PathBuf) -> String {
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()))
}

fn id(value: &str) -> ElementId {
    ElementId::new(value).unwrap()
}

#[test]
fn baseline_fixture_is_valid() {
    // Every invalid fixture is a one-field mutation of this document.
    let observation = parse(&read(fixtures().join("single_element.json"))).unwrap();
    // A 0.1 document stays valid: 0.2 only added optional fields.
    assert_eq!(observation.protocol_version, "0.1");
    assert_ne!(PROTOCOL_VERSION, "0.1");
    observation.validate().unwrap();
}

#[test]
fn every_invalid_fixture_is_rejected() {
    let mut checked = 0;
    for entry in std::fs::read_dir(fixtures().join("invalid")).unwrap() {
        let path = entry.unwrap().path();
        let result = parse(&read(path.clone()));
        assert!(result.is_err(), "{} must be rejected", path.display());
        checked += 1;
    }
    assert!(checked >= 10, "expected invalid fixtures, found {checked}");
}

#[test]
fn newer_documents_parse_with_unknowns_preserved_as_unknown() {
    let observation = parse(&read(fixtures().join("forward_compatible.json"))).unwrap();

    assert_eq!(observation.protocol_version, "0.2");
    let unknown = &observation.elements[0];
    assert_eq!(unknown.role, Role::Unknown);
    assert_eq!(unknown.sources, [Source::Accessibility, Source::Unknown]);
    assert_eq!(unknown.state.enabled, Some(true));
    assert_eq!(unknown.confidence.role.unwrap().get(), 0.7);
    assert_eq!(observation.relations[0].kind, RelationKind::Unknown);
    observation.validate().unwrap();
}

#[test]
fn absent_state_is_unknown_not_false() {
    let observation = parse(&read(fixtures().join("forward_compatible.json"))).unwrap();
    let state = observation.elements[1].state;
    assert_eq!(state.enabled, None);
    assert_eq!(state.visible, None);
    assert_eq!(state.checked, None);
}

fn golden_observation() -> Observation {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden/protocol/observation_full.json");
    parse(&read(path)).unwrap()
}

fn validation_errors(observation: &Observation) -> Vec<ValidationError> {
    observation.validate().expect_err("observation must be invalid")
}

#[test]
fn detects_duplicate_ids() {
    let mut observation = golden_observation();
    let mut duplicate = observation.elements[6].clone();
    duplicate.bounds = observation.elements[5].bounds;
    observation.elements.push(duplicate);
    assert!(
        validation_errors(&observation).contains(&ValidationError::DuplicateElementId(id("e_7")))
    );
}

#[test]
fn detects_dangling_references() {
    let mut observation = golden_observation();
    observation.elements[2].parent = Some(id("e_404"));
    observation.relations[0].to = id("e_405");

    let errors = validation_errors(&observation);
    assert!(errors.contains(&ValidationError::MissingElement {
        referenced_by: id("e_3"),
        missing: id("e_404"),
    }));
    assert!(errors.contains(&ValidationError::MissingRelationEndpoint(id("e_405"))));
}

#[test]
fn detects_parent_children_disagreement() {
    let mut observation = golden_observation();
    // e_2 lists e_3 as a child, but e_3 now claims e_1 as its parent.
    observation.elements[2].parent = Some(id("e_1"));

    let errors = validation_errors(&observation);
    assert!(
        errors
            .contains(&ValidationError::HierarchyMismatch { parent: id("e_1"), child: id("e_3") })
    );
    assert!(
        errors
            .contains(&ValidationError::HierarchyMismatch { parent: id("e_2"), child: id("e_3") })
    );
}

#[test]
fn detects_cycles() {
    let mut observation = golden_observation();
    // e_1 <-> e_2 become each other's parent and child.
    observation.elements[0].parent = Some(id("e_2"));
    observation.elements[1].children.push(id("e_1"));

    assert!(validation_errors(&observation).contains(&ValidationError::HierarchyCycle(id("e_1"))));
}

#[test]
fn detects_elements_without_sources() {
    let mut observation = golden_observation();
    observation.elements[4].sources.clear();
    assert_eq!(validation_errors(&observation), [ValidationError::NoSources(id("e_5"))]);
}
