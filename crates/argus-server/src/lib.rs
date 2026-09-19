//! Local Argus service.
//!
//! Exposes observations to external programs over a local HTTP API
//! (`spec/ARGUS_SERVICE.md`):
//!
//! ```text
//! GET /v1/health
//! GET /v1/observation              observe now
//! GET /v1/observation/{id}         an earlier observation
//! GET /v1/changes?since={id}       observe again, answer what changed
//! GET /v1/elements/{id}            an element and its evidence
//! ```
//!
//! Boundaries (security defaults):
//! - listens on 127.0.0.1 only, and refuses requests from web pages (an
//!   `Origin` header or a foreign `Host`);
//! - no cloud connectivity, no persistent screenshots, no screen-content
//!   telemetry: observations live in memory, in a bounded history;
//! - observes only when asked; serves observations and never accepts actions
//!   to execute.

#![forbid(unsafe_code)]

mod api;
mod http;
mod worker;

use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use worker::ObserverFactory;

use crate::api::Api;
use crate::http::{Response, read_request};
use crate::worker::{History, Worker};

/// The port the service listens on by default.
pub const DEFAULT_PORT: u16 = 7412;
/// Observations kept in memory by default.
pub const DEFAULT_HISTORY: usize = 32;
/// Tracking sessions (distinct applications and sources) kept at once; the
/// least recently used one ends when another starts.
pub const SESSIONS: usize = 8;
/// Connections served at once; more are refused.
const MAX_CONNECTIONS: usize = 16;
/// How long a client may take to send its request.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a client may take to receive the response.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Result alias of the service.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Why the service cannot run.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The port could not be bound (e.g. another service uses it).
    #[error("cannot listen on port {port}: {source}")]
    Bind {
        /// The requested port.
        port: u16,
        /// The underlying error.
        source: std::io::Error,
    },
    /// No observer could be created.
    #[error(transparent)]
    Observer(#[from] argus_core::Error),
    /// Accepting connections failed.
    #[error("the service stopped: {0}")]
    Io(#[from] std::io::Error),
}

impl Error {
    /// Stable, machine-readable identifier of the error class.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Bind { .. } => "bind_failed",
            Error::Observer(error) => error.code(),
            Error::Io(_) => "io_error",
        }
    }
}

/// Service settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Port on 127.0.0.1; `0` picks a free one.
    pub port: u16,
    /// Observations kept in memory for `/v1/observation/{id}`,
    /// `/v1/changes` and `/v1/elements`.
    pub history: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self { port: DEFAULT_PORT, history: DEFAULT_HISTORY }
    }
}

/// A bound, not yet running service.
#[derive(Debug)]
pub struct Server {
    listener: TcpListener,
    api: Arc<Api>,
}

impl Server {
    /// Binds 127.0.0.1:`config.port` and starts the observation thread, which
    /// creates observers with `factory`.
    pub fn bind(config: &Config, factory: ObserverFactory) -> Result<Self> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, config.port)))
            .map_err(|source| Error::Bind { port: config.port, source })?;
        let history = Arc::new(Mutex::new(History::new(config.history)));
        let worker = Worker::start(factory, Arc::clone(&history), SESSIONS)?;
        Ok(Self { listener, api: Arc::new(Api::new(worker, history)) })
    }

    /// The address the service listens on.
    pub fn local_addr(&self) -> SocketAddr {
        self.listener.local_addr().expect("a bound listener has an address")
    }

    /// Serves requests until accepting a connection fails.
    pub fn run(self) -> Result<()> {
        let port = self.local_addr().port();
        let active = Arc::new(AtomicUsize::new(0));
        tracing::info!(address = %self.local_addr(), "serving");
        for stream in self.listener.incoming() {
            let stream = match stream {
                Ok(stream) => stream,
                // A client that gave up before being accepted.
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionAborted => continue,
                Err(error) => return Err(error.into()),
            };
            if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                active.fetch_sub(1, Ordering::SeqCst);
                let busy = Response::error(503, "busy", "too many connections".to_owned());
                let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
                let _ = busy.write(&stream);
                continue;
            }
            let (api, active) = (Arc::clone(&self.api), Arc::clone(&active));
            std::thread::Builder::new()
                .name("argus-connection".to_owned())
                .spawn(move || {
                    serve(&api, &stream, port);
                    active.fetch_sub(1, Ordering::SeqCst);
                })
                .map_err(Error::Io)?;
        }
        Ok(())
    }
}

/// Answers one request on `stream`.
fn serve(api: &Api, stream: &TcpStream, port: u16) {
    let started = Instant::now();
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let (path, response) = match read_request(stream) {
        Ok(Ok(request)) => {
            let response = match request.check(port) {
                Ok(()) => api.handle(&request),
                Err(refusal) => refusal,
            };
            (request.path, response)
        }
        Ok(Err(refusal)) => (String::new(), refusal),
        Err(error) => {
            tracing::debug!(%error, "no request received");
            return;
        }
    };
    if let Err(error) = response.write(stream) {
        tracing::debug!(%error, "the client did not receive the response");
    }
    tracing::info!(
        path,
        status = response.status,
        code = response.code(),
        elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "answered"
    );
}
