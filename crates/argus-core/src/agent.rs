//! The agent view: a compact text rendering of observations and deltas for
//! language-model agents, and the regions Argus cannot describe well enough
//! in text (to be shown to the agent as images instead).
//!
//! A view over the protocol, not a data model of its own: everything here is
//! derived from [`Observation`] and [`ObservationDelta`].

use std::fmt::Write as _;

use argus_protocol::{Bounds, CheckState, Element, Observation, ObservationDelta, Role, Source};
use serde::Serialize;

/// How the agent view is rendered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgentOptions {
    /// Elements seen only in pixels with a lower existence confidence are
    /// left out (they are noise more often than not).
    pub min_confidence: f32,
    /// Bounds changes smaller than this, in points, are not reported.
    pub min_move: f32,
}

impl Default for AgentOptions {
    fn default() -> Self {
        Self { min_confidence: 0.5, min_move: 2.0 }
    }
}

/// Below this, a role or name is marked uncertain (`?`).
const LOW: f32 = 0.6;

/// A part of the window the text describes poorly.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WeakRegion {
    /// The region, in global screen points.
    pub bounds: Bounds,
    /// Why the region is weak.
    pub reason: RegionReason,
}

/// Why a region is described poorly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegionReason {
    /// The window exposes no accessibility at all: everything comes from
    /// pixels.
    NoAccessibility,
    /// Controls detected in pixels only, with uncertain roles or no names.
    UncertainElements,
    /// A drawn area (canvas, chart, game) without structure.
    DrawnContent,
    /// A pop-up window without accessibility (e.g. a custom menu).
    Popup,
}

fn pixel_only(element: &Element) -> bool {
    !element.sources.iter().any(|source| matches!(source, Source::Accessibility))
}

fn score(value: Option<argus_protocol::Score>) -> f32 {
    value.map_or(1.0, |score| score.get())
}

/// Whether the agent view shows the element at all.
fn shown(element: &Element, options: &AgentOptions) -> bool {
    !(pixel_only(element) && element.confidence.element.get() < options.min_confidence)
}

/// Structural elements without content are skipped (their children are
/// kept).
fn silent(element: &Element) -> bool {
    matches!(element.role, Role::Group | Role::Unknown | Role::Cell)
        && element.name.is_none()
        && element.value.is_none()
}

