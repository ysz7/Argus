//! The benchmark dataset as a regression test: Argus must not get worse on
//! it. Thresholds sit a little below the measured baseline
//! (`benchmarks/README.md`); raise them when Argus improves.

use std::path::PathBuf;

use argus_benchmark::metrics::Counts;
use argus_benchmark::{Mode, Options, Report, load_dataset, run};

fn dataset() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/dataset")
}

fn report(modes: &[Mode]) -> Report {
    let cases = load_dataset(&dataset()).unwrap();
    let ocr = || argus_core::perception::default_ocr_backend().ok();
    let results =
        run(&cases, &Options { modes: modes.to_vec(), repeat: 1, ocr: &ocr, explain: false });
    let steps = cases.iter().map(|case| case.steps.len()).sum();
    Report::new("benchmarks/dataset".to_owned(), cases.len(), steps, results)
}

fn at_least(counts: &Counts, mode: Mode, recall: f64, precision: f64, role: f64) {
    let (r, p, o) =
        (counts.recall().unwrap(), counts.precision().unwrap(), counts.role.accuracy().unwrap());
    assert!(
        r >= recall && p >= precision && o >= role,
        "{mode:?}: recall {r:.3} (≥ {recall}), precision {p:.3} (≥ {precision}), role {o:.3} (≥ {role})"
    );
}

#[test]
fn the_dataset_loads_and_its_truth_is_consistent() {
    let cases = load_dataset(&dataset()).unwrap();
    assert!(cases.len() >= 4);
    for case in &cases {
        for step in &case.steps {
            let truth = &step.truth;
            let mut ids: Vec<&str> = truth.elements.iter().map(|e| e.id.as_str()).collect();
            ids.sort_unstable();
            let count = ids.len();
            ids.dedup();
            assert_eq!(ids.len(), count, "{}/{}: duplicate truth ids", case.name, step.name);
            for element in &truth.elements {
                if let Some(parent) = &element.parent {
                    assert!(truth.get(parent).is_some(), "{}/{}: {parent}", case.name, step.name);
                }
            }
            for relation in &truth.relations {
                assert!(truth.get(&relation.from).is_some() && truth.get(&relation.to).is_some());
            }
        }
    }
}

#[test]
fn accessibility_and_vision_do_not_regress() {
    let report = report(&[Mode::Accessibility, Mode::Vision]);
    let accessibility = &report.mode(Mode::Accessibility).unwrap().counts;
    // The truth is drafted from the tree: everything it describes is found.
    at_least(accessibility, Mode::Accessibility, 0.89, 1.0, 1.0);
    assert_eq!(accessibility.identity.wrong, 0);
    let vision = &report.mode(Mode::Vision).unwrap().counts;
    at_least(vision, Mode::Vision, 0.62, 0.97, 0.82);
}

#[test]
#[ignore = "runs OCR (macOS Vision): slow on a fresh binary"]
fn pixels_and_fusion_do_not_regress() {
    let report = report(&[Mode::Pixels, Mode::Full]);
    let pixels = &report.mode(Mode::Pixels).unwrap().counts;
    at_least(pixels, Mode::Pixels, 0.83, 0.95, 0.86);
    // Text in detected fields is their value (phase 14.1).
    assert!(pixels.value.exact_rate().unwrap() >= 0.35);
    let full = &report.mode(Mode::Full).unwrap().counts;
    // Fusion recovers what the tree misses (the drawn canvas).
    at_least(full, Mode::Full, 0.99, 0.96, 0.99);
    assert!(full.calibration_error().unwrap() < 0.07);
}
