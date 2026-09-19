//! Golden tests: canonical observations must serialize to exactly the JSON
//! committed in `tests/golden/protocol/` and deserialize back unchanged.
//!
//! Run with `ARGUS_UPDATE_GOLDEN=1` to regenerate the files after an
//! intentional protocol change, then review the diff.

use std::path::PathBuf;

use argus_protocol::{
    Application, Bounds, CheckState, Confidence, Element, ElementChange, ElementId, ElementState,
    Observation, ObservationDelta, ObservationId, Relation, RelationKind, Role, Score, Source,
    Timestamp, Window,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/protocol").join(name)
}

fn check_golden<T>(name: &str, value: &T)
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let path = golden_path(name);
    let actual = serde_json::to_string_pretty(value).unwrap() + "\n";
    if std::env::var_os("ARGUS_UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &actual).unwrap();
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
    assert_eq!(actual, expected, "{name} differs from golden file");

    let parsed: T = serde_json::from_str(&expected).unwrap();
    assert_eq!(&parsed, value, "{name} does not round-trip");
}

fn id(value: &str) -> ElementId {
    ElementId::new(value).unwrap()
}

fn score(value: f32) -> Score {
    Score::new(value).unwrap()
}

fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds {
    Bounds::new(x, y, width, height).unwrap()
}

/// Accessibility-sourced element with full confidence.
fn ax_element(id_: &str, role: Role, name: Option<&str>, frame: Bounds) -> Element {
    Element {
        id: id(id_),
        role,
        name: name.map(str::to_owned),
        value: None,
        description: None,
        bounds: frame,
        visible_bounds: None,
        state: ElementState { enabled: Some(true), visible: Some(true), ..Default::default() },
        confidence: Confidence {
            role: Some(Score::CERTAIN),
            name: name.map(|_| Score::CERTAIN),
            bounds: Some(Score::CERTAIN),
            state: Some(Score::CERTAIN),
            ..Confidence::new(Score::CERTAIN)
        },
        sources: vec![Source::Accessibility],
        parent: None,
        children: Vec::new(),
    }
}

/// A "Delete file?" dialog: a hierarchy with states, a pixel-only element, a
/// partially visible element, relations and tracked identities.
fn dialog_observation() -> Observation {
    let mut window =
        ax_element("e_1", Role::Window, Some("Documents"), bounds(100.0, 80.0, 800.0, 600.0));
    let mut dialog = ax_element("e_2", Role::Dialog, None, bounds(300.0, 200.0, 400.0, 180.0));
    let mut message =
        ax_element("e_3", Role::Text, Some("Delete file?"), bounds(320.0, 220.0, 360.0, 20.0));
    let mut checkbox = ax_element(
        "e_4",
        Role::Checkbox,
        Some("Don't ask again"),
        bounds(320.0, 260.0, 160.0, 18.0),
    );
    let mut cancel =
        ax_element("e_5", Role::Button, Some("Cancel"), bounds(500.0, 330.0, 90.0, 28.0));
    let mut delete =
        ax_element("e_6", Role::Button, Some("Delete"), bounds(600.0, 330.0, 90.0, 28.0));

    window.children = vec![id("e_2")];
    dialog.parent = Some(id("e_1"));
    dialog.children = vec![id("e_3"), id("e_4"), id("e_5"), id("e_6")];
    for child in [&mut message, &mut checkbox, &mut cancel, &mut delete] {
        child.parent = Some(id("e_2"));
    }

    window.state.focused = Some(false);
    checkbox.state.checked = Some(CheckState::Unchecked);
    delete.state.focused = Some(true);
    delete.description = Some("Moves the file to the Trash".to_owned());
    // Confirmed by OCR as well.
    delete.sources.push(Source::Ocr);

    // An icon only visible in pixels: weak evidence stays weak.
    let icon = Element {
        id: id("e_7"),
        role: Role::Icon,
        name: None,
        value: None,
        description: None,
        bounds: bounds(870.0, 90.0, 16.0, 16.0),
        visible_bounds: None,
        state: ElementState::default(),
        confidence: Confidence {
            role: Some(score(0.48)),
            bounds: Some(score(0.82)),
            ..Confidence::new(score(0.74))
        },
        sources: vec![Source::Vision],
        parent: None,
        children: Vec::new(),
    };

    // A text field partially scrolled out of view.
    let mut search =
        ax_element("e_8", Role::TextBox, Some("Search"), bounds(110.0, 640.0, 300.0, 60.0));
    search.value = Some(String::new());
    search.visible_bounds = Some(bounds(110.0, 640.0, 300.0, 40.0));
    search.state.editable = Some(true);

    // The second observation of a tracking session: everything but the
    // search field was seen before.
    for element in [&mut window, &mut dialog, &mut message, &mut checkbox, &mut cancel, &mut delete]
    {
        element.confidence.identity = Some(Score::CERTAIN);
    }
    let mut icon = icon;
    icon.confidence.identity = Some(score(0.62));

    let mut observation =
        Observation::new(ObservationId::new("obs_000002").unwrap(), Timestamp(1_789_000_000_000));
    observation.previous = Some(ObservationId::new("obs_000001").unwrap());
    observation.application = Some(Application {
        name: Some("Finder".to_owned()),
        bundle_id: Some("com.apple.finder".to_owned()),
        pid: Some(4321),
    });
    observation.window = Some(Window {
        title: Some("Documents".to_owned()),
        bounds: Some(bounds(100.0, 80.0, 800.0, 600.0)),
    });
    observation.elements = vec![window, dialog, message, checkbox, cancel, delete, icon, search];
    observation.relations = vec![
        Relation {
            kind: RelationKind::LabelFor,
            from: id("e_3"),
            to: id("e_2"),
            confidence: Some(score(0.6)),
        },
        Relation { kind: RelationKind::Contains, from: id("e_1"), to: id("e_7"), confidence: None },
    ];
    observation
}

#[test]
fn full_observation_matches_golden() {
    let observation = dialog_observation();
    observation.validate().unwrap();
    check_golden("observation_full.json", &observation);
}

#[test]
fn minimal_observation_matches_golden() {
    let observation =
        Observation::new(ObservationId::new("obs_000002").unwrap(), Timestamp(1_789_000_000_500));
    observation.validate().unwrap();
    check_golden("observation_minimal.json", &observation);
}

#[test]
fn delta_matches_golden() {
    let mut saved =
        ax_element("e_9", Role::Dialog, Some("Saved"), bounds(300.0, 200.0, 400.0, 120.0));
    saved.state.focused = Some(true);

    let delta = ObservationDelta {
        from: ObservationId::new("obs_000001").unwrap(),
        to: ObservationId::new("obs_000002").unwrap(),
        timestamp: Some(Timestamp(1_789_000_000_500)),
        window: None,
        added: vec![saved],
        removed: vec![id("e_2")],
        changed: vec![
            ElementChange {
                id: id("e_5"),
                property: "state.enabled".to_owned(),
                from: json!(true),
                to: json!(false),
            },
            ElementChange {
                id: id("e_8"),
                property: "value".to_owned(),
                from: json!(""),
                to: json!("report"),
            },
            ElementChange {
                id: id("e_7"),
                property: "name".to_owned(),
                from: json!(null),
                to: json!("Close"),
            },
        ],
        added_relations: vec![Relation {
            kind: RelationKind::LabelFor,
            from: id("e_3"),
            to: id("e_8"),
            confidence: Some(score(0.5)),
        }],
        removed_relations: Vec::new(),
    };
    check_golden("delta.json", &delta);
}
