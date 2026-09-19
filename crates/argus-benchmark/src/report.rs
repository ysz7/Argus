//! Summaries and the text report.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::Serialize;

use crate::metrics::{BINS, Counts, Tally};
use crate::run::{CaseResult, Mode, StageTimes};

/// Everything measured, per mode.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub dataset: String,
    pub cases: usize,
    pub steps: usize,
    pub modes: Vec<ModeSummary>,
    pub results: Vec<CaseResult>,
}

/// One mode over the whole dataset.
#[derive(Debug, Clone, Serialize)]
pub struct ModeSummary {
    pub mode: Mode,
    pub counts: Counts,
    pub latency: Latency,
}

/// Replay latency: the platform stages (accessibility, capture) are read
/// from disk; recognition, detection, fusion and tracking are real.
/// Medians are over the scored observations (each step's first, perceived
/// in full or incrementally from the previous step); repeats of a step
/// show up only under `by_perception` (as `unchanged`).
#[derive(Debug, Clone, Default, Serialize)]
pub struct Latency {
    pub observations: usize,
    /// Median milliseconds per stage.
    pub median: StageTimes,
    /// 95th percentile of the total.
    pub p95_total: u64,
    /// Median total by perception mode (`full`, `partial`, `unchanged`).
    pub by_perception: BTreeMap<String, (usize, u64)>,
}

impl Report {
    pub fn new(dataset: String, cases: usize, steps: usize, results: Vec<CaseResult>) -> Self {
        let mut modes: Vec<Mode> = results.iter().map(|result| result.mode).collect();
        modes.dedup();
        let modes = modes
            .into_iter()
            .map(|mode| {
                let mut counts = Counts::default();
                let mut timings = Vec::new();
                for result in results.iter().filter(|result| result.mode == mode) {
                    counts.add(&result.counts);
                    timings.extend(result.timings.iter().copied());
                }
                ModeSummary { mode, counts, latency: latency(&timings) }
            })
            .collect();
        Self { dataset, cases, steps, modes, results }
    }

    pub fn mode(&self, mode: Mode) -> Option<&ModeSummary> {
        self.modes.iter().find(|summary| summary.mode == mode)
    }
}

fn latency(timings: &[crate::run::Timed]) -> Latency {
    let median = |values: Vec<u64>| percentile(values, 0.5);
    let scored: Vec<&crate::run::Timed> = timings.iter().filter(|t| t.scored).collect();
    let stage = |f: fn(&StageTimes) -> u64| median(scored.iter().map(|t| f(&t.times)).collect());
    let mut by_perception: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for timed in timings {
        if let Some(mode) = timed.perception {
            by_perception.entry(mode.to_owned()).or_default().push(timed.times.total);
        }
    }
    Latency {
        observations: scored.len(),
        median: StageTimes {
            accessibility: stage(|t| t.accessibility),
            capture: stage(|t| t.capture),
            ocr: stage(|t| t.ocr),
            vision: stage(|t| t.vision),
            fusion: stage(|t| t.fusion),
            tracking: stage(|t| t.tracking),
            total: stage(|t| t.total),
        },
        p95_total: percentile(scored.iter().map(|t| t.times.total).collect(), 0.95),
        by_perception: by_perception
            .into_iter()
            .map(|(mode, totals)| (mode, (totals.len(), median(totals))))
            .collect(),
    }
}

/// The value below which `share` of the values lie (nearest rank).
pub fn percentile(mut values: Vec<u64>, share: f64) -> u64 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    let rank = ((share * values.len() as f64).ceil() as usize).clamp(1, values.len());
    values[rank - 1]
}

fn percent(value: Option<f64>) -> String {
    value.map_or_else(|| "—".to_owned(), |value| format!("{:.1}%", value * 100.0))
}

fn fraction(part: usize, whole: usize) -> String {
    match whole {
        0 => "—".to_owned(),
        _ => format!("{part}/{whole}"),
    }
}

fn tally(tally: &Tally) -> String {
    match tally.accuracy() {
        None if tally.missing == 0 => "—".to_owned(),
        accuracy => format!(
            "{} (of {}, {} unreported)",
            percent(accuracy),
            tally.correct + tally.wrong,
            tally.missing
        ),
    }
}

