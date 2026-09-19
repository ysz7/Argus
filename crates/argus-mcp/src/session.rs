//! The state behind the tools: the observed application, the latest
//! observation and its frames, the region images the agent has seen, and
//! the guarded execution of actions.

use std::thread::sleep;
use std::time::{Duration, Instant};

use argus_core::accessibility::AppTarget;
use argus_core::agent::{self, AgentOptions};
use argus_core::capture::{self, CaptureBackend, CaptureTarget, WindowInfo};
use argus_core::crop::{self, Crop};
use argus_core::{Inspection, Observer};
use argus_input::{Button, Input, Modifiers, Point};
use argus_protocol::{Application, Bounds, Element, Frame, Observation, ObservationDelta, Source};
use serde_json::{Map, Value};

use crate::guard::{MAX_TEXT, Policy};

/// Pause between an action and observing its effect.
const SETTLE: Duration = Duration::from_millis(450);
/// Largest side of an image sent to the agent, in pixels (models downscale
/// larger images, which would shift image coordinates).
const MAX_IMAGE_SIDE: f32 = 1400.0;
/// Most region images in one answer.
const MAX_IMAGES: usize = 3;
/// Share of pixels that must change for a region image to be sent again.
const CHANGED: f32 = 0.004;
/// Windows above this layer (the Dock, the menu bar, overlays) are not
/// application content.
const MAX_CONTENT_LAYER: i64 = 101;

/// One piece of a tool answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    /// Text.
    Text(String),
    /// A PNG image.
    Png(Vec<u8>),
}

/// A tool answer; an error answer is shown to the agent as a failed call.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Answer {
    /// Text and images, in order.
    pub content: Vec<Content>,
    /// Whether the call failed.
    pub is_error: bool,
}

impl Answer {
    fn text(text: impl Into<String>) -> Self {
        Self { content: vec![Content::Text(text.into())], is_error: false }
    }

    pub(crate) fn error(text: impl Into<String>) -> Self {
        Self { content: vec![Content::Text(text.into())], is_error: true }
    }
}

type Refusal = String;

/// A region image the agent has seen.
#[derive(Debug)]
struct Shown {
    label: String,
    bounds: Bounds,
    /// Screen points per image pixel.
    points_per_pixel: f32,
    image: Crop,
}

/// The observed application.
#[derive(Debug, Clone)]
struct Target {
    request: AppTarget,
    application: Application,
}

/// Everything the server knows between calls.
pub struct Session {
    policy: Policy,
    observer: Option<Observer>,
    capture: Option<Box<dyn CaptureBackend>>,
    input: Option<Input>,
    target: Option<Target>,
    latest: Option<Inspection>,
    frames: Vec<Frame>,
    shown: Vec<Shown>,
    labels: u32,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Session")
            .field("policy", &self.policy)
            .field("target", &self.target)
            .field("latest", &self.latest.as_ref().map(|latest| &latest.observation.id))
            .field("shown", &self.shown.len())
            .finish_non_exhaustive()
    }
}

const SOURCES: [Source; 3] = [Source::Accessibility, Source::Ocr, Source::Vision];

fn text_arg<'a>(arguments: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    arguments.get(name).and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty())
}

fn number_arg(arguments: &Map<String, Value>, name: &str) -> Option<f64> {
    arguments.get(name).and_then(Value::as_f64).filter(|value| value.is_finite())
}

fn center(bounds: Bounds) -> Point {
    Point::new(
        f64::from(bounds.x() + bounds.width() / 2.0),
        f64::from(bounds.y() + bounds.height() / 2.0),
    )
}

fn contains(bounds: &Bounds, point: Point) -> bool {
    let (x, y) = (point.x as f32, point.y as f32);
    x >= bounds.x()
        && x < bounds.x() + bounds.width()
        && y >= bounds.y()
        && y < bounds.y() + bounds.height()
}

fn overlap(a: &Bounds, b: &Bounds) -> f32 {
    let area = |bounds: &Bounds| bounds.width() * bounds.height();
    let Some(common) = a.intersection(b) else { return 0.0 };
    area(&common) / (area(a) + area(b) - area(&common)).max(1.0)
}

fn app_label(application: &Application) -> String {
    application
        .name
        .clone()
        .or_else(|| application.bundle_id.clone())
        .unwrap_or_else(|| "the application".to_owned())
}

