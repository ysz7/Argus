//! Recorded evidence of real windows must fuse into exactly the committed
//! observations. Runs on any OS: the evidence is replayed from fixtures.
//!
//! A recording is a directory under `tests/fixtures/fusion/` with one file
//! per source, all taken of the same window within a few seconds:
//!
//! ```text
//! argus accessibility --app <APP>             > accessibility.json  (optional)
//! argus observe --app <APP> --sources ocr     > ocr.json            (optional)
//! argus observe --app <APP> --sources vision  > vision.json         (optional)
//! ```
//!
//! Run with `ARGUS_UPDATE_GOLDEN=1` to regenerate after an intentional change,
//! then review the diff.

use std::path::{Path, PathBuf};

use argus_core::accessibility::{AxSnapshot, candidates};
use argus_core::{Fused, assemble, fuse, normalize};
use argus_protocol::{
    CandidateId, Observation, ObservationId, Region, Role, Source, SourceCandidate, SourceMeta,
    Timestamp,
};

fn repo() -> PathBuf {
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

fn fuse_recording(name: &str) -> (Observation, Fused) {
    let dir = repo().join("tests/fixtures/fusion").join(name);
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
    let id = ObservationId::new("obs_golden").unwrap();
    (assemble(id, Timestamp(0), application, window, &fused), fused)
}

fn assert_golden(name: &str, observation: &Observation) {
    let path: &Path = &repo().join("tests/golden/fusion").join(format!("{name}.json"));
    let actual = serde_json::to_string_pretty(observation).unwrap() + "\n";
    if std::env::var_os("ARGUS_UPDATE_GOLDEN").is_some() {
        std::fs::write(path, &actual).unwrap();
    }
    let expected = std::fs::read_to_string(path).unwrap();
    assert_eq!(actual, expected, "{name}: fused observation differs from golden file");
}

#[test]
fn calculator_pixels_match_golden() {
    let (observation, _) = fuse_recording("calculator_pixels");
    observation.validate().unwrap();
    assert_golden("calculator_pixels", &observation);
}

/// Without accessibility, OCR names the buttons vision finds, and the
/// display's glyphs are not mistaken for icons.
#[test]
fn calculator_pixels_recover_named_buttons() {
    let (observation, fused) = fuse_recording("calculator_pixels");
    let named = |name: &str| {
        observation
            .elements
            .iter()
            .find(|e| e.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no element named {name}"))
    };

    for key in ["7", "8", "9", "4", "5", "6", "2", "+", "%"] {
        let button = named(key);
        assert_eq!(button.role, Role::Button, "{key}");
        assert_eq!(button.sources, [Source::Ocr, Source::Vision], "{key}");
        // The role is only as sure as the detector.
        assert!(button.confidence.role.unwrap().get() < 0.6, "{key}");
    }

    let display = named("7+5");
    assert_eq!(display.role, Role::Text);
    assert_eq!(display.sources, [Source::Ocr, Source::Vision], "glyphs are parts of the text");
    assert!(observation.elements.iter().all(|e| e.role != Role::Icon));

    // Every recognized line and every detection is accounted for exactly once.
    let contributions: usize = fused.evidence().map(|e| e.contributions.len()).sum();
    assert_eq!(contributions, 12 + 28);
    assert!(fused.evidence().all(|e| e.conflicts.is_empty()));
}

#[test]
fn calculator_matches_golden() {
    let (observation, _) = fuse_recording("calculator");
    observation.validate().unwrap();
    assert_golden("calculator", &observation);
}

/// With accessibility, pixels confirm the tree instead of adding to it: one
/// real object stays one element.
#[test]
fn calculator_sources_confirm_each_other() {
    let dir = repo().join("tests/fixtures/fusion/calculator");
    let snapshot: AxSnapshot =
        serde_json::from_str(&std::fs::read_to_string(dir.join("accessibility.json")).unwrap())
            .unwrap();
    let accessibility_only = normalize(candidates(&snapshot)).candidates().len();

    let (observation, fused) = fuse_recording("calculator");
    assert_eq!(observation.elements.len(), accessibility_only, "no duplicates");
    assert!(observation.elements.iter().all(|e| e.sources.contains(&Source::Accessibility)));

    let button = |name: &str| {
        observation
            .elements
            .iter()
            .find(|e| e.role == Role::Button && e.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no button named {name}"))
    };
    // OCR read "7" on the button accessibility calls "7"; vision saw the key.
    assert_eq!(button("7").sources, [Source::Accessibility, Source::Ocr, Source::Vision]);
    // The accessible name wins over the visible symbol, without penalty.
    let add = button("Add");
    assert_eq!(add.confidence.name.map(|s| s.get()), Some(1.0));

    let evidence: Vec<_> = fused.evidence().collect();
    let index = observation.elements.iter().position(|e| e.id == add.id).unwrap();
    let conflict = &evidence[index].conflicts[0];
    assert_eq!(conflict.property, argus_core::fusion::Property::VisibleText);
    assert_eq!(conflict.rejected.value, "+");
}

#[test]
fn proxy_generator_matches_golden() {
    let (observation, _) = fuse_recording("proxy_generator");
    observation.validate().unwrap();
    assert_golden("proxy_generator", &observation);
}

/// A Qt application that answers no accessibility requests: pixels alone
/// recover its named buttons and its radio buttons (second local test).
#[test]
fn proxy_generator_is_recovered_from_pixels() {
    let (observation, _) = fuse_recording("proxy_generator");
    for name in ["Start", "Add", "Remove", "Show", "Delete Proxies", "Extract Proxies"] {
        let button = observation
            .elements
            .iter()
            .find(|e| e.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no element named {name}"));
        assert_eq!(button.role, Role::Button, "{name}");
        assert_eq!(button.sources, [Source::Ocr, Source::Vision], "{name}");
    }
    let radios = observation.elements.iter().filter(|e| e.role == Role::RadioButton).count();
    assert_eq!(radios, 3, "the selected one is still seen as an icon");
}

#[test]
fn proxy_generator_full_matches_golden() {
    let (observation, _) = fuse_recording("proxy_generator_full");
    observation.validate().unwrap();
    assert_golden("proxy_generator_full", &observation);
}

/// A Qt application with a complete accessibility tree: every recognized
/// line confirms an accessible element (column headers with an unmapped
/// role, radio labels drawn beside their circles) instead of duplicating it.
#[test]
fn proxy_generator_text_is_not_duplicated() {
    let (observation, _) = fuse_recording("proxy_generator_full");
    let ocr_only: Vec<_> =
        observation.elements.iter().filter(|e| e.sources == [Source::Ocr]).collect();
    assert!(ocr_only.is_empty(), "{ocr_only:#?}");
    let radios: Vec<_> =
        observation.elements.iter().filter(|e| e.role == Role::RadioButton).collect();
    assert_eq!(radios.len(), 4);
    assert!(radios.iter().all(|r| r.sources.contains(&Source::Ocr)), "labels confirmed");
}
