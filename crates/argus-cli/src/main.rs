//! `argus` command-line interface.

#![forbid(unsafe_code)]

mod logging;

use clap::Parser;

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
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    logging::init(&cli.log_level, cli.log_format)?;
    tracing::debug!(version = env!("CARGO_PKG_VERSION"), "argus started");
    Ok(())
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
