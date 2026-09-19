//! MCP adapter for Argus.
//!
//! A Model Context Protocol server over stdio (newline-delimited JSON-RPC
//! 2.0) that an AI client (Claude Desktop, Claude Code) starts as a local
//! process. It offers the agent view of Argus as tools, plus guarded
//! actions: the agent decides, Argus checks and executes
//! (`spec/ARGUS_MCP.md`).
//!
//! Boundaries:
//! - observations and images go only to the client that started the
//!   server, as tool results; nothing is stored or sent elsewhere;
//! - actions only in the observed application, after an `expect` check when
//!   the agent gives one, never in blocked applications, never with
//!   system-wide key combinations ([`guard`]).

#![forbid(unsafe_code)]

#[cfg(unix)]
pub mod daemon;
pub mod guard;
mod session;
mod tools;

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::Instant;

use serde_json::{Map, Value, json};

pub use crate::guard::Policy;
pub use crate::session::{Answer, Content, Session};

/// Protocol revisions this server speaks, newest first.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Server settings.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Config {
    /// What the agent may do.
    pub policy: Policy,
    /// Where to append one JSON line per tool call (tool, duration, sizes;
    /// never screen content).
    pub log_file: Option<PathBuf>,
}

/// Serves MCP requests from `input` until it ends, answering on `output`,
/// in this process.
pub fn serve(config: Config, input: impl BufRead, output: impl Write) -> std::io::Result<()> {
    warm_up();
    serve_connection(config, input, output, &std::sync::Mutex::new(()))
}

/// Serves one client; `turn` is held during each request so that clients
/// sharing a process never act on the screen at the same time.
pub(crate) fn serve_connection(
    config: Config,
    input: impl BufRead,
    mut output: impl Write,
    turn: &std::sync::Mutex<()>,
) -> std::io::Result<()> {
    let mut server = Server::new(config);
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let answer = {
            let _turn = turn.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            server.handle_line(&line)
        };
        if let Some(answer) = answer {
            writeln!(output, "{answer}")?;
            output.flush()?;
        }
    }
    Ok(())
}

/// JSON-RPC dispatch around a [`Session`].
#[derive(Debug)]
pub struct Server {
    session: Session,
    log_file: Option<PathBuf>,
}

impl Server {
    /// A server with `config`; nothing touches the screen until a tool is
    /// called.
    pub fn new(config: Config) -> Self {
        Self { session: Session::new(config.policy), log_file: config.log_file }
    }

    /// Answers one line of input; `None` for notifications.
    pub fn handle_line(&mut self, line: &str) -> Option<Value> {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(_) => return Some(error(Value::Null, -32700, "parse error")),
        };
        match message {
            // Batches (protocol 2025-03-26).
            Value::Array(messages) => {
                let answers: Vec<Value> =
                    messages.into_iter().filter_map(|message| self.handle(message)).collect();
                (!answers.is_empty()).then_some(Value::Array(answers))
            }
            message => self.handle(message),
        }
    }

    fn handle(&mut self, message: Value) -> Option<Value> {
        let id = message.get("id").cloned();
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            // A response to us (we send no requests) or garbage.
            return id.map(|id| error(id, -32600, "invalid request"));
        };
        // Notifications (`notifications/initialized`, cancellations) need no answer.
        let id = id?;
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => Ok(initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools::list() })),
            "tools/call" => self.call(&params),
            _ => Err((-32601, format!("method `{method}` not found"))),
        };
        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => error(id, code, &message),
        })
    }

    fn call(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or((-32602, "`name` is required".to_owned()))?;
        let known = tools::list()
            .as_array()
            .is_some_and(|tools| tools.iter().any(|tool| tool["name"] == name));
        if !known {
            return Err((-32602, format!("unknown tool `{name}`")));
        }
        let arguments =
            params.get("arguments").and_then(Value::as_object).cloned().unwrap_or_default();
        let started = Instant::now();
        let answer = self.session.call(name, &arguments);
        self.log(name, &arguments, &answer, started);
        Ok(to_json(&answer))
    }

    /// Records the call's size and duration, never its content.
    fn log(&self, name: &str, arguments: &Map<String, Value>, answer: &Answer, started: Instant) {
        let text: usize = answer
            .content
            .iter()
            .map(|content| match content {
                Content::Text(text) => text.len(),
                Content::Png(_) => 0,
            })
            .sum();
        let images: Vec<usize> = answer
            .content
            .iter()
            .filter_map(|content| match content {
                Content::Png(png) => Some(png.len()),
                Content::Text(_) => None,
            })
            .collect();
        let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        tracing::info!(
            tool = name,
            ms,
            text_bytes = text,
            images = images.len(),
            error = answer.is_error,
            "tool call"
        );
        let Some(path) = &self.log_file else { return };
        let record = json!({
            "time_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64),
            "tool": name,
            "arguments": arguments.keys().collect::<Vec<_>>(),
            "ms": ms,
            "text_bytes": text,
            "image_bytes": images,
            "error": answer.is_error,
        });
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| writeln!(file, "{record}"));
        if let Err(error) = written {
            tracing::warn!(%error, path = %path.display(), "the call log could not be written");
        }
    }
}

/// Loads the text recognition models in the background: the first
/// recognition of a newly built binary can take half a minute, longer than
/// clients wait for a tool call. Reads a blank synthetic frame, never the
/// screen.
pub(crate) fn warm_up() {
    use argus_core::perception::default_ocr_backend;
    use argus_protocol::{Bounds, Frame, FrameId, PixelBuffer, Timestamp};

    let spawned = std::thread::Builder::new().name("argus-warm-up".to_owned()).spawn(|| {
        let started = Instant::now();
        let blank = PixelBuffer::new(64, 32, vec![255; 64 * 32 * 4]).ok().and_then(|pixels| {
            let bounds = Bounds::new(0.0, 0.0, 64.0, 32.0).ok()?;
            Frame::new(FrameId(0), Timestamp(0), bounds, 1.0, pixels).ok()
        });
        if let (Some(frame), Ok(ocr)) = (blank, default_ocr_backend()) {
            let _ = ocr.detect(&frame);
            tracing::info!(ms = started.elapsed().as_millis() as u64, "text recognition ready");
        }
    });
    if let Err(error) = spawned {
        tracing::warn!(%error, "no warm-up thread");
    }
}

fn initialize(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
    let version = PROTOCOL_VERSIONS
        .iter()
        .find(|version| **version == requested)
        .copied()
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "argus", "title": "Argus", "version": env!("CARGO_PKG_VERSION") },
        "instructions": tools::INSTRUCTIONS,
    })
}

fn to_json(answer: &Answer) -> Value {
    let content: Vec<Value> = answer
        .content
        .iter()
        .map(|content| match content {
            Content::Text(text) => json!({ "type": "text", "text": text }),
            Content::Png(png) => {
                json!({ "type": "image", "data": base64(png), "mimeType": "image/png" })
            }
        })
        .collect();
    json!({ "content": content, "isError": answer.is_error })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Standard base64 with padding.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, byte)| n | u32::from(*byte) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe]), "//4=");
    }
}
