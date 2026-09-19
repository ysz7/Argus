//! `argus benchmark`: measure Argus against recorded ground truth.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use argus_benchmark::dataset::{FrameInfo, Truth, TruthElement, save_step};
use argus_benchmark::report::percentile;
use argus_benchmark::{Mode, Options, Report, draft_truth, load_dataset, render, run};
use argus_core::capture::CaptureTarget;
use argus_core::{Observer, PerceptionMode, Timings};
use serde_json::json;

use crate::commands::observe::SourceArgs;
use crate::commands::target::AppTargetArgs;
use crate::output::{print_json, print_text};
use crate::overlay;

#[derive(Debug, clap::Args)]
pub(crate) struct BenchmarkArgs {
    #[command(subcommand)]
    command: BenchmarkCommand,
}

#[derive(Debug, clap::Subcommand)]
enum BenchmarkCommand {
    /// Score Argus on the recorded dataset (accuracy, identity, calibration).
    Run(RunArgs),
    /// Measure live latency and memory on a running application.
    Latency(LatencyArgs),
    /// Record a window's current state as a dataset step, with drafted truth.
    Record(RecordArgs),
    /// Draw a step's truth over its frame, to review it.
    Review(ReviewArgs),
}

#[derive(Debug, clap::Args)]
struct RunArgs {
    /// Dataset directory.
    #[arg(long, default_value = "benchmarks/dataset")]
    dataset: PathBuf,

    /// Modes to run, comma-separated (default: all).
    #[arg(long, value_enum, value_delimiter = ',')]
    mode: Vec<ModeArg>,

    /// Only cases whose name contains this.
    #[arg(long)]
    case: Option<String>,

    /// Observations per step: the first is scored, all are timed.
    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..=100))]
    repeat: u32,

    /// Print the full results as JSON.
    #[arg(long)]
    json: bool,

    /// List every missed element and false positive.
    #[arg(long)]
    explain: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum ModeArg {
    /// The accessibility tree alone.
    Accessibility,
    /// Visual detection alone.
    Vision,
    /// OCR and visual detection.
    Pixels,
    /// Everything fused.
    Full,
}

impl ModeArg {
    fn mode(self) -> Mode {
        match self {
            ModeArg::Accessibility => Mode::Accessibility,
            ModeArg::Vision => Mode::Vision,
            ModeArg::Pixels => Mode::Pixels,
            ModeArg::Full => Mode::Full,
        }
    }
}

#[derive(Debug, clap::Args)]
struct LatencyArgs {
    #[command(flatten)]
    target: AppTargetArgs,

    #[command(flatten)]
    sources: SourceArgs,

    /// Observations to make.
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(2..))]
    count: u32,

    /// Time between the starts of successive observations, in milliseconds.
    #[arg(long, value_name = "MS", default_value_t = 500)]
    interval_ms: u64,

    /// Print the results as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct RecordArgs {
    #[command(flatten)]
    target: AppTargetArgs,

    /// The step directory to write, e.g. `benchmarks/dataset/calculator/1_cleared`.
    #[arg(long)]
    out: PathBuf,

    /// Truth for elements the accessibility tree does not describe (a JSON
    /// array of truth elements, e.g. written by `benchmarks/apps/form.swift`).
    #[arg(long, value_name = "FILE")]
    extra_truth: Option<PathBuf>,

    /// Description of the case, written to `case.json` if it has none.
    #[arg(long)]
    description: Option<String>,
}

#[derive(Debug, clap::Args)]
struct ReviewArgs {
    /// The step directory.
    step: PathBuf,

    /// Where to write the PNG.
    #[arg(short, long)]
    output: PathBuf,

    /// Also draw insignificant elements (containers).
    #[arg(long)]
    all: bool,
}

pub(crate) fn run_command(args: &BenchmarkArgs) -> anyhow::Result<()> {
    match &args.command {
        BenchmarkCommand::Run(args) => run_dataset(args),
        BenchmarkCommand::Latency(args) => latency(args),
        BenchmarkCommand::Record(args) => record(args),
        BenchmarkCommand::Review(args) => review(args),
    }
}

