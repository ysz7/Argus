//! `argus mcp`: the MCP server over stdio, started by an AI client. It
//! relays to one shared background process (`argus_mcp::daemon`).

use std::path::PathBuf;

use argus_mcp::{Config, Policy};

#[derive(Debug, clap::Args)]
pub(crate) struct McpArgs {
    /// Observe only: refuse every action (clicks, typing, keys).
    #[arg(long)]
    read_only: bool,

    /// Only these applications may be observed and driven (name or bundle
    /// ID); repeat for several. By default, any application that is not
    /// blocked.
    #[arg(long = "allow-app", value_name = "APP")]
    allow_apps: Vec<String>,

    /// Lift the built-in block of an application (terminals, password
    /// managers, System Settings, chat clients, VS Code, Cursor); repeat for
    /// several.
    #[arg(long = "unblock-app", value_name = "APP")]
    unblock_apps: Vec<String>,

    /// Append one JSON line per tool call to this file: tool, duration,
    /// text and image sizes. Never screen content.
    #[arg(long, value_name = "PATH")]
    log_file: Option<PathBuf>,

    /// Serve in this process instead of the shared background process
    /// (only one client can then observe at a time).
    #[arg(long)]
    standalone: bool,

    /// Run the shared background process (started by `argus mcp` itself).
    #[arg(long, hide = true)]
    daemon: bool,
}

pub(crate) fn run(args: &McpArgs) -> anyhow::Result<()> {
    let config = Config {
        policy: Policy {
            read_only: args.read_only,
            allowed_apps: args.allow_apps.clone(),
            unblocked_apps: args.unblock_apps.clone(),
        },
        log_file: args.log_file.clone(),
    };
    if args.daemon {
        argus_mcp::daemon::run_daemon()?;
        return Ok(());
    }
    if args.standalone {
        argus_mcp::serve(config, std::io::stdin().lock(), std::io::stdout().lock())?;
        return Ok(());
    }
    // Every client shares one background process: macOS lets only one
    // process of this executable capture the screen.
    let exe = std::env::current_exe()?;
    argus_mcp::daemon::relay(&config, || {
        let mut command = std::process::Command::new(&exe);
        command.args(["mcp", "--daemon"]);
        command
    })?;
    Ok(())
}
