//! `argus accessibility`: dump the raw native accessibility tree.
//!
//! A platform debugging tool: the output uses native vocabulary (`AXButton`)
//! and is not an Argus observation.

use crate::commands::target::AppTargetArgs;
use crate::output::print_json;

#[derive(Debug, clap::Args)]
pub(crate) struct AccessibilityArgs {
    #[command(flatten)]
    target: AppTargetArgs,
}

pub(crate) fn run(args: &AccessibilityArgs) -> anyhow::Result<()> {
    let backend = argus_core::accessibility::default_backend()?;
    let snapshot = backend.snapshot(&args.target.target())?;
    print_json(&serde_json::to_value(&snapshot)?)
}
