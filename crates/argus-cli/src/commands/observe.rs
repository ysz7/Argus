//! `argus observe`: print one observation as JSON.

use anyhow::bail;
use argus_core::Observer;
use argus_protocol::Observation;

use crate::commands::target::AppTargetArgs;
use crate::output::print_json;

#[derive(Debug, clap::Args)]
pub(crate) struct ObserveArgs {
    #[command(flatten)]
    pub(crate) sources: SourceArgs,

    #[command(flatten)]
    pub(crate) target: AppTargetArgs,
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
    let observer = Observer::new()?;
    let observation = observer.observe(&args.target.target(), &args.sources.sources())?;
    ensure_valid(&observation)?;
    print_json(&serde_json::to_value(&observation)?)
}
