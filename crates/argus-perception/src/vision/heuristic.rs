//! A deterministic, model-free UI detector.
//!
//! 1. The frame is reduced to one pixel per point ([`Raster`]).
//! 2. An edge map marks color changes; flat UI surfaces have none.
//! 3. Connected edge components are measured: closed outlines (boxes,
//!    rings) are controls, compact open shapes may be icons.
//! 4. Straight edge runs are paired into rectangles, which finds boxes that
//!    touch or nest (stacked text fields, tabs on a group border) and
//!    therefore merge into one component.
//! 5. Shapes are classified by size, proportions and content; everything
//!    inside a detected control (labels, check marks) is its content and is
//!    not reported separately.
//!
//! All sizes are in points. Confidence values are deliberately modest: these
//! are hypotheses to be confirmed or rejected by other sources.

use std::collections::VecDeque;

use argus_protocol::{CheckState, Confidence, ElementState, Frame, PixelRect, Role, Score};

use super::raster::Raster;
use super::{VisualCandidate, VisualPerceptionBackend};
use crate::Result;

/// Model-free UI detector based on outlines and shapes.
#[derive(Debug, Clone)]
pub struct HeuristicDetector {
    /// Minimum per-channel color difference that counts as an edge.
    pub edge_threshold: u8,
}

impl Default for HeuristicDetector {
    fn default() -> Self {
        // Flat UI themes separate controls from backgrounds by very small
        // contrasts (light-gray fills on light-gray windows).
        Self { edge_threshold: 6 }
    }
}

impl VisualPerceptionBackend for HeuristicDetector {
    fn detect(&self, frame: &Frame) -> Result<Vec<VisualCandidate>> {
        let raster = Raster::from_frame(frame);
        let edges = raster.edges(self.edge_threshold);
        let map = ComponentMap::new(&edges, raster.width, raster.height);

        let mut detections: Vec<Detection> = map
            .components
            .iter()
            .filter_map(|component| classify(&map, &edges, component))
            .collect();
        for rect in line_rectangles(&edges, raster.width, raster.height) {
            let duplicate = detections.iter().any(|known| overlap(&known.bounds, &rect) > 0.6);
            if !duplicate {
                detections.push(classify_bar(&map, &edges, rect, ComponentMap::NONE));
            }
        }
        resolve_segmented(&mut detections);
        split_groups(&edges, raster.width, &mut detections);
        suppress_contained(&mut detections);
        for detection in &mut detections {
            refine_toggle(frame, raster.factor, detection);
        }

        let factor = raster.factor as f32;
        Ok(detections.into_iter().map(|detection| detection.into_candidate(factor)).collect())
    }
}

/// Bounding box in raster pixels (inclusive-exclusive).
#[derive(Debug, Clone, Copy, PartialEq)]
struct BoxPx {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl BoxPx {
    fn width(&self) -> usize {
        self.x1 - self.x0
    }

    fn height(&self) -> usize {
        self.y1 - self.y0
    }

    fn contains(&self, other: &BoxPx) -> bool {
        self.x0 <= other.x0 && self.y0 <= other.y0 && other.x1 <= self.x1 && other.y1 <= self.y1
    }

    fn inset(&self, fraction: f32) -> BoxPx {
        let dx = (self.width() as f32 * fraction) as usize;
        let dy = (self.height() as f32 * fraction) as usize;
        BoxPx { x0: self.x0 + dx, y0: self.y0 + dy, x1: self.x1 - dx, y1: self.y1 - dy }
    }
}

#[derive(Debug)]
struct Component {
    label: u32,
    bounds: BoxPx,
}

/// Connected components (8-neighborhood) of the edge map.
struct ComponentMap {
    width: usize,
    height: usize,
    labels: Vec<u32>,
    components: Vec<Component>,
}

impl ComponentMap {
    const NONE: u32 = u32::MAX;

