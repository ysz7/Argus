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
    /// Run the MCP server on stdio, for AI clients (Claude Desktop, Claude Code).
    Mcp(commands::mcp::McpArgs),
    /// Run the local service: observations over HTTP on 127.0.0.1.
    Serve(commands::serve::ServeArgs),
    /// Observe the focused window of an application.
    Observe(commands::observe::ObserveArgs),
    /// Observe repeatedly and show what changed between observations.
    Watch(commands::watch::WatchArgs),
    /// Show the evidence and conflicts behind observed elements.
    Inspect(commands::inspect::InspectArgs),
    /// Measure accuracy on the recorded dataset, and live latency and memory.
    Benchmark(commands::benchmark::BenchmarkArgs),
    /// Check permissions, backends and the local service.
    Doctor(commands::doctor::DoctorArgs),
    /// Dump the raw native accessibility tree (developer tool).
    Accessibility(commands::accessibility::AccessibilityArgs),
    /// Capture a frame of a window or display (developer tool).
    Capture(commands::capture::CaptureArgs),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    logging::init(&cli.log_level, cli.log_format)?;
    tracing::debug!(version = env!("CARGO_PKG_VERSION"), "argus started");

    let result = match cli.command {
        Some(Command::Mcp(args)) => commands::mcp::run(&args),
        Some(Command::Serve(args)) => commands::serve::run(&args),
        Some(Command::Observe(args)) => commands::observe::run(&args),
        Some(Command::Watch(args)) => commands::watch::run(&args),
        Some(Command::Inspect(args)) => commands::inspect::run(&args),
        Some(Command::Benchmark(args)) => commands::benchmark::run_command(&args),
        Some(Command::Doctor(args)) => commands::doctor::run(&args),
        Some(Command::Accessibility(args)) => commands::accessibility::run(&args),
        Some(Command::Capture(args)) => commands::capture::run(&args),
        None => Ok(()),
    };
    match result {
        // The reader went away (`argus watch | head`): nothing left to do.
        Err(error) if is_broken_pipe(&error) => Ok(()),
        Err(error) => Err(with_hint(error)),
        result => result,
    }
}

/// Adds what to do about common setup problems.
fn with_hint(error: anyhow::Error) -> anyhow::Error {
    if is_capture_timeout(&error) {
        let others = commands::doctor::other_argus_processes();
        let context = match others.is_empty() {
            true => "screen capture did not answer; `argus doctor` checks the setup".to_owned(),
            false => format!(
                "screen capture did not answer. macOS lets only one running process of a \
                 program capture, and another argus process is running (pid {}): stop it, or \
                 ask the service (`argus serve`) instead",
                others.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
            ),
        };
        return error.context(context);
    }
    let taken = error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<argus_server::Error>(),
            Some(argus_server::Error::Bind { .. })
        )
    });
    if taken {
        return error.context(
            "the service cannot start: another program (perhaps another `argus serve`) uses the \
             port; choose another with `--port`, or run `argus doctor`",
        );
    }
    let denied = error.chain().any(|cause| {
        cause.downcast_ref::<argus_core::Error>().is_some_and(|e| e.code() == "permission_denied")
    });
    if denied {
        return error.context(
            "a macOS permission is missing; `argus doctor` shows which one and where to grant it",
        );
    }
    error
}

fn is_capture_timeout(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<argus_core::Error>(),
            Some(argus_core::Error::Capture(argus_core::capture::Error::Timeout(_)))
        )
    })
}

fn is_broken_pipe(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
            || cause
                .downcast_ref::<serde_json::Error>()
                .and_then(|json| json.io_error_kind())
                .is_some_and(|kind| kind == std::io::ErrorKind::BrokenPipe)
    })
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn setup_problems_point_to_the_doctor() {
        let denied = anyhow::Error::from(argus_core::Error::PermissionDenied(
            argus_core::Permission::Accessibility,
        ));
        assert!(with_hint(denied).to_string().contains("argus doctor"));
        let other = anyhow::anyhow!("something else");
        assert_eq!(with_hint(other).to_string(), "something else");
    }
}
