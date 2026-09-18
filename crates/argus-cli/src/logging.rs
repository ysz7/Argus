//! Structured logging setup.
//!
//! Logs go to stderr so that stdout stays reserved for machine-readable output
//! (observations, JSON). Log records must never contain screen content
//! (recognized text, element names, pixels) at any level.

use anyhow::Context;
use tracing_subscriber::EnvFilter;

/// Output format of log records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum LogFormat {
    /// Human-readable text.
    Text,
    /// One JSON object per line.
    Json,
}

/// Installs the global tracing subscriber.
pub(crate) fn init(filter: &str, format: LogFormat) -> anyhow::Result<()> {
    let filter =
        EnvFilter::try_new(filter).with_context(|| format!("invalid log filter `{filter}`"))?;
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr);

    let result = match format {
        LogFormat::Text => builder.try_init(),
        LogFormat::Json => builder.json().try_init(),
    };
    result.map_err(|err| anyhow::anyhow!(err).context("failed to install logger"))
}