    fn new(edges: &[bool], width: usize, height: usize) -> Self {
        let mut labels = vec![Self::NONE; edges.len()];
        let mut components = Vec::new();
        let mut queue = VecDeque::new();
        for start in 0..edges.len() {
            if !edges[start] || labels[start] != Self::NONE {
                continue;
            }
            let label = components.len() as u32;
            let (x, y) = (start % width, start / width);
            let mut bounds = BoxPx { x0: x, y0: y, x1: x + 1, y1: y + 1 };
            labels[start] = label;
            queue.push_back(start);
            while let Some(index) = queue.pop_front() {
                let (x, y) = (index % width, index / width);
                bounds.x0 = bounds.x0.min(x);
                bounds.y0 = bounds.y0.min(y);
                bounds.x1 = bounds.x1.max(x + 1);
                bounds.y1 = bounds.y1.max(y + 1);
                for ny in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                    for nx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                        let neighbor = ny * width + nx;
                        if edges[neighbor] && labels[neighbor] == Self::NONE {
                            labels[neighbor] = label;
                            queue.push_back(neighbor);
                        }
                    }
                }
            }
            components.push(Component { label, bounds });
        }
        Self { width, height, labels, components }
    }

    fn is(&self, x: usize, y: usize, label: u32) -> bool {
        x < self.width && y < self.height && self.labels[y * self.width + x] == label
    }

    /// Whether the component has a pixel within `radius` of `(x, y)`.
    fn near(&self, x: f32, y: f32, radius: f32, label: u32) -> bool {
        let r = radius.ceil() as isize;
        let (cx, cy) = (x.round() as isize, y.round() as isize);
        (-r..=r).any(|dy| {
            (-r..=r).any(|dx| {
                let (px, py) = (cx + dx, cy + dy);
                px >= 0 && py >= 0 && self.is(px as usize, py as usize, label)
            })
        })
    }

    /// How completely the component outlines its bounding box: the minimum,
    /// over the four sides, of the covered fraction of the side's middle
    /// part (corners are excluded because they may be rounded).
    fn box_score(&self, component: &Component) -> f32 {
        const DEPTH: usize = 2;
        let b = component.bounds;
        let side = |len: usize, covered: &dyn Fn(usize) -> bool| {
            let margin = len * 3 / 10;
            let range = margin..len - margin;
            let total = range.len().max(1);
            range.filter(|&i| covered(i)).count() as f32 / total as f32
        };
        let band = |x: usize, y: usize, dx: isize, dy: isize| {
            (0..DEPTH as isize).any(|d| {
                let (px, py) = (x as isize + dx * d, y as isize + dy * d);
                px >= 0 && py >= 0 && self.is(px as usize, py as usize, component.label)
            })
        };
        let top = side(b.width(), &|i| band(b.x0 + i, b.y0, 0, 1));
        let bottom = side(b.width(), &|i| band(b.x0 + i, b.y1 - 1, 0, -1));
        let left = side(b.height(), &|i| band(b.x0, b.y0 + i, 1, 0));
        let right = side(b.height(), &|i| band(b.x1 - 1, b.y0 + i, -1, 0));
        top.min(bottom).min(left).min(right)
    }

    /// Outline pixels per point of bounding-box perimeter: about 1–2 for a
    /// plain outline, more for detailed shapes (glyphs, pictograms).
    fn complexity(&self, component: &Component) -> f32 {
        let b = component.bounds;
        let count = (b.y0..b.y1)
            .flat_map(|y| (b.x0..b.x1).map(move |x| (x, y)))
            .filter(|&(x, y)| self.is(x, y, component.label))
            .count();
        count as f32 / (2 * (b.width() + b.height())) as f32
    }

    /// Components lying entirely inside `area`, other than `own`.
    fn inside(&self, area: BoxPx, own: u32) -> impl Iterator<Item = &Component> {
        self.components
            .iter()
            .filter(move |component| component.label != own && area.contains(&component.bounds))
    }

    /// Fraction of points on the ellipse inscribed in the bounding box that
    /// the component covers.
    fn ring_score(&self, component: &Component) -> f32 {
        const SAMPLES: usize = 36;
        let b = component.bounds;
        let (cx, cy) = ((b.x0 + b.x1) as f32 / 2.0 - 0.5, (b.y0 + b.y1) as f32 / 2.0 - 0.5);
        let (rx, ry) = (b.width() as f32 / 2.0 - 0.5, b.height() as f32 / 2.0 - 0.5);
        let hits = (0..SAMPLES)
            .filter(|&i| {
                let angle = i as f32 / SAMPLES as f32 * std::f32::consts::TAU;
                self.near(cx + rx * angle.cos(), cy + ry * angle.sin(), 1.5, component.label)
            })
            .count();
        hits as f32 / SAMPLES as f32
    }

    /// Whether the component is round: walking diagonally inward from each
    /// corner of the bounding box, a circle is first met after ~0.146 of its
    /// size, a rounded square after ~0.3 of its corner radius.
    fn corners_empty(&self, component: &Component) -> bool {
        let b = component.bounds;
        let size = b.width().min(b.height());
        let limit = size / 2;
        let first_hit = |x: usize, y: usize, dx: isize, dy: isize| {
            (0..limit)
                .find(|&i| {
                    let (px, py) = (x as isize + dx * i as isize, y as isize + dy * i as isize);
                    // Allow one pixel of slack around the diagonal.
                    [(0, 0), (dx, 0), (0, dy)].iter().any(|(ox, oy)| {
                        let (qx, qy) = (px + ox, py + oy);
                        qx >= 0 && qy >= 0 && self.is(qx as usize, qy as usize, component.label)
                    })
                })
                .unwrap_or(limit)
        };
        let total = first_hit(b.x0, b.y0, 1, 1)
            + first_hit(b.x1 - 1, b.y0, -1, 1)
            + first_hit(b.x0, b.y1 - 1, 1, -1)
            + first_hit(b.x1 - 1, b.y1 - 1, -1, -1);
        total as f32 / 4.0 >= size as f32 * 0.1
    }
}

