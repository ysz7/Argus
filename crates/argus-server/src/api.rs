//! The `/v1` API: routing, parameters and answers.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use argus_core::accessibility::AppTarget;
use argus_core::{Error as CoreError, Timings};
use argus_protocol::{ObservationDelta, PROTOCOL_VERSION, Source};
use serde_json::json;

use crate::http::{Request, Response};
use crate::worker::{Entry, History, ObserveRequest, Worker, lock};

/// Answers requests.
#[derive(Debug)]
pub(crate) struct Api {
    worker: Worker,
    history: Arc<Mutex<History>>,
    started: Instant,
}

impl Api {
    pub(crate) fn new(worker: Worker, history: Arc<Mutex<History>>) -> Self {
        Self { worker, history, started: Instant::now() }
    }

    pub(crate) fn handle(&self, request: &Request) -> Response {
        if request.method != "GET" {
            return Response::error(
                405,
                "method_not_allowed",
                "the service only answers GET requests".to_owned(),
            )
            .with_header("Allow", "GET".to_owned());
        }
        let path = request.path.strip_suffix('/').unwrap_or(&request.path);
        let segments: Vec<&str> = path.split('/').skip(1).collect();
        let result = match segments.as_slice() {
            ["v1", "health"] => self.health(request),
            ["v1", "observation"] => self.observation(request),
            ["v1", "observation", id] => self.stored(request, id),
            ["v1", "changes"] => self.changes(request),
            ["v1", "elements", id] => self.element(request, id),
            _ => Err(Response::error(404, "not_found", "no such endpoint".to_owned())),
        };
        result.unwrap_or_else(|response| response)
    }

    /// `GET /v1/health`: the service is up. Does not touch the screen.
    fn health(&self, request: &Request) -> Result<Response, Response> {
        allow(request, &[])?;
        Ok(Response::ok(json!({
            "status": "ok",
            "version": env!("CARGO_PKG_VERSION"),
            "protocol_version": PROTOCOL_VERSION,
            "uptime_ms": u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "observations": lock(&self.history).len(),
        })))
    }

    /// `GET /v1/observation[?app=|pid=][&sources=]`: observes now.
    fn observation(&self, request: &Request) -> Result<Response, Response> {
        allow(request, &["app", "pid", "sources"])?;
        let entry = self
            .worker
            .observe(ObserveRequest { target: target(request)?, sources: sources(request)? })?;
        Ok(observed(json(entry.observation())?, &entry))
    }

    /// `GET /v1/observation/{id}`: an observation from the history.
    fn stored(&self, request: &Request, id: &str) -> Result<Response, Response> {
        allow(request, &[])?;
        Ok(Response::ok(json(self.entry(id)?.observation())?))
    }

    /// `GET /v1/changes?since={id}`: observes the application of `since`
    /// again, with the same sources, and answers what changed since then.
    fn changes(&self, request: &Request) -> Result<Response, Response> {
        allow(request, &["since"])?;
        let since = request.param("since")?.ok_or_else(|| {
            Response::error(
                400,
                "bad_request",
                "`since` (an observation ID) is required".to_owned(),
            )
        })?;
        let from = self.entry(since)?;
        let to = self.worker.observe(from.request.clone())?;
        if !lock(&self.history).continues(since, to.observation()) {
            // Element IDs of `to` mean nothing relative to `since`.
            let mut response = Response::error(
                409,
                "session_changed",
                "the new observation starts a new tracking session; read it in full".to_owned(),
            );
            response.body["error"]["observation"] = json!(to.observation().id);
            return Ok(response);
        }
        let delta = ObservationDelta::between(from.observation(), to.observation());
        Ok(observed(json(&delta)?, &to))
    }