/// The human-readable report.
pub fn render(report: &Report) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Argus Benchmark — {} cases, {} steps ({})\n",
        report.cases, report.steps, report.dataset
    );
    let _ = writeln!(
        out,
        "{:<14} {:>7} {:>9} {:>7} {:>7} {:>7} {:>6} {:>8} {:>8} {:>7}",
        "mode", "recall", "precision", "role", "name", "value", "IoU", "IoU≥.75", "identity", "ECE"
    );
    for summary in &report.modes {
        let c = &summary.counts;
        let _ = writeln!(
            out,
            "{:<14} {:>7} {:>9} {:>7} {:>7} {:>7} {:>6} {:>8} {:>8} {:>7}",
            summary.mode.name(),
            percent(c.recall()),
            percent(c.precision()),
            percent(c.role.accuracy()),
            percent(c.name.exact_rate()),
            percent(c.value.exact_rate()),
            c.mean_iou().map_or("—".to_owned(), |iou| format!("{iou:.3}")),
            percent(c.precise_rate()),
            percent(c.identity.recall()),
            c.calibration_error().map_or("—".to_owned(), |e| format!("{e:.3}")),
        );
    }

    let _ = writeln!(out, "\nPer case: recall / precision / role");
    let modes: Vec<Mode> = report.modes.iter().map(|summary| summary.mode).collect();
    let _ = write!(out, "{:<24}", "case");
    for mode in &modes {
        let _ = write!(out, " {:>24}", mode.name());
    }
    out.push('\n');
    let mut cases: Vec<&str> = report.results.iter().map(|r| r.case.as_str()).collect();
    cases.dedup();
    cases.sort_unstable();
    cases.dedup();
    for case in cases {
        let _ = write!(out, "{case:<24}");
        for mode in &modes {
            let cell = report
                .results
                .iter()
                .find(|r| r.case == case && r.mode == *mode)
                .map(|r| {
                    let c = &r.counts;
                    format!(
                        "{} / {} / {}",
                        short(c.recall()),
                        short(c.precision()),
                        short(c.role.accuracy())
                    )
                })
                .unwrap_or_default();
            let _ = write!(out, " {cell:>24}");
        }
        out.push('\n');
    }

    for summary in &report.modes {
        let c = &summary.counts;
        let _ = writeln!(out, "\n[{}]", summary.mode.name());
        let _ = writeln!(
            out,
            "  found {} ({} by containment), false positives {} ({} duplicates), parts {}, \
             name similarity {}, value similarity {}",
            fraction(c.found, c.truth),
            c.contained,
            c.false_positives,
            c.duplicates,
            c.parts,
            percent(c.name.mean_similarity()),
            percent(c.value.mean_similarity()),
        );
        let roles: Vec<String> = c
            .by_role
            .iter()
            .map(|(role, (truth, found))| format!("{role} {}", fraction(*found, *truth)))
            .collect();
        let _ = writeln!(out, "  recall by role: {}", roles.join(", "));
        for (state, t) in &c.states {
            if t.correct + t.wrong + t.missing > 0 {
                let _ = writeln!(out, "  state {state:<9} {}", tally(t));
            }
        }
        let _ = writeln!(out, "  parent          {}", tally(&c.parent));
        let _ = writeln!(out, "  row membership  {}", tally(&c.rows));
        let _ = writeln!(out, "  label relations {}", tally(&c.labels));
        let _ = writeln!(
            out,
            "  identity        kept {}, swapped {}, new ID {}",
            c.identity.correct, c.identity.wrong, c.identity.missing
        );
        let bins: Vec<String> = c
            .calibration
            .iter()
            .enumerate()
            .filter(|(_, bin)| bin.count > 0)
            .map(|(i, bin)| {
                format!(
                    "{:.1}–{:.1}: {} of {}",
                    i as f64 / BINS as f64,
                    (i + 1) as f64 / BINS as f64,
                    percent(Some(bin.correct as f64 / bin.count as f64)),
                    bin.count
                )
            })
            .collect();
        let _ = writeln!(out, "  calibration     {}", bins.join("; "));
        let l = &summary.latency;
        let _ = writeln!(
            out,
            "  latency (median ms, {} observations): ocr {}, vision {}, fusion {}, tracking {}, total {} (p95 {})",
            l.observations,
            l.median.ocr,
            l.median.vision,
            l.median.fusion,
            l.median.tracking,
            l.median.total,
            l.p95_total
        );
        for result in report.results.iter().filter(|r| r.mode == summary.mode) {
            for step in &result.steps {
                if !step.mistakes.is_empty() {
                    let _ = writeln!(out, "  {}/{}:", result.case, step.step);
                    for mistake in &step.mistakes {
                        let _ = writeln!(out, "    {mistake}");
                    }
                }
            }
        }
        if !l.by_perception.is_empty() {
            let modes: Vec<String> = l
                .by_perception
                .iter()
                .map(|(mode, (count, total))| format!("{mode} {total} ms (×{count})"))
                .collect();
            let _ = writeln!(out, "  incremental     {}", modes.join(", "));
        }
    }
    out
}

fn short(value: Option<f64>) -> String {
    value.map_or_else(|| "—".to_owned(), |value| format!("{:.0}%", value * 100.0))
}