/// All edge pixels inside `area`, including those of the outline itself (a
/// check mark may be drawn across a checkbox's border and merge with it).
fn marks(map: &ComponentMap, edges: &[bool], area: BoxPx) -> f32 {
    let count = (area.y0..area.y1)
        .flat_map(|y| (area.x0..area.x1).map(move |x| y * map.width + x))
        .filter(|&index| edges[index])
        .count();
    count as f32 / (area.width() * area.height()).max(1) as f32
}

/// Edge pixels of *other* components inside `area` (content of a control).
fn content(map: &ComponentMap, edges: &[bool], area: BoxPx, own: u32) -> Content {
    let mut count = 0usize;
    let (mut min_x, mut max_x, mut sum_x) = (usize::MAX, 0usize, 0usize);
    for y in area.y0..area.y1 {
        for x in area.x0..area.x1 {
            let index = y * map.width + x;
            if edges[index] && map.labels[index] != own {
                count += 1;
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                sum_x += x;
            }
        }
    }
    Content {
        count,
        area: (area.width() * area.height()).max(1),
        min_x,
        max_x,
        mean_x: if count == 0 { 0.0 } else { sum_x as f32 / count as f32 },
    }
}

struct Content {
    count: usize,
    area: usize,
    min_x: usize,
    max_x: usize,
    mean_x: f32,
}

impl Content {
    fn density(&self) -> f32 {
        self.count as f32 / self.area as f32
    }
}

#[derive(Debug, Clone)]
struct Detection {
    bounds: BoxPx,
    role: Role,
    role_confidence: f32,
    element_confidence: f32,
    checked: Option<(CheckState, f32)>,
    /// Confidence that the element is the selected one (tabs).
    selected: Option<f32>,
    /// Leaf controls own everything drawn inside them.
    leaf: bool,
}

impl Detection {
    fn new(bounds: BoxPx, role: Role, element: f32, role_confidence: f32) -> Self {
        Self {
            bounds,
            role,
            role_confidence,
            element_confidence: element,
            checked: None,
            selected: None,
            leaf: true,
        }
    }

    fn into_candidate(self, factor: f32) -> VisualCandidate {
        // Edges mark the pixel *before* a color transition, so outlines start
        // one pixel early on the left and top.
        let (x0, y0) = (self.bounds.x0 + 1, self.bounds.y0 + 1);
        let score = |value: f32| Score::new(value).ok();
        let (state, state_confidence) = match self.checked {
            Some((checked, confidence)) => (
                ElementState { checked: Some(checked), ..ElementState::default() },
                score(confidence),
            ),
            None => (ElementState::default(), None),
        };
        VisualCandidate {
            role: self.role,
            rect: PixelRect {
                x: x0 as f32 * factor,
                y: y0 as f32 * factor,
                width: self.bounds.x1.saturating_sub(x0) as f32 * factor,
                height: self.bounds.y1.saturating_sub(y0) as f32 * factor,
            },
            state,
            confidence: Confidence {
                role: score(self.role_confidence).filter(|_| self.role != Role::Unknown),
                state: state_confidence,
                ..Confidence::new(score(self.element_confidence).unwrap_or(Score::CERTAIN))
            },
        }
    }
}

