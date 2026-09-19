//! `argus watch`: observe repeatedly and show what changed.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use argus_core::Observer;
use argus_core::tracking::UNCERTAIN;
use argus_protocol::{Element, ElementChange, Observation, ObservationDelta, Relation, Window};
use serde_json::Value;

use crate::commands::observe::{SourceArgs, ensure_valid};
use crate::commands::target::AppTargetArgs;
use crate::output::{print_json_line, print_text};

#[derive(Debug, clap::Args)]
pub(crate) struct WatchArgs {
    #[command(flatten)]
    sources: SourceArgs,

    #[command(flatten)]
    target: AppTargetArgs,

    /// Time between the starts of successive observations, in milliseconds.
    #[arg(long, value_name = "MS", default_value_t = 1000)]
    interval_ms: u64,

    /// Stop after this many observations. Runs until interrupted when
    /// omitted.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    count: Option<u32>,

    /// Print JSON Lines instead of text: the first observation of a session,
    /// then one delta per observation (empty deltas included).
    #[arg(long)]
    json: bool,

    /// Also show changes of confidence, sources and children, and empty
    /// deltas.
    #[arg(long)]
    verbose: bool,
}

pub(crate) fn run(args: &WatchArgs) -> anyhow::Result<()> {
    let observer = Observer::new()?;
    let (target, sources) = (args.target.target(), args.sources.sources());
    let interval = Duration::from_millis(args.interval_ms);
    let mut previous: Option<Observation> = None;
    let mut number = 0;
    loop {
        let started = Instant::now();
        let observation = observer.observe(&target, &sources)?;
        ensure_valid(&observation)?;
        let delta =
            previous.as_ref().and_then(|previous| argus_core::delta(previous, &observation));
        match (&delta, args.json) {
            (Some(delta), true) => print_json_line(&serde_json::to_value(delta)?)?,
            (None, true) => print_json_line(&serde_json::to_value(&observation)?)?,
            (Some(delta), false) => {
                let from = previous.as_ref().expect("a delta has a base");
                print_text(&render_delta(from, &observation, delta, args.verbose))?;
            }
            (None, false) => print_text(&render_observation(&observation))?,
        }
        previous = Some(observation);

        number += 1;
        if args.count.is_some_and(|count| number >= count) {
            return Ok(());
        }
        std::thread::sleep(interval.saturating_sub(started.elapsed()));
    }
}

/// The first observation of a session: a header and every element.
fn render_observation(observation: &Observation) -> String {
    let mut out = String::new();
    let application =
        observation.application.as_ref().and_then(|a| a.name.as_deref()).unwrap_or("unknown");
    let title = observation.window.as_ref().and_then(|w| w.title.as_deref());
    let title = title.map(|title| format!(" {title:?}")).unwrap_or_default();
    let count = observation.elements.len();
    let _ = writeln!(out, "{}  {application}{title}: {count} elements", observation.id.as_str());
    for element in &observation.elements {
        let _ = writeln!(out, "+ {}", describe(element));
    }
    out
}

/// Properties hidden unless `--verbose`: evidence bookkeeping rather than
/// interface state, and `children` (every child also reports its `parent`).
fn is_detail(property: &str) -> bool {
    property.starts_with("confidence.") || property == "sources" || property == "children"
}