fn run_dataset(args: &RunArgs) -> anyhow::Result<()> {
    let mut cases = load_dataset(&args.dataset)?;
    if let Some(filter) = &args.case {
        cases.retain(|case| case.name.contains(filter.as_str()));
        if cases.is_empty() {
            bail!("no case matches `{filter}`");
        }
    }
    let modes: Vec<Mode> = match args.mode.is_empty() {
        true => Mode::ALL.to_vec(),
        false => args.mode.iter().map(|mode| mode.mode()).collect(),
    };
    if modes.iter().any(|mode| mode.needs_ocr()) {
        // The first recognition of a process (or of a new binary) prepares
        // models; it must not count as latency.
        let started = Instant::now();
        let ocr = argus_core::perception::default_ocr_backend()?;
        ocr.detect(&cases[0].steps[0].frame)?;
        tracing::info!(elapsed_ms = started.elapsed().as_millis() as u64, "OCR warmed up");
    }
    let factory = || argus_core::perception::default_ocr_backend().ok();
    let results = run(
        &cases,
        &Options { modes, repeat: args.repeat as usize, ocr: &factory, explain: args.explain },
    );
    let steps = cases.iter().map(|case| case.steps.len()).sum();
    let report = Report::new(args.dataset.display().to_string(), cases.len(), steps, results);
    if args.json {
        return print_json(&serde_json::to_value(&report)?);
    }
    print_text(&render(&report))
}

fn record(args: &RecordArgs) -> anyhow::Result<()> {
    let accessibility = argus_core::accessibility::default_backend()?;
    let capture = argus_core::capture::default_backend()?;
    let snapshot = accessibility.snapshot(&args.target.target())?;
    let window = snapshot.window.frame.context("the window has no frame")?;
    let window = argus_protocol::Bounds::new(
        window.x as f32,
        window.y as f32,
        window.width as f32,
        window.height as f32,
    )?;
    let pid = snapshot.application.pid;
    let captured = capture
        .windows()?
        .into_iter()
        .filter(|info| info.layer == 0 && info.application.pid == pid)
        .map(|info| (argus_benchmark::metrics::iou(&info.bounds, &window), info))
        .filter(|(overlap, _)| *overlap >= 0.5)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, info)| info)
        .context("the window is not on screen")?;
    let frame = capture.capture(CaptureTarget::Window(captured.id))?;

    let mut truth = draft_truth(&snapshot);
    if let Some(path) = &args.extra_truth {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let extra: Vec<TruthElement> = serde_json::from_str(&text)
            .with_context(|| format!("{} is not a list of truth elements", path.display()))?;
        let root = truth.elements.first().map(|window| window.id.clone());
        for mut element in extra {
            element.parent = element.parent.or_else(|| root.clone());
            truth.elements.push(element);
        }
    }
    let info = FrameInfo {
        bounds: frame.bounds(),
        scale_factor: frame.scale_factor(),
        // The pid means nothing once recorded.
        application: argus_protocol::Application { pid: None, ..snapshot.application.clone() },
        title: captured.title.clone(),
    };
    save_step(&args.out, &frame, &info, Some(&snapshot), &truth)?;
    if let Some(case) = args.out.parent() {
        let file = case.join("case.json");
        if !file.exists() {
            let description = args.description.clone().unwrap_or_default();
            std::fs::write(&file, format!("{:#}\n", json!({ "description": description })))?;
        }
    }
    let significant = truth.elements.iter().filter(|element| element.significant).count();
    print_text(&format!(
        "recorded {} ({} elements, {significant} significant); review the truth with \
         `argus benchmark review {} -o review.png`\n",
        args.out.display(),
        truth.elements.len(),
        args.out.display()
    ))
}

fn review(args: &ReviewArgs) -> anyhow::Result<()> {
    let step = argus_benchmark::dataset::load_step(&args.step)?;
    let truth: &Truth = &step.truth;
    let pixels = overlay::draw(&step.frame, &truth.to_observation(!args.all));
    crate::commands::capture::write_png(
        step.frame.width(),
        step.frame.height(),
        &pixels,
        &args.output,
    )?;
    print_text(&format!("wrote {}\n", args.output.display()))
}