/// Size limits, in points.
const TOGGLE: std::ops::RangeInclusive<usize> = 10..=22;
const ROUND_BUTTON: std::ops::RangeInclusive<usize> = 24..=80;
const BAR_HEIGHT: std::ops::RangeInclusive<usize> = 16..=44;
const ICON: std::ops::RangeInclusive<usize> = 14..=40;
const CLOSED: f32 = 0.8;

fn classify(map: &ComponentMap, edges: &[bool], component: &Component) -> Option<Detection> {
    let b = component.bounds;
    let (w, h) = (b.width(), b.height());
    if w < 6 || h < 6 {
        return None;
    }
    // Shapes touching the frame edge are window chrome (rounded window
    // corners, borders) or controls cut off by the capture; neither can be
    // grounded as a whole control.
    if b.x0 <= 1 || b.y0 <= 1 || b.x1 + 1 >= map.width || b.y1 + 1 >= map.height {
        return None;
    }
    let aspect = w as f32 / h as f32;
    let square = (0.8..=1.25).contains(&aspect);

    let boxy = map.box_score(component) >= CLOSED;
    let ring = map.ring_score(component) >= CLOSED;

    // Toggles: a plain outline following the whole bounding box (squares and
    // circles alike cover the middle of every side).
    let box_score = map.box_score(component);
    let plain = map.complexity(component) <= 2.4;
    if square && box_score >= 0.95 && plain && TOGGLE.contains(&w) && TOGGLE.contains(&h) {
        let round = map.corners_empty(component);
        let checked = if marks(map, edges, b.inset(0.25)) > 0.08 {
            CheckState::Checked
        } else {
            CheckState::Unchecked
        };
        let role = if round { Role::RadioButton } else { Role::Checkbox };
        let mut detection = Detection::new(b, role, 0.6, 0.5);
        detection.checked = Some((checked, 0.45));
        return Some(detection);
    }

    if ring && square && ROUND_BUTTON.contains(&w) && map.corners_empty(component) {
        return Some(Detection::new(b, Role::Button, 0.55, 0.45));
    }

    if (boxy || ring) && BAR_HEIGHT.contains(&h) && aspect >= 1.5 && w <= 800 {
        return Some(classify_bar(map, edges, b, component.label));
    }

    if boxy && w >= 64 && h >= 48 {
        // A large outlined area: a picture if busy inside, otherwise a
        // container, which is not reported.
        let inner = content(map, edges, b.inset(0.1), component.label);
        if inner.density() > 0.25 {
            let mut detection = Detection::new(b, Role::Image, 0.35, 0.3);
            detection.leaf = false;
            return Some(detection);
        }
        return None;
    }

    // Compact pictograms. Glyph clusters of text can look alike; OCR evidence
    // settles that during fusion, hence the low confidence.
    if ICON.contains(&w) && ICON.contains(&h) && (0.6..=1.6).contains(&aspect) {
        return Some(Detection::new(b, Role::Icon, 0.3, 0.3));
    }

    if boxy && w >= 16 && h >= 16 && w <= 400 && h <= 44 {
        return Some(Detection::new(b, Role::Unknown, 0.35, 0.0));
    }
    None
}

/// Classifies a closed, bar-shaped outline (text field, push button,
/// pop-up button or segmented control) by what is drawn inside it. `own` is
/// the outline's component label, if the outline is one component.
fn classify_bar(map: &ComponentMap, edges: &[bool], b: BoxPx, own: u32) -> Detection {
    let (w, h) = (b.width(), b.height());
    let interior = b.inset(0.1);

    // Segmented controls contain separators or a highlighted segment.
    let segmented = map.inside(b, own).any(|inner| {
        let (iw, ih) = (inner.bounds.width(), inner.bounds.height());
        let separator = iw <= 2 && ih * 2 >= h;
        let segment = ih * 10 >= h * 8 && iw * 10 <= w * 6 && map.box_score(inner) >= CLOSED;
        separator || segment
    });
    if segmented {
        return Detection::new(b, Role::Tab, 0.45, 0.35);
    }

    let inner = content(map, edges, interior, own);
    if inner.count == 0 {
        // An empty field; an unlabeled button would be unusual.
        return Detection::new(b, Role::TextBox, 0.5, 0.4);
    }
    let margin = w / 10;
    let middle = (b.x0 + b.x1) as f32 / 2.0;
    let (left_gap, right_gap) = (inner.min_x - b.x0, b.x1.saturating_sub(inner.max_x + 1));
    // A label with (nearly) equal space on both sides, however long.
    let centered = (inner.mean_x - middle).abs() < w as f32 * 0.12
        && left_gap.abs_diff(right_gap) <= (w / 20).max(3)
        && left_gap >= 3;
    if centered {
        return Detection::new(b, Role::Button, 0.55, 0.5);
    }
    // Text at the start and a marker (chevron) at the very end: a pop-up.
    let starts_left = inner.min_x < interior.x0 + margin;
    let marker_at_end = inner.max_x + h / 2 >= interior.x1 && w > 3 * h;
    let gap_before_marker = content(
        map,
        edges,
        BoxPx { x0: interior.x1 - h, y0: interior.y0, x1: interior.x1 - h / 2, y1: interior.y1 },
        own,
    )
    .count
        == 0;
    if starts_left && marker_at_end && gap_before_marker {
        return Detection::new(b, Role::Button, 0.5, 0.4);
    }
    Detection::new(b, Role::TextBox, 0.5, 0.4)
}

