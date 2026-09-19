//! `argus observe`: print observations as JSON.

use std::time::{Duration, Instant};

use anyhow::bail;
use argus_core::Observer;
use argus_protocol::Observation;

use crate::commands::target::AppTargetArgs;
use crate::output::{print_json, print_json_line};

#[derive(Debug, clap::Args)]
pub(crate) struct ObserveArgs {
    #[command(flatten)]
    pub(crate) sources: SourceArgs,

    #[command(flatten)]
    pub(crate) target: AppTargetArgs,

    /// Number of observations to make. Successive observations are tracked:
    /// elements that are still there keep their IDs. More than one
    /// observation is printed as JSON Lines (one observation per line).
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    pub(crate) count: u32,

    /// Time between the starts of successive observations, in milliseconds.
    #[arg(long, value_name = "MS", default_value_t = 1000)]
    pub(crate) interval_ms: u64,

    #[command(flatten)]
    pub(crate) perception: PerceptionArgs,
}

/// How successive frames are perceived.
#[derive(Debug, clap::Args)]
pub(crate) struct PerceptionArgs {
    /// Perceive every frame in full instead of only the regions that changed
    /// since the previous one.
    #[arg(long)]
    pub(crate) no_incremental: bool,

    /// Check every incremental perception against a full perception of the
    /// same frame and warn about differences (slow; for development).
    #[arg(long, conflicts_with = "no_incremental")]
    pub(crate) verify_incremental: bool,
}

impl PerceptionArgs {
    /// A default observer configured accordingly.
    pub(crate) fn observer(&self) -> anyhow::Result<Observer> {
        Ok(Observer::new()?
            .with_incremental(!self.no_incremental)
            .with_verification(self.verify_incremental))
    }
}

/// Selection of evidence sources.
#[derive(Debug, clap::Args)]
pub(crate) struct SourceArgs {
    /// Evidence sources to fuse, comma-separated. OCR and vision need the
    /// Screen Recording permission.
    #[arg(
        long = "sources",
        visible_alias = "source",
        value_enum,
        value_delimiter = ',',
        default_values_t = [Source::Accessibility, Source::Ocr, Source::Vision],
    )]
    pub(crate) list: Vec<Source>,
}

impl SourceArgs {
    pub(crate) fn sources(&self) -> Vec<argus_protocol::Source> {
        self.list.iter().map(|source| source.to_protocol()).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Source {
    /// The platform accessibility tree.
    Accessibility,
    /// Text recognized in the window's pixels.
    Ocr,
    /// UI elements detected in the window's pixels.
    Vision,
}

impl Source {
    pub(crate) fn to_protocol(self) -> argus_protocol::Source {
        match self {
            Source::Accessibility => argus_protocol::Source::Accessibility,
            Source::Ocr => argus_protocol::Source::Ocr,
            Source::Vision => argus_protocol::Source::Vision,
        }
    }
}

/// Fails if the observation breaks a protocol invariant.
pub(crate) fn ensure_valid(observation: &Observation) -> anyhow::Result<()> {
    if let Err(errors) = observation.validate() {
        let details: Vec<String> = errors.iter().map(ToString::to_string).collect();
        bail!("internal error: produced an invalid observation: {}", details.join("; "));
    }
    Ok(())
}

pub(crate) fn run(args: &ObserveArgs) -> anyhow::Result<()> {
    let observer = args.perception.observer()?;
    let (target, sources) = (args.target.target(), args.sources.sources());
    if args.count == 1 {
        let observation = observer.observe(&target, &sources)?;
        ensure_valid(&observation)?;
        return print_json(&serde_json::to_value(&observation)?);
    }
    let interval = Duration::from_millis(args.interval_ms);
    for number in 0..args.count {
        let started = Instant::now();
        let observation = observer.observe(&target, &sources)?;
        ensure_valid(&observation)?;
        print_json_line(&serde_json::to_value(&observation)?)?;
        if number + 1 < args.count {
            std::thread::sleep(interval.saturating_sub(started.elapsed()));
        }
    }
    Ok(())
}
