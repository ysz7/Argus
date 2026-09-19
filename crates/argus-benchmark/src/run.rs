//! Running the dataset through the pipeline.

use std::time::Instant;

use argus_core::perception::OcrBackend;
use argus_core::{Inspection, PerceptionMode, Timings};
use argus_protocol::{Observation, ObservationId, Source, Timestamp};
use serde::Serialize;

use crate::dataset::{Case, Truth};
use crate::metrics::{Counts, Pairs, compare, compare_identity, explain, pair};
use crate::replay::{Stage, TARGET, observer};

/// Which evidence the pipeline gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The accessibility tree alone: what the platform says.
    Accessibility,
    /// Visual detection alone (no OCR): fast and deterministic.
    Vision,
    /// OCR and visual detection: what Argus recovers without the tree.
    Pixels,
    /// Everything fused: what Argus reports by default.
    Full,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Accessibility, Mode::Vision, Mode::Pixels, Mode::Full];

    pub fn sources(self) -> Vec<Source> {
        match self {
            Mode::Accessibility => vec![Source::Accessibility],
            Mode::Vision => vec![Source::Vision],
            Mode::Pixels => vec![Source::Ocr, Source::Vision],
            Mode::Full => vec![Source::Accessibility, Source::Ocr, Source::Vision],
        }
    }

    pub fn needs_ocr(self) -> bool {
        self.sources().contains(&Source::Ocr)
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Accessibility => "accessibility",
            Mode::Vision => "vision",
            Mode::Pixels => "pixels",
            Mode::Full => "full",
        }
    }
}

/// Creates a text recognizer (one per case and mode).
pub type OcrFactory<'a> = &'a dyn Fn() -> Option<Box<dyn OcrBackend>>;

/// How to run.
#[allow(missing_debug_implementations)] // holds a closure
pub struct Options<'a> {
    pub modes: Vec<Mode>,
    /// Observations per step: the first is scored, all are timed (later
    /// ones see an unchanged frame).
    pub repeat: usize,
    pub ocr: OcrFactory<'a>,
    /// Record the mistakes of every step ([`StepResult::mistakes`]).
    pub explain: bool,
}

/// Milliseconds per stage of one observation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct StageTimes {
    pub accessibility: u64,
    pub capture: u64,
    pub ocr: u64,
    pub vision: u64,
    pub fusion: u64,
    pub tracking: u64,
    pub total: u64,
}

impl From<Timings> for StageTimes {
    fn from(t: Timings) -> Self {
        Self {
            accessibility: t.accessibility_ms,
            capture: t.capture_ms,
            ocr: t.ocr_ms,
            vision: t.vision_ms,
            fusion: t.fusion_ms,
            tracking: t.tracking_ms,
            total: t.total_ms,
        }
    }
}

/// One timed observation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Timed {
    pub times: StageTimes,
    /// How pixels were perceived (`None` without pixel sources).
    pub perception: Option<&'static str>,
    /// The first observation of a step (scored); later ones repeat it.
    pub scored: bool,
}

/// The result of one step in one mode.
#[derive(Debug, Clone, Serialize)]
pub struct StepResult {
    pub step: String,
    pub counts: Counts,
    /// Missed elements and false positives, in words.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mistakes: Vec<String>,
    /// Why the observation failed (it then counts as empty).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The result of one case in one mode.
#[derive(Debug, Clone, Serialize)]
pub struct CaseResult {
    pub case: String,
    pub mode: Mode,
    pub counts: Counts,
    pub steps: Vec<StepResult>,
    pub timings: Vec<Timed>,
}

/// Runs every case in every mode.
pub fn run(cases: &[Case], options: &Options<'_>) -> Vec<CaseResult> {
    let mut results = Vec::new();
    for &mode in &options.modes {
        for case in cases {
            let started = Instant::now();
            let result = run_case(case, mode, options);
            tracing::info!(
                case = %case.name,
                mode = mode.name(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "benchmarked"
            );
            results.push(result);
        }
    }
    results
}

fn run_case(case: &Case, mode: Mode, options: &Options<'_>) -> CaseResult {
    let stage = Stage::default();
    let ocr = if mode.needs_ocr() { (options.ocr)() } else { None };
    let observer = observer(&stage, ocr);
    let sources = mode.sources();
    let mut number = 0;
    let mut counts = Counts::default();
    let mut steps = Vec::new();
    let mut timings = Vec::new();
    let mut previous: Option<(Truth, Observation, Pairs)> = None;

    for step in &case.steps {
        let mut scored = None;
        let mut error = None;
        for repeat in 0..options.repeat.max(1) {
            number += 1;
            stage.set(step, number);
            match observer.inspect(&TARGET, &sources) {
                Ok(inspection) => {
                    timings.push(timed(&inspection, repeat == 0));
                    scored.get_or_insert(inspection.observation);
                }
                Err(failure) => {
                    error.get_or_insert(failure.to_string());
                }
            }
        }
        let observation = scored.unwrap_or_else(|| {
            Observation::new(ObservationId::new("failed").expect("non-empty"), Timestamp(0))
        });
        let pairs = pair(&step.truth, &observation);
        let mut step_counts = compare(&step.truth, &observation, &pairs);
        let mistakes = match options.explain {
            true => explain(&step.truth, &observation, &pairs),
            false => Vec::new(),
        };
        if let Some((truth, before, before_pairs)) = &previous {
            step_counts.identity = compare_identity(
                (truth, before, before_pairs),
                (&step.truth, &observation, &pairs),
            );
        }
        counts.add(&step_counts);
        steps.push(StepResult { step: step.name.clone(), counts: step_counts, mistakes, error });
        previous = Some((step.truth.clone(), observation, pairs));
    }
    CaseResult { case: case.name.clone(), mode, counts, steps, timings }
}

fn timed(inspection: &Inspection, scored: bool) -> Timed {
    Timed {
        scored,
        times: inspection.timings.into(),
        perception: inspection.perception.as_ref().map(|report| match report.mode {
            PerceptionMode::Full => "full",
            PerceptionMode::Unchanged => "unchanged",
            PerceptionMode::Partial => "partial",
        }),
    }
}