/// Resident memory of this process, in MB.
fn resident_mb() -> Option<f64> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    let kilobytes: f64 = String::from_utf8(output.stdout).ok()?.trim().parse().ok()?;
    Some(kilobytes / 1024.0)
}

fn latency(args: &LatencyArgs) -> anyhow::Result<()> {
    let idle = resident_mb();
    let observer = Observer::new()?;
    let (target, sources) = (args.target.target(), args.sources.sources());
    let interval = Duration::from_millis(args.interval_ms);
    let mut timings: Vec<(Timings, Option<PerceptionMode>)> = Vec::new();
    let mut memory = Vec::new();
    for number in 0..args.count {
        let started = Instant::now();
        let inspection = observer.inspect(&target, &sources)?;
        timings.push((inspection.timings, inspection.perception.map(|report| report.mode)));
        memory.push(resident_mb().unwrap_or_default());
        if number + 1 < args.count {
            std::thread::sleep(interval.saturating_sub(started.elapsed()));
        }
    }
    // The first observation prepares models and perceives in full.
    let first = timings[0].0;
    let steady: Vec<&(Timings, Option<PerceptionMode>)> = timings[1..].iter().collect();
    let stat = |f: fn(&Timings) -> u64| {
        let values: Vec<u64> = steady.iter().map(|(t, _)| f(t)).collect();
        (percentile(values.clone(), 0.5), percentile(values, 0.95))
    };
    let stages = [
        ("accessibility", stat(|t| t.accessibility_ms), first.accessibility_ms),
        ("capture", stat(|t| t.capture_ms), first.capture_ms),
        ("ocr", stat(|t| t.ocr_ms), first.ocr_ms),
        ("vision", stat(|t| t.vision_ms), first.vision_ms),
        ("fusion", stat(|t| t.fusion_ms), first.fusion_ms),
        ("tracking", stat(|t| t.tracking_ms), first.tracking_ms),
        ("total", stat(|t| t.total_ms), first.total_ms),
    ];
    let mut modes = std::collections::BTreeMap::<&str, Vec<u64>>::new();
    for (timing, mode) in &steady {
        let name = match mode {
            Some(PerceptionMode::Full) => "full",
            Some(PerceptionMode::Partial) => "partial",
            Some(PerceptionMode::Unchanged) => "unchanged",
            None => "none",
        };
        modes.entry(name).or_default().push(timing.total_ms);
    }
    let half = &memory[memory.len() / 2..];
    let steady_memory = {
        let mut values = half.to_vec();
        values.sort_by(f64::total_cmp);
        values[values.len() / 2]
    };
    let peak = memory.iter().copied().fold(0.0, f64::max);

    if args.json {
        return print_json(&json!({
            "observations": args.count,
            "stages": stages.iter().map(|(name, (median, p95), first)| {
                (name.to_string(), json!({ "median_ms": median, "p95_ms": p95, "first_ms": first }))
            }).collect::<serde_json::Map<_, _>>(),
            "incremental": modes.iter().map(|(mode, totals)| {
                (mode.to_string(), json!({ "count": totals.len(), "median_total_ms": percentile(totals.clone(), 0.5) }))
            }).collect::<serde_json::Map<_, _>>(),
            "memory_mb": { "idle": idle, "steady": steady_memory, "peak": peak },
        }));
    }
    let mut out = format!(
        "Argus live latency — {} observations ({} after the first)\n\n{:<14} {:>8} {:>8} {:>8}\n",
        args.count,
        args.count - 1,
        "stage",
        "median",
        "p95",
        "first"
    );
    for (name, (median, p95), first) in stages {
        out.push_str(&format!("{name:<14} {median:>6} ms {p95:>5} ms {first:>5} ms\n"));
    }
    let incremental: Vec<String> = modes
        .iter()
        .map(|(mode, totals)| {
            format!("{mode} ×{} (median {} ms)", totals.len(), percentile(totals.clone(), 0.5))
        })
        .collect();
    out.push_str(&format!("\nperception: {}\n", incremental.join(", ")));
    out.push_str(&format!(
        "memory: idle {} MB, steady {:.0} MB, peak {:.0} MB\n",
        idle.map_or("?".to_owned(), |mb| format!("{mb:.0}")),
        steady_memory,
        peak
    ));
    print_text(&out)
}
