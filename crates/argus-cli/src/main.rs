//! `argus` command-line interface.

#![forbid(unsafe_code)]

mod commands;
mod logging;
mod output;
mod overlay;

use clap::{Parser, Subcommand};

use crate::logging::LogFormat;

/// Argus: a local perception layer that turns user interfaces into
/// structured, grounded, confidence-aware observations.
#[derive(Debug, Parser)]
#[command(name = "argus", version, about, arg_required_else_help = true)]
struct Cli {
    /// Log filter, e.g. `info` or `argus_core=debug,warn`.
    #[arg(long, global = true, env = "ARGUS_LOG", default_value = "warn")]
    log_level: String,

    /// Log output format. Logs are always written to stderr.
    #[arg(long, global = true, value_enum, default_value_t = LogFormat::Text)]
    log_format: LogFormat,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Observe the focused window of an application.
    Observe(commands::observe::ObserveArgs),
    /// Show the evidence and conflicts behind observed elements.
    Inspect(commands::inspect::InspectArgs),
    /// Dump the raw native accessibility tree (developer tool).
    Accessibility(commands::accessibility::AccessibilityArgs),
    /// Capture a frame of a window or display (developer tool).
    Capture(commands::capture::CaptureArgs),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    logging::init(&cli.log_level, cli.log_format)?;
    tracing::debug!(version = env!("CARGO_PKG_VERSION"), "argus started");

    match cli.command {
        Some(Command::Observe(args)) => commands::observe::run(&args),
        Some(Command::Inspect(args)) => commands::inspect::run(&args),
        Some(Command::Accessibility(args)) => commands::accessibility::run(&args),
        Some(Command::Capture(args)) => commands::capture::run(&args),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