/// A delta in the developer format of the roadmap:
///
/// ```text
/// obs_mu7z4911_000002  +1 -0 ~1
/// + e_19 dialog "Saved"
/// ~ e_17 enabled true → false
/// ```
fn render_delta(
    from: &Observation,
    to: &Observation,
    delta: &ObservationDelta,
    verbose: bool,
) -> String {
    // Screen coordinates of everything change when the window moves; unless
    // verbose, such changes are only counted.
    let offset = window_offset(from.window.as_ref(), delta.window.as_ref());
    let with_window = |change: &ElementChange| {
        offset.is_some_and(|offset| {
            matches!(change.property.as_str(), "bounds" | "visible_bounds")
                && moved_by(&change.from, &change.to, offset)
        })
    };
    let changes: Vec<&ElementChange> = delta
        .changed
        .iter()
        .filter(|change| verbose || !(is_detail(&change.property) || with_window(change)))
        .collect();
    let mut moved: Vec<&str> = delta
        .changed
        .iter()
        .filter(|change| !verbose && with_window(change))
        .map(|change| change.id.as_str())
        .collect();
    moved.dedup();
    let mut changed_ids: Vec<&str> = changes.iter().map(|change| change.id.as_str()).collect();
    changed_ids.dedup();
    let relations = delta.added_relations.len() + delta.removed_relations.len();
    if !verbose
        && delta.window.is_none()
        && delta.added.is_empty()
        && delta.removed.is_empty()
        && changes.is_empty()
        && relations == 0
    {
        return String::new();
    }

    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}  +{} -{} ~{}",
        delta.to.as_str(),
        delta.added.len(),
        delta.removed.len(),
        changed_ids.len()
    );
    if let Some(window) = &delta.window {
        let before = describe_window(from.window.as_ref());
        let _ = writeln!(out, "~ window {before} → {}", describe_window(Some(window)));
        if !moved.is_empty() {
            let _ = writeln!(out, "  ({} elements moved with the window)", moved.len());
        }
    }
    for element in &delta.added {
        let restored = element.confidence.identity.map(|_| "  (seen before)").unwrap_or_default();
        let _ = writeln!(out, "+ {}{restored}", describe(element));
    }
    for id in &delta.removed {
        match from.elements.iter().find(|element| &element.id == id) {
            Some(element) => {
                let _ = writeln!(out, "- {}", describe(element));
            }
            None => {
                let _ = writeln!(out, "- {}", id.as_str());
            }
        }
    }
    for change in changes {
        let property = change.property.strip_prefix("state.").unwrap_or(&change.property);
        let identity = to
            .elements
            .iter()
            .find(|element| element.id == change.id)
            .and_then(|element| element.confidence.identity)
            .filter(|identity| identity.get() < UNCERTAIN)
            .map(|identity| format!("  (identity {:.2})", identity.get()))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "~ {} {property} {} → {}{identity}",
            change.id.as_str(),
            describe_value(&change.from),
            describe_value(&change.to)
        );
    }
    for (sign, relation) in delta
        .added_relations
        .iter()
        .map(|relation| ('+', relation))
        .chain(delta.removed_relations.iter().map(|relation| ('-', relation)))
    {
        let _ = writeln!(out, "{sign} {}", describe_relation(relation));
    }
    out
}

/// How far the window moved, if it did and its size did not change.
fn window_offset(from: Option<&Window>, to: Option<&Window>) -> Option<(f64, f64)> {
    let (from, to) = (from?.bounds?, to?.bounds?);
    let resized = from.width() != to.width() || from.height() != to.height();
    let offset = (f64::from(to.x() - from.x()), f64::from(to.y() - from.y()));
    (!resized && offset != (0.0, 0.0)).then_some(offset)
}

/// Whether bounds `to` are bounds `from` shifted by `offset`.
fn moved_by(from: &Value, to: &Value, (dx, dy): (f64, f64)) -> bool {
    let get = |value: &Value, key: &str| value.get(key).and_then(Value::as_f64);
    let near = |a: Option<f64>, b: Option<f64>, shift: f64| match (a, b) {
        (Some(a), Some(b)) => (a + shift - b).abs() < 0.01,
        _ => false,
    };
    near(get(from, "x"), get(to, "x"), dx)
        && near(get(from, "y"), get(to, "y"), dy)
        && near(get(from, "width"), get(to, "width"), 0.0)
        && near(get(from, "height"), get(to, "height"), 0.0)
}

fn describe(element: &Element) -> String {
    let role = serde_json::to_value(element.role).ok();
    let role = role.as_ref().and_then(Value::as_str).unwrap_or("unknown");
    match &element.name {
        Some(name) => format!("{} {role} {name:?}", element.id.as_str()),
        None => format!("{} {role}", element.id.as_str()),
    }
}

fn describe_relation(relation: &Relation) -> String {
    let kind = serde_json::to_value(relation.kind).ok();
    let kind = kind.as_ref().and_then(Value::as_str).unwrap_or("unknown").to_owned();
    format!("{} {kind} {}", relation.from.as_str(), relation.to.as_str())
}

fn describe_window(window: Option<&Window>) -> String {
    let Some(window) = window else { return "unknown".to_owned() };
    let bounds = window.bounds.map(|b| format!("{},{} {}×{}", b.x(), b.y(), b.width(), b.height()));
    match (&window.title, bounds) {
        (Some(title), Some(bounds)) => format!("{title:?} {bounds}"),
        (Some(title), None) => format!("{title:?}"),
        (None, Some(bounds)) => bounds,
        (None, None) => "unknown".to_owned(),
    }
}

