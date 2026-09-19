//! The `/v1` API: routing, parameters and answers.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use argus_core::accessibility::AppTarget;
use argus_core::agent::{self, AgentOptions};
use argus_core::{Error as CoreError, Timings};
use argus_protocol::{Bounds, ObservationDelta, PROTOCOL_VERSION, Source};
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
            ["v1", "observation", id, "frame"] => self.frame(request, id),
            ["v1", "agent", "observation"] => self.agent_observation(request),
            ["v1", "agent", "changes"] => self.agent_changes(request),
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

    /// `GET /v1/observation/{id}/frame?x=&y=&width=&height=[&scale=]`: the
    /// pixels of a region of a recent observation, as PNG. Only the latest
    /// observation of each session keeps its frames.
    fn frame(&self, request: &Request, id: &str) -> Result<Response, Response> {
        allow(request, &["x", "y", "width", "height", "scale"])?;
        let entry = self.entry(id)?;
        let number = |name: &str, default: Option<f32>| -> Result<f32, Response> {
            match request.param(name)? {
                Some(text) => text.parse::<f32>().ok().filter(|v| v.is_finite()).ok_or_else(|| {
                    Response::error(400, "bad_request", format!("`{name}` is not a number"))
                }),
                None => default.ok_or_else(|| {
                    Response::error(400, "bad_request", format!("`{name}` is required"))
                }),
            }
        };
        let region = Bounds::new(
            number("x", None)?,
            number("y", None)?,
            number("width", None)?,
            number("height", None)?,
        )
        .map_err(|_| Response::error(400, "bad_request", "invalid region".to_owned()))?;
        let scale = number("scale", Some(1.0))?.clamp(0.1, 2.0);
        let frames = lock(&entry.frames);
        if frames.is_empty() {
            return Err(Response::error(
                404,
                "frame_not_available",
                "the frames of this observation are no longer kept (or it had no pixel sources)"
                    .to_owned(),
            ));
        }
        argus_core::crop::crop_png(&frames, region, scale)
            .map(|png| Response::bytes("image/png", png))
            .ok_or_else(|| {
                Response::error(404, "frame_not_available", "no frame covers the region".to_owned())
            })
    }

    /// `GET /v1/agent/observation[?app=|pid=][&sources=][&mode=][&min_confidence=]`:
    /// observes now and answers the agent view: compact text and, in
    /// `hybrid` mode, the regions to look at as images.
    fn agent_observation(&self, request: &Request) -> Result<Response, Response> {
        allow(request, &["app", "pid", "sources", "mode", "min_confidence"])?;
        let (hybrid, options) = agent_options(request)?;
        let entry = self
            .worker
            .observe(ObserveRequest { target: target(request)?, sources: sources(request)? })?;
        let observation = entry.observation();
        let text = agent::render_observation(observation, &options);
        let regions = if hybrid { regions(&entry, None) } else { Vec::new() };
        Ok(observed(
            json!({ "observation": observation.id, "text": text, "regions": regions }),
            &entry,
        ))
    }

    /// `GET /v1/agent/changes?since={id}[&mode=][&min_confidence=]`: what
    /// changed, as agent text; the whole view when a new session started.
    fn agent_changes(&self, request: &Request) -> Result<Response, Response> {
        allow(request, &["since", "mode", "min_confidence"])?;
        let (hybrid, options) = agent_options(request)?;
        let since = request.param("since")?.ok_or_else(|| {
            Response::error(
                400,
                "bad_request",
                "`since` (an observation ID) is required".to_owned(),
            )
        })?;
        let from = self.entry(since)?;
        let to = self.worker.observe(from.request.clone())?;
        let observation = to.observation();
        if !lock(&self.history).continues(since, observation) {
            let text = agent::render_observation(observation, &options);
            let regions = if hybrid { regions(&to, None) } else { Vec::new() };
            return Ok(observed(
                json!({
                    "from": since,
                    "observation": observation.id,
                    "new_session": true,
                    "text": text,
                    "regions": regions,
                }),
                &to,
            ));
        }
        let delta = ObservationDelta::between(from.observation(), observation);
        let text = agent::render_delta(&delta, from.observation(), observation, &options);
        let regions = if hybrid { regions(&to, Some(&delta)) } else { Vec::new() };
        Ok(observed(
            json!({ "from": since, "observation": observation.id, "text": text, "regions": regions }),
            &to,
        ))
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

/// `mode` (`text` or `hybrid`) and `min_confidence` of the agent view.
fn agent_options(request: &Request) -> Result<(bool, AgentOptions), Response> {
    let hybrid = match request.param("mode")? {
        None | Some("text") => false,
        Some("hybrid") => true,
        Some(other) => {
            return Err(Response::error(
                400,
                "bad_request",
                format!("unknown mode `{other}` (text, hybrid)"),
            ));
        }
    };
    let mut options = AgentOptions::default();
    if let Some(value) = request.param("min_confidence")? {
        options.min_confidence = value
            .parse::<f32>()
            .ok()
            .filter(|value| (0.0..=1.0).contains(value))
            .ok_or_else(|| {
                Response::error(400, "bad_request", "`min_confidence` must be in 0..=1".to_owned())
            })?;
    }
    Ok((hybrid, options))
}

/// The weak regions of an observation, with the URL of each one's image.
/// After a delta, only the regions touched by the changes.
fn regions(entry: &Entry, delta: Option<&ObservationDelta>) -> Vec<serde_json::Value> {
    if lock(&entry.frames).is_empty() {
        return Vec::new();
    }
    let observation = entry.observation();
    let touched = |bounds: &Bounds| -> bool {
        let Some(delta) = delta else { return true };
        let changed = delta.changed.iter().map(|change| &change.id).chain(delta.removed.iter());
        let mut areas: Vec<Bounds> = delta.added.iter().map(|element| element.bounds).collect();
        for id in changed {
            if let Some(element) = observation.elements.iter().find(|element| &element.id == id) {
                areas.push(element.bounds);
            }
        }
        areas.iter().any(|area| area.intersection(bounds).is_some())
    };
    agent::weak_regions(observation)
        .into_iter()
        .filter(|region| touched(&region.bounds))
        .map(|region| {
            let b = region.bounds;
            json!({
                "bounds": b,
                "reason": region.reason,
                "image": format!(
                    "/v1/observation/{}/frame?x={:.0}&y={:.0}&width={:.0}&height={:.0}",
                    observation.id, b.x(), b.y(), b.width(), b.height()
                ),
            })
        })
        .collect()
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