    /// `GET /v1/elements/{id}[?observation=]`: an element of the latest (or
    /// the given) observation, with the evidence behind it.
    fn element(&self, request: &Request, id: &str) -> Result<Response, Response> {
        allow(request, &["observation"])?;
        let entry = match request.param("observation")? {
            Some(observation) => self.entry(observation)?,
            None => lock(&self.history).latest().ok_or_else(|| {
                Response::error(
                    404,
                    "observation_not_found",
                    "nothing has been observed yet".to_owned(),
                )
            })?,
        };
        let (element, evidence) = entry.inspection.element(id).ok_or_else(|| {
            Response::error(
                404,
                "element_not_found",
                format!("observation `{}` has no element `{id}`", entry.observation().id),
            )
        })?;
        Ok(Response::ok(json!({
            "observation": entry.observation().id,
            "element": element,
            "evidence": evidence,
        })))
    }

    fn entry(&self, id: &str) -> Result<Arc<Entry>, Response> {
        lock(&self.history).get(id).ok_or_else(|| {
            Response::error(
                404,
                "observation_not_found",
                format!("observation `{id}` is not in the service's history"),
            )
        })
    }
}

/// Refuses query parameters other than `allowed`: a misspelled parameter
/// must not silently observe something else.
fn allow(request: &Request, allowed: &[&str]) -> Result<(), Response> {
    match request.query.iter().find(|(key, _)| !allowed.contains(&key.as_str())) {
        Some((key, _)) => {
            Err(Response::error(400, "bad_request", format!("unknown query parameter `{key}`")))
        }
        None => Ok(()),
    }
}

fn target(request: &Request) -> Result<AppTarget, Response> {
    let bad = |message: &str| Response::error(400, "bad_request", message.to_owned());
    match (request.param("app")?, request.param("pid")?) {
        (Some(_), Some(_)) => Err(bad("`app` and `pid` cannot be used together")),
        (Some(""), None) => Err(bad("`app` is empty")),
        (Some(name), None) => Ok(AppTarget::Name(name.to_owned())),
        (None, Some(pid)) => pid.parse().map(AppTarget::Pid).map_err(|_| bad("`pid` is invalid")),
        (None, None) => Ok(AppTarget::Frontmost),
    }
}

fn sources(request: &Request) -> Result<Vec<Source>, Response> {
    let Some(list) = request.param("sources")? else {
        return Ok(vec![Source::Accessibility, Source::Ocr, Source::Vision]);
    };
    list.split(',')
        .map(|name| match name.trim() {
            "accessibility" => Ok(Source::Accessibility),
            "ocr" => Ok(Source::Ocr),
            "vision" => Ok(Source::Vision),
            other => Err(Response::error(
                400,
                "bad_request",
                format!("unknown source `{other}` (accessibility, ocr, vision)"),
            )),
        })
        .collect()
}

fn json(value: &impl serde::Serialize) -> Result<serde_json::Value, Response> {
    serde_json::to_value(value)
        .map_err(|_| Response::error(500, "internal", "serialization failed".to_owned()))
}

/// A response about a new observation, with its timings.
fn observed(body: serde_json::Value, entry: &Entry) -> Response {
    Response::ok(body).with_header("Server-Timing", server_timing(&entry.inspection.timings))
}

/// Timings in the standard `Server-Timing` form.
fn server_timing(timings: &Timings) -> String {
    [
        ("accessibility", timings.accessibility_ms),
        ("capture", timings.capture_ms),
        ("ocr", timings.ocr_ms),
        ("vision", timings.vision_ms),
        ("fusion", timings.fusion_ms),
        ("tracking", timings.tracking_ms),
        ("total", timings.total_ms),
    ]
    .iter()
    .map(|(name, ms)| format!("{name};dur={ms}"))
    .collect::<Vec<_>>()
    .join(", ")
}

/// The answer to a failed observation.
pub(crate) fn failure(error: &CoreError) -> Response {
    let status = match error.code() {
        "no_sources" => 400,
        "permission_denied" => 403,
        "not_found" => 404,
        "window_mismatch" => 409,
        "unsupported" => 501,
        "timeout" => 504,
        _ => 500,
    };
    Response::error(status, error.code(), error.to_string())
}
