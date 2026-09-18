//! `argus inspect`: show the evidence behind observed elements.
//!
//! Element IDs are assigned per observation. `inspect` observes again, so an
//! ID taken from an earlier `argus observe` refers to the same element only
//! while the interface is unchanged.

use std::fmt::Write as _;

use anyhow::{Context, anyhow};
use argus_core::fusion::{ElementEvidence, Link};
use argus_core::{Inspection, Observer};
use argus_protocol::{Confidence, Element, ElementState, Score};
use serde::Serialize;
use serde_json::json;

use crate::commands::observe::{SourceArgs, ensure_valid};
use crate::commands::target::AppTargetArgs;
use crate::output::{print_json, print_text};

#[derive(Debug, clap::Args)]
pub(crate) struct InspectArgs {
    /// Element to inspect (e.g. `e_42`). All elements when omitted.
    #[arg(value_name = "ELEMENT")]
    element: Option<String>,

    /// Print JSON instead of text.
    #[arg(long)]
    json: bool,

    #[command(flatten)]
    sources: SourceArgs,

    #[command(flatten)]
    target: AppTargetArgs,
}

pub(crate) fn run(args: &InspectArgs) -> anyhow::Result<()> {
    let observer = Observer::new()?;
    let inspection = observer.inspect(&args.target.target(), &args.sources.sources())?;
    ensure_valid(&inspection.observation)?;

    let selected: Vec<(&Element, &ElementEvidence)> = match &args.element {
        Some(id) => vec![inspection.element(id).with_context(|| {
            format!(
                "no element `{id}` in the new observation ({} elements); IDs change when the interface changes",
                inspection.observation.elements.len()
            )
        })?],
        None => inspection.observation.elements.iter().zip(&inspection.evidence).collect(),
    };

    if args.json {
        let items: Vec<_> = selected
            .iter()
            .map(|(element, evidence)| json!({ "element": element, "evidence": evidence }))
            .collect();
        return print_json(&json!({
            "observation": inspection.observation.id,
            "elements": items,
        }));
    }
    print_text(&render(&inspection, &selected)?)
}

fn render(
    inspection: &Inspection,
    selected: &[(&Element, &ElementEvidence)],
) -> anyhow::Result<String> {
    let mut out = String::new();
    writeln!(out, "Observation {}", inspection.observation.id.as_str())?;
    for (element, evidence) in selected {
        writeln!(out)?;
        write_element(&mut out, element, evidence)?;
    }
    Ok(out)
}

fn write_element(
    out: &mut String,
    element: &Element,
    evidence: &ElementEvidence,
) -> anyhow::Result<()> {
    let confidence = &element.confidence;
    writeln!(out, "Element {}", element.id.as_str())?;
    writeln!(out, "role: {}{}", word(&element.role)?, score(confidence.role))?;
    if let Some(name) = &element.name {
        writeln!(out, "name: {name:?}{}", score(confidence.name))?;
    }
    if let Some(value) = &element.value {
        writeln!(out, "value: {value:?}{}", score(confidence.value))?;
    }
    let b = element.bounds;
    writeln!(out, "bounds: x={} y={} w={} h={}", b.x(), b.y(), b.width(), b.height())?;
    if !element.state.is_unknown() {
        writeln!(out, "state: {}{}", state(&element.state)?, score(confidence.state))?;
    }
    writeln!(out, "exists: {:.2}", confidence.element.get())?;
    let sources: Vec<String> = element.sources.iter().map(word).collect::<anyhow::Result<_>>()?;
    writeln!(out, "sources: {}", sources.join(", "))?;
    if let Some(parent) = &element.parent {
        writeln!(out, "parent: {}", parent.as_str())?;
    }

    writeln!(out, "Evidence:")?;
    for contribution in &evidence.contributions {
        let mut line = format!("  {} ({})", word(&contribution.source)?, contribution.link);
        if contribution.link == Link::Text {
            if let Some(text) = &contribution.name {
                write!(line, " text={text:?}")?;
            }
        } else {
            write!(line, " role={}", word(&contribution.role)?)?;
            if let Some(name) = &contribution.name {
                write!(line, " name={name:?}")?;
            }
            if let Some(value) = &contribution.value {
                write!(line, " value={value:?}")?;
            }
            if !contribution.state.is_unknown() {
                write!(line, " {}", state(&contribution.state)?)?;
            }
        }
        write!(line, " confidence={}", confidences(&contribution.confidence))?;
        if let Some(native) = &contribution.native_role {
            write!(line, " native={native}")?;
        }
        writeln!(out, "{line}")?;
    }
    if !evidence.conflicts.is_empty() {
        writeln!(out, "Conflicts:")?;
        for conflict in &evidence.conflicts {
            writeln!(
                out,
                "  {}: kept {} {:?}, rejected {} {:?}{}",
                conflict.property,
                word(&conflict.kept.source)?,
                conflict.kept.value,
                word(&conflict.rejected.source)?,
                conflict.rejected.value,
                if conflict.lowers_confidence { " (confidence lowered)" } else { "" },
            )?;
        }
    }
    Ok(())
}

/// The protocol spelling of an enum value (`text_box`, `accessibility`).
fn word(value: &impl Serialize) -> anyhow::Result<String> {
    match serde_json::to_value(value)? {
        serde_json::Value::String(text) => Ok(text),
        other => Err(anyhow!("expected a string, got {other}")),
    }
}

fn state(state: &ElementState) -> anyhow::Result<String> {
    let serde_json::Value::Object(fields) = serde_json::to_value(state)? else {
        return Err(anyhow!("state is not an object"));
    };
    Ok(fields
        .iter()
        .map(|(key, value)| {
            format!("{key}={}", value.as_str().map_or(value.to_string(), str::to_owned))
        })
        .collect::<Vec<_>>()
        .join(" "))
}

fn score(score: Option<Score>) -> String {
    score.map_or(String::new(), |score| format!(" ({:.2})", score.get()))
}

fn confidences(confidence: &Confidence) -> String {
    let mut text = format!("{:.2}", confidence.element.get());
    for (name, value) in [("role", confidence.role), ("state", confidence.state)] {
        if let Some(value) = value {
            write!(text, " {name}={:.2}", value.get()).expect("writing to a string");
        }
    }
    text
}
