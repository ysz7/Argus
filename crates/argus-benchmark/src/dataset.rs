//! The dataset: recorded interface states and their ground truth.
//!
//! ```text
//! benchmarks/dataset/<case>/
//!     case.json                 description
//!     <step>/                   one recorded state; steps sort by name
//!         frame.png             the window's pixels (RGBA, as captured)
//!         frame.json            where the frame was on screen
//!         accessibility.json    the raw accessibility tree (optional)
//!         expected.json         the ground truth
//! ```
//!
//! Successive steps of a case are states of one window over time: they are
//! observed in order, as one tracking session.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use argus_core::accessibility::{AxSnapshot, candidates};
use argus_core::{assemble, fuse, normalize};
use argus_protocol::{
    Application, Bounds, Confidence, Element, ElementState, Frame, FrameId, Observation,
    ObservationId, PixelBuffer, RelationKind, Role, Score, Timestamp,
};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// A recorded case: one window, one or more states.
#[derive(Debug, Clone)]
pub struct Case {
    /// Directory name.
    pub name: String,
    pub description: String,
    pub steps: Vec<Step>,
}

/// `case.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CaseInfo {
    pub description: String,
}

/// One recorded state of the window.
#[derive(Debug, Clone)]
pub struct Step {
    /// Directory name.
    pub name: String,
    pub frame: Frame,
    pub info: FrameInfo,
    pub accessibility: Option<AxSnapshot>,
    pub truth: Truth,
}

/// `frame.json`: where the frame was on screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameInfo {
    /// The window frame in global points.
    pub bounds: Bounds,
    /// Pixels per point.
    pub scale_factor: f32,
    /// The application owning the window.
    #[serde(default)]
    pub application: Application,
    /// Window title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// `expected.json`: what a perfect observation of the step contains.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Truth {
    pub elements: Vec<TruthElement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<TruthRelation>,
}

/// One real element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TruthElement {
    /// Identity of the element across the steps of a case (not an Argus
    /// element ID): the same element has the same `id` in every step.
    pub id: String,
    pub role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Global points.
    pub bounds: Bounds,
    #[serde(default)]
    pub state: ElementState,
    /// The structural parent's `id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Whether an observation must contain it: visible leaves and controls
    /// count, structural containers do not.
    pub significant: bool,
    /// Where the truth comes from, when not from the accessibility tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A relation between two truth elements.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TruthRelation {
    pub kind: RelationKind,
    pub from: String,
    pub to: String,
}

impl Truth {
    pub fn get(&self, id: &str) -> Option<&TruthElement> {
        self.elements.iter().find(|element| element.id == id)
    }

    /// The truth as an observation (for drawing it over the frame).
    pub fn to_observation(&self, significant_only: bool) -> Observation {
        let mut observation =
            Observation::new(ObservationId::new("truth").expect("non-empty"), Timestamp(0));
        for (i, truth) in self.elements.iter().enumerate() {
            if significant_only && !truth.significant {
                continue;
            }
            observation.elements.push(Element {
                id: argus_protocol::ElementId::new(format!("t_{i}")).expect("non-empty"),
                role: truth.role,
                name: truth.name.clone(),
                value: truth.value.clone(),
                text: None,
                description: None,
                bounds: truth.bounds,
                visible_bounds: None,
                state: truth.state,
                confidence: Confidence::new(Score::new(1.0).expect("in range")),
                sources: Vec::new(),
                parent: None,
                children: Vec::new(),
            });
        }
        observation
    }
}

/// Roles that structure an interface rather than being part of it.
pub fn is_structural(role: Role) -> bool {
    matches!(
        role,
        Role::Window
            | Role::Dialog
            | Role::Group
            | Role::List
            | Role::Table
            | Role::Row
            | Role::Menu
            | Role::Unknown
    )
}

