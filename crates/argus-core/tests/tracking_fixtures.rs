//! Recorded sequences of a real window must be tracked into exactly the
//! committed identities. Runs on any OS: the evidence is replayed from
//! fixtures.
//!
//! A sequence is a directory under `tests/fixtures/tracking/` with one
//! recording (as described in `fusion_golden.rs`) per step, in name order.
//! The golden file lists every element of every step with its tracked ID and
//! identity confidence (`new` for elements seen for the first time).
//!
//! Run with `ARGUS_UPDATE_GOLDEN=1` to regenerate after an intentional change,
//! then review the diff.

mod common;

use std::fmt::Write as _;

use argus_core::tracking::{Tracker, TrackingReport};
use argus_protocol::{Observation, Role};

use common::repo;

/// Replays the steps of a sequence through fusion and one tracker.
fn track_sequence(name: &str) -> Vec<(String, Observation, TrackingReport)> {
    let dir = repo().join("tests/fixtures/tracking").join(name);
    let mut steps: Vec<_> =
        std::fs::read_dir(&dir).unwrap().map(|entry| entry.unwrap().path()).collect();
    steps.sort();
    let mut tracker = Tracker::new();
    steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let (mut observation, fused) =
                common::fuse_recording(step, &format!("obs_{}", index + 1));
            let report = tracker.track(&mut observation, &fused.native_ids());
            observation.validate().unwrap();
            let step = step.file_name().unwrap().to_string_lossy().into_owned();
            (step, observation, report)
        })
        .collect()
}

fn assert_golden(name: &str, steps: &[(String, Observation, TrackingReport)]) {
    let mut actual = String::new();
    for (step, observation, report) in steps {
        let TrackingReport { kept, restored, new, lost, uncertain, .. } = report;
        writeln!(
            actual,
            "# {step}: kept {kept}, restored {restored}, new {new}, lost {lost}, uncertain {uncertain}"
        )
        .unwrap();
        for element in &observation.elements {
            let identity = element
                .confidence
                .identity
                .map_or_else(|| "new".to_owned(), |score| format!("{:.2}", score.get()));
            let name = element.name.as_deref().map(|name| format!(" {name:?}")).unwrap_or_default();
            let role = serde_json::to_value(element.role).unwrap();
            let role = role.as_str().unwrap();
            writeln!(actual, "{:>5} {identity:>4} {role}{name}", element.id.as_str()).unwrap();
        }
    }
    let path = repo().join("tests/golden/tracking").join(format!("{name}.txt"));
    if std::env::var_os("ARGUS_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
    }
    let expected = std::fs::read_to_string(&path).unwrap();
    assert_eq!(actual, expected, "{name}: tracked identities differ from golden file");
}

#[test]
fn calculator_matches_golden() {
    assert_golden("calculator", &track_sequence("calculator"));
}

/// Pressing keys changes the display and the Clear key; everything else keeps
/// its ID with full confidence, also after the window moved.
#[test]
fn calculator_keys_keep_their_ids() {
    let steps = track_sequence("calculator");
    let id_of = |observation: &Observation, role: Role, name: &str| {
        let element = observation
            .elements
            .iter()
            .find(|e| e.role == role && e.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no {role:?} named {name}"));
        (element.id.clone(), element.confidence.identity.map(|score| score.get()))
    };

    let keys = [
        "Delete",
        "Percent",
        "Divide",
        "7",
        "8",
        "9",
        "Multiply",
        "4",
        "5",
        "6",
        "Subtract",
        "1",
        "2",
        "3",
        "Add",
        "Change Sign",
        "0",
        "Point",
        "Equals",
        "Mode",
        "Close",
        "Minimize",
    ];
    let (_, first, _) = &steps[0];
    for (step, observation, report) in &steps[1..] {
        assert!(report.continued, "{step}");
        for key in keys {
            let (id, identity) = id_of(observation, Role::Button, key);
            assert_eq!(id, id_of(first, Role::Button, key).0, "{step}: {key}");
            assert_eq!(identity, Some(1.0), "{step}: {key}");
        }
    }

    // The display is one element whose text changes: "0" → "7 + 57" → "7 + 5".
    let display = |index: usize, text: &str| id_of(&steps[index].1, Role::Text, text);
    let (zero, _) = display(0, "0");
    let (typed, typed_identity) = display(1, "7 + 57");
    let (deleted, _) = display(2, "7 + 5");
    assert_eq!((&zero, &typed), (&typed, &deleted));
    assert!(typed_identity.unwrap() < 1.0, "its text changed");

    // All Clear becomes Clear once something is typed: the same key.
    let (all_clear, _) = id_of(first, Role::Button, "All Clear");
    let (clear, identity) = id_of(&steps[1].1, Role::Button, "Clear");
    assert_eq!(all_clear, clear);
    assert!((0.5..1.0).contains(&identity.unwrap()));

    // Moving the window changes nothing.
    let (_, moved, report) = &steps[4];
    assert_eq!(report.new, 0, "{report:?}");
    assert!(moved.elements.iter().all(|e| e.confidence.identity.is_some()));
}
