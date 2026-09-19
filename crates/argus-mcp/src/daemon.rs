//! One Argus process for all clients.
//!
//! macOS lets only one process of an executable capture the screen: a second
//! `argus mcp` (another Claude session) would time out on every observation.
//! So each `argus mcp` started by a client is a thin relay ([`relay`]) that
//! forwards its stdio to a shared background process ([`run_daemon`]) over a
//! Unix socket in the user's private temporary directory, starting it if
//! needed. The daemon serves every connection with its own session, one
//! request at a time across all of them, and exits when no client has been
//! connected for [`IDLE`].

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::Config;

/// How long the daemon stays without clients before it exits.
pub const IDLE: Duration = Duration::from_secs(60);
/// How long a relay waits for a daemon it started.
const START: Duration = Duration::from_secs(10);

/// The socket of the current user's daemon: in the per-user temporary
/// directory, which only the user can read.
pub fn socket_path() -> PathBuf {
    std::env::temp_dir().join("argus-mcp.sock")
}

/// Relays stdin and stdout to the daemon, starting it if none runs. The
/// first line sent is `config` (the daemon applies it to this client only).
pub fn relay(config: &Config, daemon: impl Fn() -> Command) -> std::io::Result<()> {
    let path = socket_path();
    let stream = match UnixStream::connect(&path) {
        Ok(stream) => stream,
        Err(_) => start(&path, daemon)?,
    };
    let mut to_daemon = stream.try_clone()?;
    writeln!(to_daemon, "{}", serde_json::to_string(config)?)?;
    let forward = std::thread::Builder::new().name("argus-relay".to_owned()).spawn(move || {
        let stdin = std::io::stdin().lock();
        for line in stdin.lines() {
            let Ok(line) = line else { break };
            if writeln!(to_daemon, "{line}").is_err() {
                break;
            }
        }
        // The client is gone: the daemon ends this session.
        let _ = to_daemon.shutdown(std::net::Shutdown::Write);
    })?;
    let mut stdout = std::io::stdout().lock();
    for line in BufReader::new(stream).lines() {
        writeln!(stdout, "{}", line?)?;
        stdout.flush()?;
    }
    drop(forward);
    Ok(())
}

/// Starts the daemon in its own process group (it outlives this relay) and
/// waits for its socket.
fn start(path: &Path, daemon: impl Fn() -> Command) -> std::io::Result<UnixStream> {
    let mut command = daemon();
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0);
    command.spawn()?;
    let deadline = Instant::now() + START;
    loop {
        match UnixStream::connect(path) {
            Ok(stream) => return Ok(stream),
            Err(error) if Instant::now() > deadline => return Err(error),
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// Runs the daemon: serves clients on the socket until it has been idle for
/// [`IDLE`]. Returns at once if another daemon already serves.
pub fn run_daemon() -> std::io::Result<()> {
    let path = socket_path();
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            if UnixStream::connect(&path).is_ok() {
                tracing::info!("another Argus daemon serves");
                return Ok(());
            }
            // Left over by a daemon that crashed.
            std::fs::remove_file(&path)?;
            UnixListener::bind(&path)?
        }
        Err(error) => return Err(error),
    };
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    tracing::info!(socket = %path.display(), "Argus daemon started");
    crate::warm_up();

    let active = Arc::new(AtomicUsize::new(0));
    let last_seen = Arc::new(Mutex::new(Instant::now()));
    let turn = Arc::new(Mutex::new(()));
    {
        let (active, last_seen, path) = (Arc::clone(&active), Arc::clone(&last_seen), path.clone());
        std::thread::Builder::new().name("argus-idle".to_owned()).spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(5));
                let idle = last_seen.lock().map(|at| at.elapsed()).unwrap_or_default();
                if active.load(Ordering::SeqCst) == 0 && idle > IDLE {
                    tracing::info!("no clients: Argus daemon exits");
                    let _ = std::fs::remove_file(&path);
                    std::process::exit(0);
                }
            }
        })?;
    }
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        active.fetch_add(1, Ordering::SeqCst);
        let (active, last_seen, turn) =
            (Arc::clone(&active), Arc::clone(&last_seen), Arc::clone(&turn));
        std::thread::Builder::new().name("argus-client".to_owned()).spawn(move || {
            if let Err(error) = client(stream, &turn) {
                tracing::debug!(%error, "client connection ended");
            }
            if let Ok(mut at) = last_seen.lock() {
                *at = Instant::now();
            }
            active.fetch_sub(1, Ordering::SeqCst);
        })?;
    }
    Ok(())
}

/// Serves one relay: its configuration line, then MCP.
fn client(stream: UnixStream, turn: &Mutex<()>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut first = String::new();
    reader.read_line(&mut first)?;
    let config: Config = serde_json::from_str(&first).map_err(std::io::Error::other)?;
    crate::serve_connection(config, reader, stream, turn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_sends_its_configuration_then_speaks_mcp() {
        let (mut relay, daemon) = UnixStream::pair().unwrap();
        let turn = Mutex::new(());
        let server = std::thread::spawn(move || client(daemon, &turn));
        let config = Config {
            policy: crate::Policy { read_only: true, ..crate::Policy::default() },
            log_file: None,
        };
        writeln!(relay, "{}", serde_json::to_string(&config).unwrap()).unwrap();
        writeln!(
            relay,
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"press_keys","arguments":{{"keys":"Return"}}}}}}"#
        )
        .unwrap();
        relay.shutdown(std::net::Shutdown::Write).unwrap();
        let mut answer = String::new();
        BufReader::new(&relay).read_line(&mut answer).unwrap();
        let answer: serde_json::Value = serde_json::from_str(&answer).unwrap();
        // The client's own configuration applies: read-only.
        assert_eq!(answer["result"]["isError"], true);
        assert!(answer["result"]["content"][0]["text"].as_str().unwrap().contains("read-only"));
        server.join().unwrap().unwrap();
    }
}