/// Loads every case under `root` (directories with a `case.json`), sorted
/// by name.
pub fn load_dataset(root: &Path) -> Result<Vec<Case>> {
    let mut cases = Vec::new();
    for entry in read_dir(root)? {
        if entry.join("case.json").is_file() {
            cases.push(load_case(&entry)?);
        }
    }
    if cases.is_empty() {
        return Err(Error::Dataset(format!("no cases in {}", root.display())));
    }
    Ok(cases)
}

/// Loads one case directory.
pub fn load_case(dir: &Path) -> Result<Case> {
    let info: CaseInfo = read_json(&dir.join("case.json"))?;
    let steps = read_dir(dir)?
        .into_iter()
        .filter(|path| path.join("frame.json").is_file())
        .map(|path| load_step(&path))
        .collect::<Result<Vec<_>>>()?;
    if steps.is_empty() {
        return Err(Error::Dataset(format!("case {} has no steps", dir.display())));
    }
    Ok(Case { name: file_name(dir), description: info.description, steps })
}

/// Loads one step directory.
pub fn load_step(dir: &Path) -> Result<Step> {
    let info: FrameInfo = read_json(&dir.join("frame.json"))?;
    let accessibility = match dir.join("accessibility.json") {
        path if path.is_file() => Some(read_json(&path)?),
        _ => None,
    };
    let truth: Truth = read_json(&dir.join("expected.json"))?;
    let pixels = read_png(&dir.join("frame.png"))?;
    let frame = Frame::new(FrameId(0), Timestamp(0), info.bounds, info.scale_factor, pixels)
        .map_err(|error| Error::Dataset(format!("{}: {error}", dir.display())))?;
    Ok(Step { name: file_name(dir), frame, info, accessibility, truth })
}

/// Writes a recorded step.
pub fn save_step(
    dir: &Path,
    frame: &Frame,
    info: &FrameInfo,
    accessibility: Option<&AxSnapshot>,
    truth: &Truth,
) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|error| io(dir, error))?;
    write_png(&dir.join("frame.png"), frame.pixels())?;
    write_json(&dir.join("frame.json"), info)?;
    if let Some(snapshot) = accessibility {
        write_json(&dir.join("accessibility.json"), snapshot)?;
    }
    write_json(&dir.join("expected.json"), truth)
}

