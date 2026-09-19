//! `argus serve`: the local service.

use argus_core::Observer;
use argus_server::{Config, DEFAULT_HISTORY, DEFAULT_PORT, Server};

use crate::commands::observe::PerceptionArgs;
use crate::output::print_text;

#[derive(Debug, clap::Args)]
pub(crate) struct ServeArgs {
    /// Port to listen on, on 127.0.0.1 only. `0` picks a free port.
    #[arg(long, env = "ARGUS_PORT", default_value_t = DEFAULT_PORT)]
    port: u16,

    /// Observations kept in memory for `/v1/observation/{id}`,
    /// `/v1/changes` and `/v1/elements`.
    #[arg(long, value_name = "N", default_value_t = DEFAULT_HISTORY as u32,
          value_parser = clap::value_parser!(u32).range(1..=1024))]
    history: u32,

    #[command(flatten)]
    perception: PerceptionArgs,
}

pub(crate) fn run(args: &ServeArgs) -> anyhow::Result<()> {
    let (incremental, verify) =
        (!args.perception.no_incremental, args.perception.verify_incremental);
    let config = Config { port: args.port, history: args.history as usize };
    let server = Server::bind(
        &config,
        Box::new(move || {
            Ok(Observer::new()?.with_incremental(incremental).with_verification(verify))
        }),
    )?;
    // The address on stdout, so that a program starting the service with
    // `--port 0` knows where to connect.
    print_text(&format!("listening on http://{}\n", server.local_addr()))?;
    server.run()?;
    Ok(())
}