impl Session {
    /// A session under `policy`. Platform backends start on first use.
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            observer: None,
            capture: None,
            input: None,
            target: None,
            latest: None,
            frames: Vec::new(),
            shown: Vec::new(),
            labels: 0,
        }
    }

    /// Runs tool `name`.
    pub fn call(&mut self, name: &str, arguments: &Map<String, Value>) -> Answer {
        let started = Instant::now();
        let result = match name {
            "list_apps" => self.list_apps(),
            "observe" => self.observe(arguments),
            "screenshot" => self.screenshot(),
            "click" | "type_text" | "press_keys" | "scroll" | "drag" => self.act(name, arguments),
            other => Err(format!("unknown tool `{other}`")),
        };
        let mut answer = result.unwrap_or_else(Answer::error);
        let elapsed = started.elapsed().as_millis();
        if let Some(Content::Text(text)) = answer.content.first_mut() {
            text.push_str(&format!("\n(Argus: {elapsed} ms)"));
        }
        answer
    }

    // --- platform ---------------------------------------------------------

    fn capture(&mut self) -> Result<&dyn CaptureBackend, Refusal> {
        if self.capture.is_none() {
            self.capture = Some(capture::default_backend().map_err(|error| error.to_string())?);
        }
        Ok(self.capture.as_deref().expect("created above"))
    }

    fn observer(&mut self) -> Result<&Observer, Refusal> {
        if self.observer.is_none() {
            let observer = Observer::new().map_err(|error| error.to_string())?;
            self.observer = Some(observer.with_incremental(true));
        }
        Ok(self.observer.as_ref().expect("created above"))
    }

    fn input(&mut self) -> Result<&Input, Refusal> {
        if self.input.is_none() {
            let input = Input::new().map_err(|error| error.to_string())?;
            if !input.has_permission() {
                return Err(
                    "macOS denies input events: grant the Accessibility permission to the \
                            app that runs Argus (System Settings → Privacy & Security → \
                            Accessibility), then restart it"
                        .to_owned(),
                );
            }
            self.input = Some(input);
        }
        Ok(self.input.as_ref().expect("created above"))
    }

    // --- tools: perception --------------------------------------------------

    fn list_apps(&mut self) -> Result<Answer, Refusal> {
        let windows = self.capture()?.windows().map_err(|error| error.to_string())?;
        let mut names: Vec<String> = Vec::new();
        for window in windows.iter().filter(|window| window.layer == 0) {
            let application = &window.application;
            let mut name = app_label(application);
            if self.policy.refuse_app(application).is_some() {
                name.push_str(" (blocked)");
            }
            if !names.contains(&name) {
                names.push(name);
            }
        }
        if names.is_empty() {
            return Ok(Answer::text("No application has a window on screen."));
        }
        Ok(Answer::text(format!("Applications with windows, front to back:\n{}", names.join("\n"))))
    }

    fn observe(&mut self, arguments: &Map<String, Value>) -> Result<Answer, Refusal> {
        let request = match text_arg(arguments, "app") {
            Some(name) => {
                let named = Application { name: Some(name.to_owned()), bundle_id: None, pid: None };
                if let Some(refusal) = self.policy.refuse_app(&named) {
                    return Err(refusal);
                }
                AppTarget::Name(name.to_owned())
            }
            None => match &self.target {
                Some(target) => target.request.clone(),
                None => return Err("pass `app` (see `list_apps`) to choose what to observe".into()),
            },
        };
        let same = self.target.as_ref().is_some_and(|target| target.request == request);
        if !same {
            // Another application: fresh ids, fresh images.
            if let Some(observer) = &self.observer {
                observer.reset_tracking();
            }
            self.forget();
        }
        let inspection = self.inspect(&request)?;
        let observation = &inspection.observation;
        let mut text = format!(
            "{} — observation {}\n{}",
            observation.application.as_ref().map(app_label).unwrap_or_default(),
            observation.id,
            agent::render_observation(observation, &AgentOptions::default())
        );
        self.shown.clear();
        let images = self.region_images(observation, &mut text, false);
        self.latest = Some(inspection);
        let mut answer = Answer::text(text);
        answer.content.extend(images);
        Ok(answer)
    }

    /// Observes `request`, checking the policy on the application found.
    fn inspect(&mut self, request: &AppTarget) -> Result<Inspection, Refusal> {
        let mut inspection = self
            .observer()?
            .inspect(request, &SOURCES)
            .map_err(|error| format!("Argus could not observe the application: {error}"))?;
        let application = inspection.observation.application.clone().unwrap_or_default();
        if let Some(refusal) = self.policy.refuse_app(&application) {
            self.forget();
            return Err(refusal);
        }
        self.frames = std::mem::take(&mut inspection.frames);
        self.target = Some(Target { request: request.clone(), application });
        Ok(inspection)
    }

    fn forget(&mut self) {
        self.target = None;
        self.latest = None;
        self.frames.clear();
        self.shown.clear();
        self.labels = 0;
    }

    /// Images of the weak regions of `observation`. After an action
    /// (`changes`), only regions whose pixels differ from the image the
    /// agent already has.
    fn region_images(
        &mut self,
        observation: &Observation,
        text: &mut String,
        changes: bool,
    ) -> Vec<Content> {
        let mut images = Vec::new();
        for region in agent::weak_regions(observation) {
            if images.len() == MAX_IMAGES {
                break;
            }
            let bounds = region.bounds;
            let scale = (MAX_IMAGE_SIDE / bounds.width().max(bounds.height())).min(1.0);
            let Some(image) = crop::crop(&self.frames, bounds, scale) else { continue };
            let earlier = self.shown.iter().position(|shown| overlap(&shown.bounds, &bounds) > 0.8);
            if changes {
                if let Some(index) = earlier {
                    if !self.shown[index].image.differs(&image, CHANGED) {
                        continue;
                    }
                }
            }
            let Some(png) = image.png() else { continue };
            let label = match earlier {
                Some(index) => self.shown.remove(index).label,
                None => {
                    self.labels += 1;
                    format!("R{}", self.labels)
                }
            };
            let reason = serde_json::to_value(region.reason)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default()
                .replace('_', " ");
            text.push_str(&format!(
                "\nImage {label}: region ({:.0},{:.0} {:.0}x{:.0}), {reason}{}",
                bounds.x(),
                bounds.y(),
                bounds.width(),
                bounds.height(),
                if changes { ", changed" } else { "" }
            ));
            self.shown.push(Shown {
                label,
                bounds,
                points_per_pixel: bounds.width() / image.width.max(1) as f32,
                image,
            });
            images.push(Content::Png(png));
        }
        images
    }

    fn screenshot(&mut self) -> Result<Answer, Refusal> {
        let Some(target) = self.target.clone() else {
            return Err("observe an application first".into());
        };
        let window = self.front_window(&target)?;
        let frame = self
            .capture()?
            .capture(CaptureTarget::Window(window.id))
            .map_err(|error| format!("the window could not be captured: {error}"))?;
        let bounds = frame.bounds();
        let scale = (MAX_IMAGE_SIDE / bounds.width().max(bounds.height())).min(1.0);
        let image = crop::crop(std::slice::from_ref(&frame), bounds, scale)
            .and_then(|image| image.png())
            .ok_or("the window could not be encoded")?;
        let mut answer = Answer::text(format!(
            "Window of {} at ({:.0},{:.0} {:.0}x{:.0}); one image pixel is {:.2} screen points.",
            app_label(&target.application),
            bounds.x(),
            bounds.y(),
            bounds.width(),
            bounds.height(),
            1.0 / scale
        ));
        answer.content.push(Content::Png(image));
        Ok(answer)
    }

    /// The front window of the target (dialogs and sheets included).
    fn front_window(&mut self, target: &Target) -> Result<WindowInfo, Refusal> {
        let pid = target.application.pid;
        self.capture()?
            .windows()
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|window| {
                window.application.pid == pid
                    && (0..=8).contains(&window.layer)
                    && window.bounds.width() >= 60.0
                    && window.bounds.height() >= 40.0
            })
            .ok_or_else(|| format!("{} has no window on screen", app_label(&target.application)))
    }

    // --- tools: actions -----------------------------------------------------

    fn act(&mut self, name: &str, arguments: &Map<String, Value>) -> Result<Answer, Refusal> {
        if let Some(refusal) = self.policy.refuse_actions() {
            return Err(refusal);
        }
        let Some(target) = self.target.clone() else {
            return Err("observe an application first (`observe` with `app`)".into());
        };
        let done = match name {
            "click" => {
                let point = self.point(arguments, "")?;
                let button = match text_arg(arguments, "button") {
                    None | Some("left") => Button::Left,
                    Some("right") => Button::Right,
                    Some("middle") => Button::Middle,
                    Some(other) => return Err(format!("unknown button `{other}`")),
                };
                let count = number_arg(arguments, "count").unwrap_or(1.0).clamp(1.0, 3.0) as u32;
                let mut modifiers = Modifiers::default();
                for modifier in
                    arguments.get("modifiers").and_then(Value::as_array).into_iter().flatten()
                {
                    if !modifier.as_str().is_some_and(|name| modifiers.add(name)) {
                        return Err(format!("unknown modifier {modifier}"));
                    }
                }
                self.prepare(&target, Some(point))?;
                self.input()?.click(point, button, count, modifiers).map_err(|e| e.to_string())?;
                format!("clicked at ({:.0},{:.0})", point.x, point.y)
            }
            "type_text" => {
                let text = arguments.get("text").and_then(Value::as_str).unwrap_or_default();
                if text.chars().count() > MAX_TEXT {
                    return Err(format!("text longer than {MAX_TEXT} characters"));
                }
                let field = match text_arg(arguments, "element_id") {
                    Some(_) => Some(self.point(arguments, "")?),
                    None => None,
                };
                self.prepare(&target, field)?;
                let input = self.input()?;
                if let Some(point) = field {
                    input
                        .click(point, Button::Left, 1, Modifiers::default())
                        .map_err(|e| e.to_string())?;
                    sleep(Duration::from_millis(120));
                }
                if arguments.get("clear").and_then(Value::as_bool) == Some(true) {
                    let all = argus_input::parse_combo("cmd+a").map_err(|e| e.to_string())?;
                    input.press(&all, 1).map_err(|e| e.to_string())?;
                }
                input.type_text(text).map_err(|e| e.to_string())?;
                format!("typed {} characters", text.chars().count())
            }
            "press_keys" => {
                let keys = text_arg(arguments, "keys").ok_or("`keys` is required")?;
                let combo = argus_input::parse_combo(keys).map_err(|e| e.to_string())?;
                if let Some(refusal) = self.policy.refuse_keys(&combo) {
                    return Err(refusal);
                }
                let repeat = number_arg(arguments, "repeat").unwrap_or(1.0).clamp(1.0, 50.0) as u32;
                self.prepare(&target, None)?;
                self.input()?.press(&combo, repeat).map_err(|e| e.to_string())?;
                format!(
                    "pressed {}{}",
                    combo.canonical(),
                    if repeat > 1 { format!(" ×{repeat}") } else { String::new() }
                )
            }
            "scroll" => {
                let point = match self.point(arguments, "") {
                    Ok(point) => point,
                    Err(_) if !has_target(arguments, "") => {
                        center(self.front_window(&target)?.bounds)
                    }
                    Err(refusal) => return Err(refusal),
                };
                let amount = number_arg(arguments, "amount").unwrap_or(5.0).clamp(1.0, 30.0) as i32;
                let (dx, dy) = match text_arg(arguments, "direction") {
                    Some("up") => (0, amount),
                    Some("down") => (0, -amount),
                    Some("left") => (amount, 0),
                    Some("right") => (-amount, 0),
                    _ => return Err("`direction` must be up, down, left or right".into()),
                };
                self.prepare(&target, Some(point))?;
                self.input()?.scroll(point, dx, dy).map_err(|e| e.to_string())?;
                format!("scrolled {} by {amount}", text_arg(arguments, "direction").unwrap_or(""))
            }
            "drag" => {
                let from = self.point(arguments, "from_")?;
                let to = self.point(arguments, "to_")?;
                self.prepare(&target, Some(from))?;
                self.check_point(&target, to)?;
                self.input()?.drag(from, to).map_err(|e| e.to_string())?;
                format!("dragged from ({:.0},{:.0}) to ({:.0},{:.0})", from.x, from.y, to.x, to.y)
            }
            other => return Err(format!("unknown tool `{other}`")),
        };
        sleep(SETTLE);
        self.changes(&target, &format!("Done: {done}."))
    }

    /// Observes again after an action and answers what changed.
    fn changes(&mut self, target: &Target, done: &str) -> Result<Answer, Refusal> {
        let before = self.latest.take();
        let inspection = match self.inspect(&target.request) {
            Ok(inspection) => inspection,
            Err(refusal) => {
                return Ok(Answer::text(format!(
                    "{done}\nArgus could not observe afterwards: {refusal}"
                )));
            }
        };
        let observation = &inspection.observation;
        let options = AgentOptions::default();
        let continues = before
            .as_ref()
            .is_some_and(|before| observation.previous.as_ref() == Some(&before.observation.id));
        let mut text = match (&before, continues) {
            (Some(before), true) => {
                let delta = ObservationDelta::between(&before.observation, observation);
                format!(
                    "{done}\nChanges (observation {}):\n{}",
                    observation.id,
                    agent::render_delta(&delta, &before.observation, observation, &options)
                )
            }
            _ => {
                self.shown.clear();
                format!(
                    "{done}\nThe window changed completely (observation {}):\n{}",
                    observation.id,
                    agent::render_observation(observation, &options)
                )
            }
        };
        let images = self.region_images(observation, &mut text, continues);
        self.latest = Some(inspection);
        let mut answer = Answer::text(text);
        answer.content.extend(images);
        Ok(answer)
    }

    /// The screen point an action targets: an element (checked against
    /// `expect`), a point in a region image, or a screen point.
    fn point(&self, arguments: &Map<String, Value>, prefix: &str) -> Result<Point, Refusal> {
        let arg = |name: &str| format!("{prefix}{name}");
        let (x, y) = (number_arg(arguments, &arg("x")), number_arg(arguments, &arg("y")));
        if let Some(id) = text_arg(arguments, &arg("element_id")) {
            let latest = self.latest.as_ref().ok_or("observe first")?;
            let observation = &latest.observation;
            let element: &Element = observation
                .elements
                .iter()
                .find(|element| element.id.as_str() == id)
                .ok_or_else(|| {
                    format!("there is no element {id} in the latest observation; nothing was done")
                })?;
            if let Some(expect) = text_arg(arguments, &arg("expect")) {
                if !agent::expect_matches(observation, element, expect) {
                    let what =
                        element.name.as_deref().or(element.value.as_deref()).unwrap_or("unnamed");
                    return Err(format!(
                        "{id} is {:?} ({}), not {expect:?}; nothing was done",
                        what,
                        serde_json::to_value(element.role)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_default()
                    ));
                }
            }
            return Ok(center(element.visible_bounds.unwrap_or(element.bounds)));
        }
        if let Some(label) = text_arg(arguments, &arg("image")) {
            let shown = self
                .shown
                .iter()
                .find(|shown| shown.label.eq_ignore_ascii_case(label))
                .ok_or_else(|| format!("no region image {label} is current; observe again"))?;
            let (Some(x), Some(y)) = (x, y) else {
                return Err(format!("give x and y in image {label}"));
            };
            let scale = f64::from(shown.points_per_pixel);
            return Ok(Point::new(
                f64::from(shown.bounds.x()) + x * scale,
                f64::from(shown.bounds.y()) + y * scale,
            ));
        }
        match (x, y) {
            (Some(x), Some(y)) => Ok(Point::new(x, y)),
            _ => Err(format!(
                "give {0}element_id (with {0}expect), {0}image with {0}x and {0}y, or screen {0}x and {0}y",
                prefix
            )),
        }
    }

    /// Brings the target to the front and checks that `point`, if any, is
    /// on one of its windows.
    fn prepare(&mut self, target: &Target, point: Option<Point>) -> Result<(), Refusal> {
        let pid = target.application.pid.ok_or("the application has no process id")?;
        let frontmost = |session: &mut Self| -> Result<Option<u32>, Refusal> {
            Ok(session
                .capture()?
                .frontmost_application()
                .map_err(|error| error.to_string())?
                .and_then(|application| application.pid))
        };
        if frontmost(self)? != Some(pid) {
            self.input()?.activate(pid);
            let deadline = Instant::now() + Duration::from_millis(1500);
            while frontmost(self)? != Some(pid) {
                if Instant::now() > deadline {
                    return Err(format!(
                        "{} could not be brought to the front; nothing was done",
                        app_label(&target.application)
                    ));
                }
                sleep(Duration::from_millis(100));
            }
            sleep(Duration::from_millis(150));
        }
        if let Some(point) = point {
            self.check_point(target, point)?;
        }
        Ok(())
    }

    /// Refuses points that are not on the target's own windows (another
    /// window above, or outside every window).
    fn check_point(&mut self, target: &Target, point: Point) -> Result<(), Refusal> {
        let pid = target.application.pid;
        let windows = self.capture()?.windows().map_err(|error| error.to_string())?;
        let top = windows.iter().find(|window| {
            window.layer <= MAX_CONTENT_LAYER
                && (window.application.pid == pid || window.layer < 20)
                && contains(&window.bounds, point)
        });
        match top {
            Some(window) if window.application.pid == pid => Ok(()),
            Some(window) => Err(format!(
                "({:.0},{:.0}) is covered by a window of {}; nothing was done",
                point.x,
                point.y,
                app_label(&window.application)
            )),
            None => Err(format!(
                "({:.0},{:.0}) is outside the windows of {}; nothing was done",
                point.x,
                point.y,
                app_label(&target.application)
            )),
        }
    }
}

/// Whether the arguments name any target (element, image or point).
fn has_target(arguments: &Map<String, Value>, prefix: &str) -> bool {
    ["element_id", "image", "x", "y"]
        .iter()
        .any(|name| arguments.contains_key(&format!("{prefix}{name}")))
}