/// A draft of the truth from an accessibility tree, to be reviewed: every
/// element Argus's accessibility adapter reports, with identities that
/// survive changes of the interface (names and native identifiers rather
/// than positions), and the reliable label relations.
pub fn draft_truth(snapshot: &AxSnapshot) -> Truth {
    let fused = fuse(&[normalize(candidates(snapshot))]);
    let native_ids = fused.native_ids();
    let observation =
        assemble(ObservationId::new("draft").expect("non-empty"), Timestamp(0), None, None, &fused);
    let window = observation.elements.first().map(|window| window.bounds);

    // Identities: parents precede children.
    let mut keys: HashMap<&str, String> = HashMap::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut siblings: HashMap<(Option<&str>, Role), usize> = HashMap::new();
    for (element, native) in observation.elements.iter().zip(&native_ids) {
        let parent = element.parent.as_ref().map(|id| id.as_str());
        let position = siblings.entry((parent, element.role)).or_default();
        *position += 1;
        let native = native.as_deref().map(|id| id.split(';').next().unwrap_or(id));
        let name = element.name.as_deref().filter(|name| !name.is_empty());
        let label = match element.role {
            Role::Text | Role::Cell => native.map(str::to_owned),
            _ => name.or(native).map(str::to_owned),
        }
        .unwrap_or_else(|| format!("#{position}"));
        let mut key = match parent.and_then(|parent| keys.get(parent)) {
            Some(parent) => format!("{parent}/{}:{label}", role_name(element.role)),
            None => role_name(element.role).to_owned(),
        };
        let count = seen.entry(key.clone()).or_default();
        *count += 1;
        if *count > 1 {
            key = format!("{key}~{count}");
        }
        keys.insert(element.id.as_str(), key);
    }

    let mut elements: Vec<TruthElement> = observation
        .elements
        .iter()
        .map(|element| {
            let visible = element.state.visible != Some(false)
                && element.bounds.width() * element.bounds.height() >= 16.0
                && window.is_none_or(|window| element.bounds.intersection(&window).is_some());
            TruthElement {
                id: keys[element.id.as_str()].clone(),
                role: element.role,
                name: element.name.clone(),
                value: element.value.clone(),
                bounds: element.visible_bounds.unwrap_or(element.bounds),
                state: element.state,
                parent: element.parent.as_ref().map(|id| keys[id.as_str()].clone()),
                significant: visible && !is_structural(element.role),
                note: None,
            }
        })
        .collect();

    // A cell or list item that is only the frame of its text: the text is
    // the element to find.
    let covered: Vec<bool> = elements
        .iter()
        .map(|outer| {
            matches!(outer.role, Role::Cell | Role::ListItem)
                && elements.iter().any(|inner| {
                    inner.significant
                        && inner.parent.as_deref() == Some(outer.id.as_str())
                        && crate::metrics::iou(&inner.bounds, &outer.bounds) >= 0.8
                })
        })
        .collect();
    for (element, covered) in elements.iter_mut().zip(covered) {
        if covered {
            element.significant = false;
        }
    }

    let relations = observation
        .relations
        .iter()
        .filter(|relation| relation.confidence.is_none_or(|score| score.get() >= 1.0))
        .filter(|relation| relation.kind == RelationKind::LabelFor)
        .map(|relation| TruthRelation {
            kind: relation.kind,
            from: keys[relation.from.as_str()].clone(),
            to: keys[relation.to.as_str()].clone(),
        })
        .collect();
    Truth { elements, relations }
}

pub(crate) fn role_name(role: Role) -> String {
    match serde_json::to_value(role) {
        Ok(serde_json::Value::String(name)) => name,
        _ => format!("{role:?}"),
    }
}

fn read_dir(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|error| io(dir, error))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir())
        .collect();
    paths.sort();
    Ok(paths)
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

fn io(path: &Path, error: std::io::Error) -> Error {
    Error::Dataset(format!("{}: {error}", path.display()))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path).map_err(|error| io(path, error))?;
    serde_json::from_str(&text)
        .map_err(|error| Error::Dataset(format!("{}: {error}", path.display())))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|error| Error::Dataset(format!("{}: {error}", path.display())))?;
    text.push('\n');
    std::fs::write(path, text).map_err(|error| io(path, error))
}

fn read_png(path: &Path) -> Result<PixelBuffer> {
    let file = File::open(path).map_err(|error| io(path, error))?;
    let bad = |error: String| Error::Dataset(format!("{}: {error}", path.display()));
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .map_err(|e| bad(e.to_string()))?;
    let mut data = vec![0; reader.output_buffer_size().unwrap_or_default()];
    let info = reader.next_frame(&mut data).map_err(|e| bad(e.to_string()))?;
    if (info.color_type, info.bit_depth) != (png::ColorType::Rgba, png::BitDepth::Eight) {
        return Err(bad("expected 8-bit RGBA".to_owned()));
    }
    data.truncate(info.buffer_size());
    PixelBuffer::new(info.width, info.height, data).map_err(|e| bad(e.to_string()))
}

fn write_png(path: &Path, pixels: &PixelBuffer) -> Result<()> {
    let bad = |error: String| Error::Dataset(format!("{}: {error}", path.display()));
    let file = File::create(path).map_err(|error| io(path, error))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), pixels.width(), pixels.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::High);
    let mut writer = encoder.write_header().map_err(|e| bad(e.to_string()))?;
    writer.write_image_data(pixels.as_bytes()).map_err(|e| bad(e.to_string()))?;
    writer.finish().map_err(|e| bad(e.to_string()))
}
