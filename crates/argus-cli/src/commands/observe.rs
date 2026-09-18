//! `argus observe`: print one observation as JSON.

use anyhow::bail;
use argus_core::Observer;

use crate::commands::target::AppTargetArgs;
use crate::output::print_json;

#[derive(Debug, clap::Args)]
pub(crate) struct ObserveArgs {
    /// Evidence source to use.
    #[arg(long, value_enum, default_value_t = Source::Accessibility)]
    source: Source,

    #[command(flatten)]
    target: AppTargetArgs,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum Source {
    /// The platform accessibility tree.
    Accessibility,
}

pub(crate) fn run(args: &ObserveArgs) -> anyhow::Result<()> {
    let observer = Observer::new()?;
    let observation = match args.source {
        Source::Accessibility => observer.observe_accessibility(&args.target.target())?,
    };
    if let Err(errors) = observation.validate() {
        let details: Vec<String> = errors.iter().map(ToString::to_string).collect();
        bail!("internal error: produced an invalid observation: {}", details.join("; "));
    }
    print_json(&serde_json::to_value(&observation)?)
}