/// Segmented controls: a bar holding a same-height box at one of its ends
/// (the selected segment). Both become tabs; the inner one is selected.
fn resolve_segmented(detections: &mut [Detection]) {
    let bars: Vec<usize> = (0..detections.len())
        .filter(|&i| matches!(detections[i].role, Role::TextBox | Role::Button | Role::Tab))
        .collect();
    for &outer in &bars {
        for &inner in &bars {
            let (o, i) = (detections[outer].bounds, detections[inner].bounds);
            let same_height = o.height().abs_diff(i.height()) <= 6;
            let narrower = i.width() * 10 <= o.width() * 7;
            let at_end = i.x0.abs_diff(o.x0) <= 4 || i.x1.abs_diff(o.x1) <= 4;
            let within =
                i.x0 + 3 >= o.x0 && i.x1 <= o.x1 + 3 && i.y0 + 3 >= o.y0 && i.y1 <= o.y1 + 3;
            if outer != inner && same_height && narrower && at_end && within {
                for (index, confidence) in [(outer, 0.35), (inner, 0.35)] {
                    detections[index].role = Role::Tab;
                    detections[index].role_confidence = confidence;
                }
                detections[outer].leaf = false;
                detections[inner].selected = Some(0.35);
            }
        }
    }
}

/// Decides round versus square for small toggles on the full-resolution
/// frame, where the difference is several pixels, and recognizes macOS
/// window buttons (three small circles at the top-left of a window).
fn refine_toggle(frame: &Frame, factor: usize, detection: &mut Detection) {
    if !matches!(detection.role, Role::Checkbox | Role::RadioButton) {
        return;
    }
    let b = detection.bounds;
    let pixels = frame.pixels();
    // The raster box starts one raster pixel before the shape on the left
    // and top (edges mark the pixel before a transition); that pixel is
    // background.
    let (outside_x, outside_y) = (b.x0 * factor, b.y0 * factor);
    let (x0, y0, x1, y1) = ((b.x0 + 1) * factor, (b.y0 + 1) * factor, b.x1 * factor, b.y1 * factor);
    let (Some(background), true) = (
        pixels.pixel(outside_x as u32, outside_y as u32),
        x1 <= pixels.width() as usize && y1 <= pixels.height() as usize,
    ) else {
        return;
    };
    let shape = |x: usize, y: usize| {
        pixels
            .pixel(x as u32, y as u32)
            .is_some_and(|p| p.iter().zip(background).take(3).any(|(a, b)| a.abs_diff(b) > 12))
    };
    let size = (x1 - x0).min(y1 - y0);
    let gap = |cx: usize, cy: usize, dx: isize, dy: isize| {
        (0..size / 2)
            .find(|&i| {
                let (x, y) = (cx as isize + dx * i as isize, cy as isize + dy * i as isize);
                x >= 0 && y >= 0 && shape(x as usize, y as usize)
            })
            .unwrap_or(size / 2)
    };
    let average = (gap(x0, y0, 1, 1)
        + gap(x1 - 1, y0, -1, 1)
        + gap(x0, y1 - 1, 1, -1)
        + gap(x1 - 1, y1 - 1, -1, -1)) as f32
        / 4.0;
    // Inscribed circle: ~0.146 of the size; rounded squares measure ~0.1.
    let round = average >= size as f32 * 0.125;
    tracing::trace!(x0, y0, size, ratio = average / size as f32, round, "toggle shape");

    let window_button = round && b.y0 <= 24 && b.x1 <= 90 && b.width() <= 16;
    if window_button {
        detection.role = Role::Button;
        detection.checked = None;
    } else {
        detection.role = if round { Role::RadioButton } else { Role::Checkbox };
    }
}