fn role_name(role: Role) -> String {
    serde_json::to_value(role)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

fn quote(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

fn boxed(bounds: &Bounds) -> String {
    format!("({:.0},{:.0} {:.0}x{:.0})", bounds.x(), bounds.y(), bounds.width(), bounds.height())
}

/// A slice of `text` by character positions.
fn chars(text: &str, start: u32, length: u32) -> String {
    text.chars().skip(start as usize).take(length as usize).collect()
}

fn excerpt(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max - 1).collect();
    format!("{head}…")
}

/// One element on one line, without bounds.
fn label(element: &Element) -> String {
    let confidence = &element.confidence;
    let mut out = element.id.to_string();
    out.push(' ');
    out.push_str(&role_name(element.role));
    if score(confidence.role) < LOW {
        out.push('?');
    }
    if let Some(name) = &element.name {
        out.push(' ');
        out.push_str(&quote(name));
        if score(confidence.name) < LOW {
            out.push('?');
        }
    }
    if let Some(value) = &element.value {
        out.push_str(" = ");
        out.push_str(&quote(value));
    }
    let state = &element.state;
    let mut flags = Vec::new();
    if state.enabled == Some(false) {
        flags.push("disabled");
    }
    match state.checked {
        Some(CheckState::Checked) => flags.push("checked"),
        Some(CheckState::Unchecked) => flags.push("unchecked"),
        Some(CheckState::Mixed) => flags.push("mixed"),
        None => {}
    }
    for (flag, name) in
        [(state.focused, "focused"), (state.selected, "selected"), (state.expanded, "expanded")]
    {
        if flag == Some(true) {
            flags.push(name);
        }
    }
    if state.visible == Some(false) {
        flags.push("hidden");
    }
    if !flags.is_empty() {
        let _ = write!(out, " [{}]", flags.join(", "));
    }
    out
}

/// The selection and styles of a text control, on continuation lines.
fn text_lines(element: &Element, indent: &str) -> Vec<String> {
    let (Some(text), Some(value)) = (&element.text, &element.value) else { return Vec::new() };
    let mut lines = Vec::new();
    // An insertion point matters only where typing goes.
    let selection = text
        .selection
        .filter(|selection| selection.length > 0 || element.state.focused == Some(true));
    if let Some(selection) = selection {
        let line = if selection.length == 0 {
            format!("{indent}  caret at {}", selection.start)
        } else {
            format!(
                "{indent}  selected {}..{} {}",
                selection.start,
                selection.start + selection.length,
                quote(&excerpt(&chars(value, selection.start, selection.length), 80))
            )
        };
        lines.push(line);
    }
    if !text.runs.is_empty() {
        let runs: Vec<String> = text
            .runs
            .iter()
            .take(20)
            .map(|run| {
                let mut style = Vec::new();
                if let Some(font) = &run.font {
                    style.push(font.clone());
                }
                if let Some(size) = run.size {
                    style.push(format!("{size:.0}pt"));
                }
                for (flag, name) in
                    [(run.bold, "bold"), (run.italic, "italic"), (run.underline, "underline")]
                {
                    if flag == Some(true) {
                        style.push(name.to_owned());
                    }
                }
                format!(
                    "{} {}",
                    quote(&excerpt(chars(value, run.start, run.length).trim_end(), 40)),
                    style.join(" ")
                )
            })
            .collect();
        let more = text.runs.len().saturating_sub(20);
        let mut line = format!("{indent}  styles: {}", runs.join("; "));
        if more > 0 {
            let _ = write!(line, "; … {more} more");
        }
        lines.push(line);
    }
    lines
}

/// Whether consecutive siblings form a row of like controls, shown on one
/// line (a calculator keypad row, a toolbar).
fn same_row(a: &Element, b: &Element) -> bool {
    a.role == b.role
        && matches!(
            a.role,
            Role::Button
                | Role::Checkbox
                | Role::RadioButton
                | Role::Tab
                | Role::MenuItem
                | Role::Icon
        )
        && a.children.is_empty()
        && b.children.is_empty()
        && (a.bounds.y() - b.bounds.y()).abs() <= 2.0
        && (a.bounds.height() - b.bounds.height()).abs() <= 2.0
}

/// The observation as indented text.
pub fn render_observation(observation: &Observation, options: &AgentOptions) -> String {
    let mut out = String::new();
    let window = observation.window.as_ref();
    let title = window.and_then(|window| window.title.as_deref()).unwrap_or("");
    let _ = write!(out, "Window {}", quote(title));
    if let Some(bounds) = window.and_then(|window| window.bounds) {
        let _ = write!(out, " {}", boxed(&bounds));
    }
    out.push('\n');

    if !observation.elements.is_empty() && observation.elements.iter().all(pixel_only) {
        render_targets(observation, options, &mut out);
        return out;
    }

    let known: std::collections::HashSet<&str> =
        observation.elements.iter().map(|element| element.id.as_str()).collect();
    let roots: Vec<&Element> = observation
        .elements
        .iter()
        .filter(|element| {
            element.parent.as_ref().is_none_or(|parent| !known.contains(parent.as_str()))
        })
        .collect();
    let by_id: std::collections::HashMap<&str, &Element> =
        observation.elements.iter().map(|element| (element.id.as_str(), element)).collect();
    let mut hidden = 0;
    render_level(&roots, &by_id, 0, options, &mut out, &mut hidden);
    if hidden > 0 {
        let _ = writeln!(out, "({hidden} low-confidence pixel detections not shown)");
    }
    let labels: Vec<String> = observation
        .relations
        .iter()
        .filter(|relation| relation.kind == argus_protocol::RelationKind::LabelFor)
        .map(|relation| format!("{}→{}", relation.from, relation.to))
        .collect();
    if !labels.is_empty() {
        let _ = writeln!(out, "Labels: {}", labels.join(", "));
    }
    out
}

/// Most targets listed for a window without accessibility.
const MAX_TARGETS: usize = 60;

/// A window without accessibility: its image goes to the agent anyway, so
/// the text is only a short, flat list of what can be addressed by id,
/// controls first. Nesting, low-confidence detections and long text are
/// left out: the image shows them better.
fn render_targets(observation: &Observation, options: &AgentOptions, out: &mut String) {
    out.push_str("(no accessibility: read the window image; these targets can be clicked by id)\n");
    let candidates: Vec<&Element> = observation
        .elements
        .iter()
        .filter(|element| shown(element, options) && element.name.is_some())
        .filter(|element| !matches!(element.role, Role::Window | Role::Group | Role::Unknown))
        .collect();
    let (controls, texts): (Vec<&Element>, Vec<&Element>) =
        candidates.into_iter().partition(|element| element.role != Role::Text);
    let mut listed = 0;
    for element in controls.iter().chain(texts.iter()).take(MAX_TARGETS) {
        let role = role_name(element.role);
        let name = excerpt(element.name.as_deref().unwrap_or(""), 40);
        let _ = writeln!(out, "{} {role} {} {}", element.id, quote(&name), boxed(&element.bounds));
        listed += 1;
    }
    let total = controls.len() + texts.len();
    if total > listed {
        let _ = writeln!(out, "(… {} more text items: see the image)", total - listed);
    }
}

/// Whether `expect`, the name an agent expects of an element before acting
/// on it, fits the element. Compared case-insensitively, ignoring
/// punctuation and spaces:
/// - the name, description or label name (a text field is often called by
///   its label) equals `expect`, or contains it as most of itself
///   (`"Save"` fits `"Save…"`, but not `"Don't Save"`);
/// - or the value contains `expect` (a text field called by its content).
pub fn expect_matches(observation: &Observation, element: &Element, expect: &str) -> bool {
    let expect = letters(expect);
    if expect.is_empty() {
        return true;
    }
    let labels = observation
        .relations
        .iter()
        .filter(|relation| {
            relation.kind == argus_protocol::RelationKind::LabelFor && relation.to == element.id
        })
        .filter_map(|relation| {
            observation.elements.iter().find(|other| other.id == relation.from)?.name.as_deref()
        });
    let names = [element.name.as_deref(), element.description.as_deref()]
        .into_iter()
        .flatten()
        .chain(labels)
        .map(letters);
    let fits = |name: String| {
        name == expect
            || (name.contains(&expect) && expect.chars().count() * 3 > name.chars().count() * 2)
    };
    names.into_iter().any(fits)
        || element.value.as_deref().is_some_and(|value| letters(value).contains(&expect))
}

/// Lowercase letters and digits only.
fn letters(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn render_level(
    level: &[&Element],
    by_id: &std::collections::HashMap<&str, &Element>,
    depth: usize,
    options: &AgentOptions,
    out: &mut String,
    hidden: &mut usize,
) {
    let indent = "  ".repeat(depth);
    // A hidden element's children still count: they may be confident.
    let mut visible: Vec<&Element> = Vec::new();
    for element in level {
        if shown(element, options) {
            visible.push(element);
        } else {
            *hidden += 1;
            let children: Vec<&Element> =
                element.children.iter().filter_map(|id| by_id.get(id.as_str()).copied()).collect();
            render_level(&children, by_id, depth, options, out, hidden);
        }
    }
    let mut index = 0;
    while index < visible.len() {
        let element = visible[index];
        if silent(element) {
            let children: Vec<&Element> =
                element.children.iter().filter_map(|id| by_id.get(id.as_str()).copied()).collect();
            render_level(&children, by_id, depth, options, out, hidden);
            index += 1;
            continue;
        }
        let mut end = index + 1;
        while end < visible.len() && same_row(visible[end - 1], visible[end]) {
            end += 1;
        }
        if end - index >= 3 {
            let row = &visible[index..end];
            let union =
                row.iter().skip(1).fold(row[0].bounds, |acc, element| cover(acc, element.bounds));
            let items: Vec<String> = row
                .iter()
                .map(|element| {
                    let full = label(element);
                    // `e_7 button "Save"` → `e_7 "Save"`: the role is on the row.
                    let role = format!(" {}", role_name(element.role));
                    full.replacen(&role, "", 1)
                })
                .collect();
            let _ = writeln!(
                out,
                "{indent}row of {} {}s {}: {}",
                row.len(),
                role_name(row[0].role),
                boxed(&union),
                items.join(" | ")
            );
            index = end;
            continue;
        }
        let bounds = element.visible_bounds.unwrap_or(element.bounds);
        let mut line = format!("{indent}{} {}", label(element), boxed(&bounds));
        if element.confidence.element.get() < LOW {
            line.push_str(" (uncertain)");
        }
        out.push_str(&line);
        out.push('\n');
        for extra in text_lines(element, &indent) {
            out.push_str(&extra);
            out.push('\n');
        }
        let children: Vec<&Element> =
            element.children.iter().filter_map(|id| by_id.get(id.as_str()).copied()).collect();
        render_level(&children, by_id, depth + 1, options, out, hidden);
        index += 1;
    }
}

fn cover(a: Bounds, b: Bounds) -> Bounds {
    let x = a.x().min(b.x());
    let y = a.y().min(b.y());
    let right = (a.x() + a.width()).max(b.x() + b.width());
    let bottom = (a.y() + a.height()).max(b.y() + b.height());
    Bounds::new(x, y, right - x, bottom - y).unwrap_or(a)
}

/// What changed between two observations, in the agent's terms: noise
/// (confidence, bookkeeping, sub-point moves, low-confidence pixel
/// detections) is left out.
pub fn render_delta(
    delta: &ObservationDelta,
    before: &Observation,
    after: &Observation,
    options: &AgentOptions,
) -> String {
    let old: std::collections::HashMap<&str, &Element> =
        before.elements.iter().map(|element| (element.id.as_str(), element)).collect();
    let new: std::collections::HashMap<&str, &Element> =
        after.elements.iter().map(|element| (element.id.as_str(), element)).collect();
    let mut lines = Vec::new();
    if let Some(window) = &delta.window {
        let title = window.title.as_deref().unwrap_or("");
        let mut line = format!("window now {}", quote(title));
        if let Some(bounds) = window.bounds {
            let _ = write!(line, " {}", boxed(&bounds));
        }
        lines.push(line);
    }
    // Uniform moves (the window moved) are summarized.
    let moves: Vec<(f32, f32)> = delta
        .changed
        .iter()
        .filter(|change| change.property == "bounds")
        .filter_map(|change| {
            let from: Bounds = serde_json::from_value(change.from.clone()).ok()?;
            let to: Bounds = serde_json::from_value(change.to.clone()).ok()?;
            Some(((to.x() - from.x()).round(), (to.y() - from.y()).round()))
        })
        .collect();
    let shift = (moves.len() > 5 && moves.iter().all(|m| *m == moves[0])).then(|| moves[0]);
    if let Some((dx, dy)) = shift {
        lines.push(format!("{} elements moved by ({dx:.0},{dy:.0})", moves.len()));
    }
    for element in &delta.added {
        if shown(element, options) {
            lines.push(format!("+ {} {}", label(element), boxed(&element.bounds)));
            for extra in text_lines(element, "") {
                lines.push(extra);
            }
        }
    }
    for id in &delta.removed {
        match old.get(id.as_str()) {
            Some(element) if !shown(element, options) => {}
            Some(element) => lines.push(format!("- {}", label(element))),
            None => lines.push(format!("- {id}")),
        }
    }
    for change in &delta.changed {
        let property = change.property.as_str();
        if property.starts_with("confidence.")
            || matches!(property, "sources" | "children" | "parent")
            || (property == "bounds" && shift.is_some())
        {
            continue;
        }
        let Some(element) = new.get(change.id.as_str()) else { continue };
        if !shown(element, options) {
            continue;
        }
        let name =
            element.name.as_deref().map(|name| format!(" {}", quote(name))).unwrap_or_default();
        match property {
            "bounds" | "visible_bounds" => {
                let from: Option<Bounds> = serde_json::from_value(change.from.clone()).ok();
                let to: Option<Bounds> = serde_json::from_value(change.to.clone()).ok();
                if let (Some(from), Some(to)) = (from, to) {
                    let moved = [
                        from.x() - to.x(),
                        from.y() - to.y(),
                        from.width() - to.width(),
                        from.height() - to.height(),
                    ];
                    if moved.iter().all(|d| d.abs() < options.min_move) {
                        continue;
                    }
                }
                let show = |b: Option<Bounds>| b.map_or("none".to_owned(), |b| boxed(&b));
                lines.push(format!(
                    "~ {}{name} {property}: {} → {}",
                    change.id,
                    show(from),
                    show(to)
                ));
            }
            "text" => {
                lines.push(format!("~ {}{name} text:", change.id));
                lines.extend(text_lines(element, ""));
            }
            _ => lines
                .push(format!("~ {}{name} {property}: {} → {}", change.id, change.from, change.to)),
        }
    }
    if lines.is_empty() { "no visible change".to_owned() } else { lines.join("\n") }
}

/// Parts of the observed window (and its pop-ups) the text describes
/// poorly, largest first: the agent should look at them as images.
pub fn weak_regions(observation: &Observation) -> Vec<WeakRegion> {
    let window = observation.window.as_ref().and_then(|window| window.bounds);
    let structured = observation.elements.iter().any(|element| !pixel_only(element));
    // Accessible controls already describe what is drawn inside them.
    let controls: Vec<Bounds> = observation
        .elements
        .iter()
        .filter(|element| {
            !pixel_only(element)
                && !matches!(
                    element.role,
                    Role::Window
                        | Role::Dialog
                        | Role::Group
                        | Role::Unknown
                        | Role::Table
                        | Role::List
                        | Role::Row
                        | Role::Cell
                )
        })
        .map(|element| element.bounds)
        .collect();
    let described = |bounds: &Bounds| {
        let (x, y) = (bounds.x() + bounds.width() / 2.0, bounds.y() + bounds.height() / 2.0);
        controls.iter().any(|control| {
            x >= control.x()
                && x <= control.x() + control.width()
                && y >= control.y()
                && y <= control.y() + control.height()
        })
    };
    let mut regions = Vec::new();
    if !structured {
        if let Some(window) = window {
            regions.push(WeakRegion { bounds: window, reason: RegionReason::NoAccessibility });
        }
    }
    for element in &observation.elements {
        if !pixel_only(element) {
            continue;
        }
        let bounds = element.visible_bounds.unwrap_or(element.bounds);
        // Not entirely inside the window: floats above it.
        let outside = window.is_some_and(|window| window.intersection(&bounds) != Some(bounds));
        let reason = if matches!(element.role, Role::Menu | Role::Dialog) && outside {
            Some(RegionReason::Popup)
        } else if !structured {
            None // covered by the whole window
        } else if element.role == Role::Group
            && element.name.is_none()
            && bounds.width() * bounds.height() >= 10_000.0
        {
            Some(RegionReason::DrawnContent)
        } else if (matches!(element.role, Role::Icon | Role::Unknown) && element.name.is_none()
            || score(element.confidence.role) < LOW)
            && !described(&bounds)
        {
            Some(RegionReason::UncertainElements)
        } else {
            None
        };
        if let Some(reason) = reason {
            regions.push(WeakRegion { bounds: pad(bounds, 6.0), reason });
        }
    }
    merge(regions)
}

fn pad(bounds: Bounds, by: f32) -> Bounds {
    Bounds::new(
        bounds.x() - by,
        bounds.y() - by,
        bounds.width() + 2.0 * by,
        bounds.height() + 2.0 * by,
    )
    .unwrap_or(bounds)
}

/// Merges overlapping regions of the same kind and keeps the four largest.
fn merge(mut regions: Vec<WeakRegion>) -> Vec<WeakRegion> {
    let mut merged: Vec<WeakRegion> = Vec::new();
    while let Some(mut region) = regions.pop() {
        loop {
            // Regions of different kinds stay apart: a pop-up has a frame of
            // its own and must not be cropped from the window's.
            let near = merged.iter().position(|other| {
                other.reason == region.reason
                    && pad(other.bounds, 8.0).intersection(&region.bounds).is_some()
            });
            match near {
                Some(index) => {
                    let other = merged.swap_remove(index);
                    region = WeakRegion { bounds: cover(other.bounds, region.bounds), ..region };
                }
                None => break,
            }
        }
        merged.push(region);
    }
    let area = |r: &WeakRegion| r.bounds.width() * r.bounds.height();
    merged.sort_by(|a, b| area(b).total_cmp(&area(a)));
    // A region inside a larger one adds nothing.
    let mut kept: Vec<WeakRegion> = Vec::new();
    for region in merged {
        if !kept
            .iter()
            .any(|larger| larger.bounds.intersection(&region.bounds) == Some(region.bounds))
        {
            kept.push(region);
        }
    }
    kept.truncate(4);
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_protocol::{
        Confidence, ElementId, ElementState, ObservationId, Score, TextRange, TextRun, TextState,
        Timestamp, Window,
    };

    fn element(
        id: &str,
        role: Role,
        name: Option<&str>,
        bounds: (f32, f32, f32, f32),
        source: Source,
    ) -> Element {
        Element {
            id: ElementId::new(id).unwrap(),
            role,
            name: name.map(str::to_owned),
            value: None,
            text: None,
            description: None,
            bounds: Bounds::new(bounds.0, bounds.1, bounds.2, bounds.3).unwrap(),
            visible_bounds: None,
            state: ElementState::default(),
            confidence: Confidence::new(Score::CERTAIN),
            sources: vec![source],
            parent: None,
            children: Vec::new(),
        }
    }

    fn observation(elements: Vec<Element>) -> Observation {
        let mut observation = Observation::new(ObservationId::new("obs_1").unwrap(), Timestamp(0));
        observation.window = Some(Window {
            title: Some("Demo".into()),
            bounds: Some(Bounds::new(0.0, 0.0, 400.0, 300.0).unwrap()),
        });
        observation.elements = elements;
        observation
    }

    #[test]
    fn keypad_rows_share_a_line() {
        let keys: Vec<Element> = ["7", "8", "9"]
            .iter()
            .enumerate()
            .map(|(i, name)| {
                element(
                    &format!("e_{}", i + 1),
                    Role::Button,
                    Some(name),
                    (i as f32 * 50.0, 100.0, 48.0, 48.0),
                    Source::Accessibility,
                )
            })
            .collect();
        let text = render_observation(&observation(keys), &AgentOptions::default());
        assert!(
            text.contains(r#"row of 3 buttons (0,100 148x48): e_1 "7" | e_2 "8" | e_3 "9""#),
            "{text}"
        );
    }

    #[test]
    fn text_controls_show_selection_and_styles() {
        let mut field =
            element("e_1", Role::TextBox, None, (0.0, 0.0, 300.0, 100.0), Source::Accessibility);
        field.value = Some("Title\nBody".into());
        let run = |start, length, font: &str, bold| TextRun {
            start,
            length,
            font: Some(font.into()),
            size: Some(12.0),
            bold: Some(bold),
            italic: Some(false),
            underline: None,
        };
        field.text = Some(TextState {
            selection: Some(TextRange { start: 0, length: 5 }),
            runs: vec![run(0, 6, "Helvetica-Bold", true), run(6, 4, "Helvetica", false)],
        });
        let text = render_observation(&observation(vec![field]), &AgentOptions::default());
        assert!(text.contains(r#"selected 0..5 "Title""#), "{text}");
        assert!(
            text.contains(r#"styles: "Title" Helvetica-Bold 12pt bold; "Body" Helvetica 12pt"#),
            "{text}"
        );
    }

    #[test]
    fn low_confidence_pixel_detections_are_hidden() {
        let mut noise = element("e_2", Role::Group, None, (10.0, 10.0, 14.0, 14.0), Source::Vision);
        noise.confidence = Confidence::new(Score::new(0.3).unwrap());
        let text = render_observation(
            &observation(vec![
                element(
                    "e_1",
                    Role::Button,
                    Some("OK"),
                    (0.0, 0.0, 50.0, 20.0),
                    Source::Accessibility,
                ),
                noise,
            ]),
            &AgentOptions::default(),
        );
        assert!(!text.contains("e_2"), "{text}");
        assert!(text.contains("1 low-confidence pixel detections not shown"), "{text}");
    }

    #[test]
    fn windows_without_accessibility_get_a_short_target_list() {
        let mut elements =
            vec![element("e_1", Role::Group, None, (0.0, 0.0, 400.0, 300.0), Source::Vision)];
        elements.push(element(
            "e_2",
            Role::Text,
            Some("Hello"),
            (10.0, 40.0, 50.0, 12.0),
            Source::Ocr,
        ));
        elements.push(element(
            "e_3",
            Role::Tab,
            Some("Fonts"),
            (10.0, 10.0, 50.0, 20.0),
            Source::Vision,
        ));
        let text = render_observation(&observation(elements), &AgentOptions::default());
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[1].starts_with("(no accessibility"), "{text}");
        assert_eq!(lines[2], r#"e_3 tab "Fonts" (10,10 50x20)"#, "controls first: {text}");
        assert_eq!(lines[3], r#"e_2 text "Hello" (10,40 50x12)"#);
        assert_eq!(lines.len(), 4, "no group line: {text}");
    }

    #[test]
    fn expect_matches_names_values_and_labels() {
        let mut field =
            element("e_2", Role::TextBox, None, (60.0, 0.0, 100.0, 20.0), Source::Accessibility);
        field.value = Some("80".into());
        let label = element(
            "e_1",
            Role::Text,
            Some("Width:"),
            (0.0, 0.0, 50.0, 20.0),
            Source::Accessibility,
        );
        let button = element(
            "e_3",
            Role::Button,
            Some("Save"),
            (0.0, 30.0, 50.0, 20.0),
            Source::Accessibility,
        );
        let mut window = observation(vec![label, field.clone(), button.clone()]);
        window.relations.push(argus_protocol::Relation {
            kind: argus_protocol::RelationKind::LabelFor,
            from: ElementId::new("e_1").unwrap(),
            to: ElementId::new("e_2").unwrap(),
            confidence: Some(Score::CERTAIN),
        });
        assert!(expect_matches(&window, &button, "save"));
        assert!(expect_matches(&window, &button, "Save…"), "the name inside the expectation");
        assert!(!expect_matches(&window, &button, "Cancel"));
        let mut other = button.clone();
        other.name = Some("Don't Save".into());
        assert!(!expect_matches(&window, &other, "Save"), "another button containing the name");
        assert!(expect_matches(&window, &field, "80"), "by value");
        assert!(expect_matches(&window, &field, "Width"), "by label");
        assert!(!expect_matches(&window, &field, "Height"));
    }

    #[test]
    fn weak_regions_cover_windows_without_accessibility_and_drawn_areas() {
        let pixels = observation(vec![element(
            "e_1",
            Role::Text,
            Some("Hello"),
            (10.0, 10.0, 50.0, 12.0),
            Source::Ocr,
        )]);
        let regions = weak_regions(&pixels);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].reason, RegionReason::NoAccessibility);

        let canvas =
            element("e_2", Role::Group, None, (100.0, 100.0, 200.0, 150.0), Source::Vision);
        let mixed = observation(vec![
            element("e_1", Role::Button, Some("OK"), (0.0, 0.0, 50.0, 20.0), Source::Accessibility),
            canvas,
            element("e_3", Role::Icon, None, (150.0, 150.0, 20.0, 20.0), Source::Vision),
        ]);
        let regions = weak_regions(&mixed);
        assert_eq!(regions.len(), 1, "{regions:?}");
        assert_eq!(regions[0].reason, RegionReason::DrawnContent);
    }
}