/// A property value, compactly: bounds as `x,y w×h`, absent as `none`.
fn describe_value(value: &Value) -> String {
    let number = |key: &str| value.get(key).and_then(Value::as_f64);
    match value {
        Value::Null => "none".to_owned(),
        Value::Object(_) => match (number("x"), number("y"), number("width"), number("height")) {
            (Some(x), Some(y), Some(width), Some(height)) => format!("{x},{y} {width}×{height}"),
            _ => value.to_string(),
        },
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use argus_protocol::{
        Bounds, Confidence, ElementId, ElementState, ObservationId, Role, Score, Source, Timestamp,
    };

    use super::*;

    fn element(id: &str, role: Role, name: Option<&str>) -> Element {
        Element {
            id: ElementId::new(id).unwrap(),
            role,
            name: name.map(str::to_owned),
            value: None,
            description: None,
            bounds: Bounds::new(10.0, 20.0, 80.0, 24.0).unwrap(),
            visible_bounds: None,
            state: ElementState { enabled: Some(true), ..ElementState::default() },
            confidence: Confidence::new(Score::CERTAIN),
            sources: vec![Source::Accessibility],
            parent: None,
            children: Vec::new(),
        }
    }

    fn observation(id: &str, elements: Vec<Element>) -> Observation {
        let mut observation = Observation::new(ObservationId::new(id).unwrap(), Timestamp(0));
        observation.elements = elements;
        observation
    }

    #[test]
    fn renders_the_roadmap_example() {
        let from = observation(
            "obs_1",
            vec![element("e_17", Role::Button, Some("Save")), element("e_18", Role::TextBox, None)],
        );
        let mut save = element("e_17", Role::Button, Some("Save"));
        save.state.enabled = Some(false);
        save.confidence.identity = Some(Score::CERTAIN);
        let mut search = element("e_18", Role::TextBox, None);
        search.confidence.identity = Some(Score::new(0.6).unwrap());
        search.bounds = Bounds::new(10.0, 22.0, 80.0, 24.0).unwrap();
        let mut to =
            observation("obs_2", vec![save, search, element("e_19", Role::Dialog, Some("Saved"))]);
        to.previous = Some(from.id.clone());

        let delta = argus_core::delta(&from, &to).unwrap();
        assert_eq!(
            render_delta(&from, &to, &delta, false),
            "obs_2  +1 -0 ~2\n\
             + e_19 dialog \"Saved\"\n\
             ~ e_17 enabled true → false\n\
             ~ e_18 bounds 10,20 80×24 → 10,22 80×24  (identity 0.60)\n"
        );
        // Identity changes are details.
        assert!(render_delta(&from, &to, &delta, true).contains("confidence.identity"));
    }

    #[test]
    fn moving_the_window_is_summarized() {
        let window = |x: f32| Window {
            title: Some("Demo".to_owned()),
            bounds: Some(Bounds::new(x, 0.0, 300.0, 200.0).unwrap()),
        };
        let mut from = observation("obs_1", vec![element("e_1", Role::Button, Some("Save"))]);
        from.window = Some(window(0.0));
        let mut save = element("e_1", Role::Button, Some("Saved"));
        save.bounds = Bounds::new(110.0, 20.0, 80.0, 24.0).unwrap();
        let mut to = observation("obs_2", vec![save]);
        to.window = Some(window(100.0));
        to.previous = Some(from.id.clone());
        let delta = argus_core::delta(&from, &to).unwrap();
        assert_eq!(
            render_delta(&from, &to, &delta, false),
            "obs_2  +0 -0 ~1\n\
             ~ window \"Demo\" 0,0 300×200 → \"Demo\" 100,0 300×200\n  \
             (1 elements moved with the window)\n\
             ~ e_1 name \"Save\" → \"Saved\"\n"
        );
    }

    #[test]
    fn quiet_deltas_print_nothing() {
        let from = observation("obs_1", vec![element("e_1", Role::Button, Some("Save"))]);
        let mut same = element("e_1", Role::Button, Some("Save"));
        same.confidence.identity = Some(Score::CERTAIN);
        let mut to = observation("obs_2", vec![same]);
        to.previous = Some(from.id.clone());
        let delta = argus_core::delta(&from, &to).unwrap();
        assert_eq!(render_delta(&from, &to, &delta, false), "");
        assert!(render_delta(&from, &to, &delta, true).starts_with("obs_2  +0 -0 ~1"));
    }
}