/// Toolbars group several borderless buttons (or segments) in one outline.
/// When the content of a bar splits into clusters separated by more than
/// word spacing, each cluster becomes its own element and the bar becomes a
/// group. Thin vertical clusters are separators, not elements.
fn split_groups(edges: &[bool], width: usize, detections: &mut Vec<Detection>) {
    let mut parts = Vec::new();
    for detection in detections.iter_mut() {
        if !matches!(detection.role, Role::Button | Role::Tab) || detection.selected.is_some() {
            continue;
        }
        let b = detection.bounds;
        let (h, inset) = (b.height(), (b.height() / 5).max(2));
        let (y0, y1) = (b.y0 + inset, b.y1.saturating_sub(inset));
        let (x0, x1) = (b.x0 + inset, b.x1.saturating_sub(inset));
        if y1 <= y0 || x1 <= x0 {
            continue;
        }
        // Columns holding content, then runs of such columns.
        let filled: Vec<bool> = (x0..x1).map(|x| (y0..y1).any(|y| edges[y * width + x])).collect();
        let mut clusters: Vec<(usize, usize)> = Vec::new();
        let mut start = None;
        for (i, &on) in filled.iter().chain(std::iter::once(&false)).enumerate() {
            match (on, start) {
                (true, None) => start = Some(i),
                (false, Some(s)) => {
                    clusters.push((x0 + s, x0 + i));
                    start = None;
                }
                _ => {}
            }
        }
        // Merge clusters closer than word spacing; drop separators.
        let min_gap = (h * 45 / 100).max(6);
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (s, e) in clusters {
            match merged.last_mut() {
                Some(last) if s - last.1 < min_gap && e - s > 2 && last.1 - last.0 > 2 => {
                    last.1 = e
                }
                _ => merged.push((s, e)),
            }
        }
        merged.retain(|(s, e)| e - s > 2);
        // Buttons split only into icon-sized parts; a label next to a marker
        // (a pop-up's chevron) is one control.
        let icon_sized = merged.iter().all(|(s, e)| e - s <= h * 3 / 2);
        if merged.len() < 2 || (detection.role == Role::Button && !icon_sized) {
            continue;
        }
        // Each part spans from the midpoint to its neighbors.
        let role = detection.role;
        for (index, &(s, e)) in merged.iter().enumerate() {
            let left = if index == 0 { b.x0 } else { (merged[index - 1].1 + s) / 2 };
            let right =
                if index + 1 == merged.len() { b.x1 } else { (e + merged[index + 1].0) / 2 };
            let bounds = BoxPx { x0: left, y0: b.y0, x1: right, y1: b.y1 };
            parts.push(Detection::new(bounds, role, 0.45, 0.4));
        }
        detection.role = Role::Group;
        detection.leaf = false;
    }
    detections.extend(parts);
}

/// Intersection over union of two boxes.
fn overlap(a: &BoxPx, b: &BoxPx) -> f32 {
    let w = a.x1.min(b.x1).saturating_sub(a.x0.max(b.x0));
    let h = a.y1.min(b.y1).saturating_sub(a.y0.max(b.y0));
    let intersection = (w * h) as f32;
    let area = |r: &BoxPx| (r.width() * r.height()) as f32;
    intersection / (area(a) + area(b) - intersection).max(1.0)
}

/// A straight run of edge pixels: `start..end` along one axis at `at`.
#[derive(Debug, Clone, Copy)]
struct Run {
    start: usize,
    end: usize,
    at: usize,
}

impl Run {
    fn len(&self) -> usize {
        self.end - self.start
    }
}

/// Maximal runs of edge pixels along rows (`horizontal`) or columns,
/// bridging single-pixel gaps, at least `min_len` long.
fn runs(edges: &[bool], width: usize, height: usize, horizontal: bool, min_len: usize) -> Vec<Run> {
    let (lines, length) = if horizontal { (height, width) } else { (width, height) };
    let at = |line: usize, i: usize| {
        if horizontal { edges[line * width + i] } else { edges[i * width + line] }
    };
    let mut runs = Vec::new();
    for line in 0..lines {
        let mut i = 0;
        while i < length {
            if !at(line, i) {
                i += 1;
                continue;
            }
            let start = i;
            let mut end = i + 1;
            while end < length && (at(line, end) || (end + 1 < length && at(line, end + 1))) {
                end += 1;
            }
            if end - start >= min_len {
                runs.push(Run { start, end, at: line });
            }
            i = end;
        }
    }
    runs
}

