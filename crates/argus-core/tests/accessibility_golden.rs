//! Recorded accessibility trees must turn into exactly the committed
//! observations. Runs on any OS: the macOS tree is replayed from a fixture.
//!
//! Run with `ARGUS_UPDATE_GOLDEN=1` to regenerate after an intentional change,
//! then review the diff.

use std::path::PathBuf;

use argus_core::accessibility::{AxSnapshot, candidates};
use argus_core::{assemble, fuse, normalize};
use argus_protocol::{Observation, ObservationId, Role, Timestamp, Window};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn observe_fixture(name: &str) -> Observation {
    let path = repo().join("tests/fixtures/accessibility").join(format!("{name}.json"));
    let snapshot: AxSnapshot =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let window = Window { title: snapshot.window.title.clone(), bounds: None };
    assemble(
        ObservationId::new("obs_golden").unwrap(),
        Timestamp(0),
        Some(snapshot.application.clone()),
        Some(window),
        &fuse(&[normalize(candidates(&snapshot))]),
    )
}

#[test]
fn calculator_matches_golden() {
    let observation = observe_fixture("calculator");
    observation.validate().unwrap();

    let path = repo().join("tests/golden/accessibility/calculator.json");
    let actual = serde_json::to_string_pretty(&observation).unwrap() + "\n";
    if std::env::var_os("ARGUS_UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &actual).unwrap();
    }
    let expected = std::fs::read_to_string(&path).unwrap();
    assert_eq!(actual, expected, "calculator observation differs from golden file");
}

#[test]
fn calculator_exposes_the_keypad() {
    let observation = observe_fixture("calculator");
    let button = |name: &str| {
        observation
            .elements
            .iter()
            .find(|e| e.role == Role::Button && e.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no button named {name}"))
    };

    let seven = button("7");
    assert_eq!(
        (seven.bounds.x(), seven.bounds.y(), seven.bounds.width(), seven.bounds.height()),
        (295.0, 666.0, 48.0, 48.0)
    );
    assert_eq!(seven.state.enabled, Some(true));

    // All digits share the keypad as their parent.
    for digit in ["0", "1", "2", "3", "4", "5", "6", "8", "9"] {
        assert_eq!(button(digit).parent, seven.parent, "{digit}");
    }

    // The display shows "0" as text; the zoom button is disabled.
    assert!(
        observation.elements.iter().any(|e| e.role == Role::Text && e.name.as_deref() == Some("0"))
    );
    assert_eq!(button("Zoom").state.enabled, Some(false));
}