/// Bar-shaped rectangles assembled from straight edges: a top and a bottom
/// run of about the same extent, confirmed by vertical runs near both ends.
/// Rounded corners are tolerated (the sides may start a few points inward).
fn line_rectangles(edges: &[bool], width: usize, height: usize) -> Vec<BoxPx> {
    const MAX_RADIUS: usize = 8;
    let horizontal = runs(edges, width, height, true, 24);
    let vertical = runs(edges, width, height, false, 6);

    let has_side = |x_near: usize, left: bool, y0: usize, y1: usize| -> Option<usize> {
        let need = (y1 - y0) / 2;
        vertical
            .iter()
            .filter(|v| {
                let x_ok = if left {
                    v.at + MAX_RADIUS >= x_near && v.at <= x_near + 2
                } else {
                    v.at + 2 >= x_near && v.at <= x_near + MAX_RADIUS
                };
                let covered = v.end.min(y1).saturating_sub(v.start.max(y0));
                x_ok && covered >= need
            })
            .map(|v| v.at)
            .reduce(|a, b| if left { a.min(b) } else { a.max(b) })
    };

    let mut found: Vec<BoxPx> = Vec::new();
    for (i, top) in horizontal.iter().enumerate() {
        for bottom in &horizontal[i + 1..] {
            let gap = bottom.at - top.at;
            if gap > *BAR_HEIGHT.end() {
                break; // runs are sorted by row
            }
            if gap < *BAR_HEIGHT.start() {
                continue;
            }
            let x0 = top.start.max(bottom.start);
            let x1 = top.end.min(bottom.end);
            if x1 <= x0 || (x1 - x0) * 10 < top.len().max(bottom.len()) * 9 {
                continue; // not the same extent
            }
            let (Some(left), Some(right)) =
                (has_side(x0, true, top.at, bottom.at), has_side(x1 - 1, false, top.at, bottom.at))
            else {
                continue;
            };
            let rect = BoxPx { x0: left, y0: top.at, x1: right + 1, y1: bottom.at + 1 };
            let aspect = rect.width() as f32 / rect.height() as f32;
            if aspect >= 1.5
                && rect.width() <= 800
                && !found.iter().any(|f| overlap(f, &rect) > 0.6)
            {
                found.push(rect);
            }
        }
    }
    found
}

/// Removes detections inside leaf controls (their labels, check marks and
/// focus rings) and duplicates of the same outline.
fn suppress_contained(detections: &mut Vec<Detection>) {
    // Groups own nothing: their parts are reported individually.
    detections.retain(|detection| detection.role != Role::Group || !detection.leaf);
    detections.sort_by_key(|detection| {
        std::cmp::Reverse(detection.bounds.width() * detection.bounds.height())
    });
    let mut kept: Vec<Detection> = Vec::with_capacity(detections.len());
    for detection in detections.drain(..) {
        let inside_leaf = kept
            .iter()
            .any(|outer| outer.leaf && outer.bounds.contains(&detection.bounds.inset(0.0)));
        if !inside_leaf {
            kept.push(detection);
        }
    }
    kept.sort_by_key(|detection| (detection.bounds.y0, detection.bounds.x0));
    *detections = kept;
}

#[cfg(test)]
mod tests {
    use argus_protocol::{Bounds, FrameId, PixelBuffer, Timestamp};

    use super::*;

    /// A 1x frame with a background color and shapes drawn by `paint`.
    fn frame(width: u32, height: u32, paint: impl Fn(u32, u32) -> Option<[u8; 3]>) -> Frame {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let [r, g, b] = paint(x, y).unwrap_or([236, 236, 236]);
                data.extend_from_slice(&[r, g, b, 255]);
            }
        }
        let pixels = PixelBuffer::new(width, height, data).unwrap();
        let bounds = Bounds::new(0.0, 0.0, width as f32, height as f32).unwrap();
        Frame::new(FrameId(1), Timestamp(0), bounds, 1.0, pixels).unwrap()
    }

    fn outline(x: u32, y: u32, x0: u32, y0: u32, w: u32, h: u32) -> bool {
        let inside = (x0..x0 + w).contains(&x) && (y0..y0 + h).contains(&y);
        inside && (x == x0 || y == y0 || x == x0 + w - 1 || y == y0 + h - 1)
    }

    fn detect(frame: &Frame) -> Vec<VisualCandidate> {
        HeuristicDetector::default().detect(frame).unwrap()
    }

    #[test]
    fn flat_background_has_no_elements() {
        assert!(detect(&frame(120, 80, |_, _| None)).is_empty());
    }

    #[test]
    fn empty_wide_box_is_a_text_box() {
        let detections = detect(&frame(200, 60, |x, y| {
            outline(x, y, 10, 10, 160, 24).then_some([180, 180, 180])
        }));
        let [field] = detections.as_slice() else { panic!("{detections:#?}") };
        assert_eq!(field.role, Role::TextBox);
        assert_eq!(field.rect, PixelRect { x: 10.0, y: 10.0, width: 160.0, height: 24.0 });
        assert!(field.confidence.role.unwrap().get() < 0.6, "a guess must stay a guess");
    }

    #[test]
    fn box_with_centered_label_is_a_button() {
        let detections = detect(&frame(200, 60, |x, y| {
            let label = (80..100).contains(&x) && (18..26).contains(&y) && (x + y) % 3 == 0;
            (outline(x, y, 10, 10, 160, 24) || label).then_some([90, 90, 90])
        }));
        let [button] = detections.as_slice() else { panic!("{detections:#?}") };
        assert_eq!(button.role, Role::Button);
    }

    #[test]
    fn small_square_is_a_checkbox_and_its_mark_sets_the_state() {
        let detections = detect(&frame(60, 40, |x, y| {
            let mark = (10..14).contains(&x) && (10..14).contains(&y);
            (outline(x, y, 5, 5, 14, 14) || mark).then_some([60, 60, 60])
        }));
        let [checkbox] = detections.as_slice() else { panic!("{detections:#?}") };
        assert_eq!(checkbox.role, Role::Checkbox);
        assert_eq!(checkbox.state.checked, Some(CheckState::Checked));
    }

    #[test]
    fn small_circle_is_a_radio_button() {
        let detections = detect(&frame(60, 40, |x, y| {
            let (dx, dy) = (x as f32 - 14.5, y as f32 - 14.5);
            let distance = (dx * dx + dy * dy).sqrt();
            (7.0..8.2).contains(&distance).then_some([60, 60, 60])
        }));
        let [radio] = detections.as_slice() else { panic!("{detections:#?}") };
        assert_eq!(radio.role, Role::RadioButton);
        assert_eq!(radio.state.checked, Some(CheckState::Unchecked));
    }

    #[test]
    fn toolbar_groups_split_into_individual_buttons() {
        // A pill holding two icon glyphs far apart (like Share | Tags).
        let detections = detect(&frame(200, 60, |x, y| {
            let icon =
                |x0: u32| (x0..x0 + 12).contains(&x) && (16..28).contains(&y) && (x + y) % 2 == 0;
            (outline(x, y, 10, 10, 90, 24) || icon(24) || icon(70)).then_some([90, 90, 90])
        }));
        let roles: Vec<Role> = detections.iter().map(|d| d.role).collect();
        assert_eq!(roles.iter().filter(|r| **r == Role::Button).count(), 2, "{detections:#?}");
        assert!(roles.contains(&Role::Group));
        let buttons: Vec<_> = detections.iter().filter(|d| d.role == Role::Button).collect();
        assert!(buttons[0].rect.x + buttons[0].rect.width <= buttons[1].rect.x + 1.0);
    }

    #[test]
    fn retina_frames_are_detected_in_points_and_reported_in_pixels() {
        let pixels =
            |x: u32, y: u32| outline(x / 2, y / 2, 10, 10, 160, 24).then_some([180, 180, 180]);
        let mut data = Vec::new();
        for y in 0..120 {
            for x in 0..400 {
                let [r, g, b] = pixels(x, y).unwrap_or([236, 236, 236]);
                data.extend_from_slice(&[r, g, b, 255]);
            }
        }
        let buffer = PixelBuffer::new(400, 120, data).unwrap();
        let bounds = Bounds::new(0.0, 0.0, 200.0, 60.0).unwrap();
        let frame = Frame::new(FrameId(1), Timestamp(0), bounds, 2.0, buffer).unwrap();

        let detections = detect(&frame);
        let [field] = detections.as_slice() else { panic!("{detections:#?}") };
        assert_eq!(field.rect, PixelRect { x: 20.0, y: 20.0, width: 320.0, height: 48.0 });
    }
}
