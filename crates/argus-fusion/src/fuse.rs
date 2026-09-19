//! The fusion algorithm. The policy is described in the crate documentation.

use std::collections::HashMap;

use argus_protocol::{
    Bounds, CandidateId, Confidence, ElementCandidate, ElementState, RelationKind, Role, Score,
    Source,
};

use crate::evidence::{Claim, Conflict, Contribution, ElementEvidence, Link, Property};
use crate::geometry::{area, cover, iou};
use crate::order::{lines, reading_order, single_line};
use crate::roles::{carries_text, compatible, is_checkable, is_control, is_detail, named_by_text};
use crate::text::similar;

mod scene;

/// A text line inside an element at least this much is about the element.
const TEXT_INSIDE: f32 = 0.7;
/// A text line overlapping an element this much is about the element.
const TEXT_OVERLAP: f32 = 0.5;
/// Boxes of the same object with compatible roles overlap at least this much.
const SAME_IOU: f32 = 0.5;
/// Boxes of the same object with incompatible roles overlap at least this
/// much.
const SAME_IOU_CONFLICTING: f32 = 0.75;
/// A detection lies inside an element when this much of it is inside.
const INSIDE: f32 = 0.9;
/// A glyph lies on a text when this much of it is inside the text.
const GLYPH_ON_TEXT: f32 = 0.7;
/// A pixel-derived element belongs to a new element when this much of it is
/// inside.
const ADOPT: f32 = 0.8;
/// Highest confidence of a field's value read from its pixels: the text may
/// be a placeholder rather than the contents.
const FIELD_TEXT: f32 = 0.7;

/// The result of fusion: elements in output order (parents before children,
/// siblings in reading order).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fusion {
    /// Fused elements.
    pub elements: Vec<FusedElement>,
    /// Relations between fused elements.
    pub relations: Vec<FusedRelation>,
}

/// One element fused from the evidence of one or more sources.
#[derive(Debug, Clone, PartialEq)]
pub struct FusedElement {
    /// Role.
    pub role: Role,
    /// Name or visible label.
    pub name: Option<String>,
    /// Value.
    pub value: Option<String>,
    /// Description.
    pub description: Option<String>,
    /// Full extent.
    pub bounds: Bounds,
    /// Visible part of `bounds`, when only partially visible.
    pub visible_bounds: Option<Bounds>,
    /// State.
    pub state: ElementState,
    /// Property-level confidence after fusion.
    pub confidence: Confidence,
    /// Contributing sources, sorted and deduplicated.
    pub sources: Vec<Source>,
    /// Index of the structural parent in [`Fusion::elements`]; always lower
    /// than the element's own index.
    pub parent: Option<usize>,
    /// How the element came to be.
    pub evidence: ElementEvidence,
    /// Identifier the application or platform gave the object the element
    /// was created from (e.g. `AXIdentifier`), for tracking. Never exposed in
    /// observations.
    pub native_id: Option<String>,
}

/// A relation between two fused elements (indices into
/// [`Fusion::elements`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FusedRelation {
    /// Relation type.
    pub kind: RelationKind,
    /// Subject.
    pub from: usize,
    /// Object.
    pub to: usize,
    /// Confidence that the relation holds.
    pub confidence: Option<Score>,
}

/// Fuses normalized candidates of any number of sources.
///
/// Candidate identifiers only need to be unique within their source.
/// Structural parents are taken from structured sources (accessibility);
/// pixel sources (OCR, vision) are placed by containment instead.
pub fn fuse<'a>(candidates: impl IntoIterator<Item = &'a ElementCandidate>) -> Fusion {
    let mut structured = Vec::new();
    let mut text = Vec::new();
    let mut visual = Vec::new();
    for candidate in candidates {
        match candidate.meta.source {
            Source::Ocr => text.push(candidate),
            Source::Vision => visual.push(candidate),
            _ => structured.push(candidate),
        }
    }

    let mut builder = Builder::default();
    for candidate in structured {
        builder.add_structured(candidate);
    }
    builder.add_text(&text);
    // Largest first, so containers exist before their contents.
    visual.sort_by(|a, b| area(&b.bounds).total_cmp(&area(&a.bounds)));
    for candidate in visual {
        builder.add_visual(candidate);
    }
    builder.resolve_relations();
    builder.build_scene();
    builder.finish()
}

/// An element under construction.
#[derive(Debug)]
struct Node {
    role: Role,
    name: Option<String>,
    value: Option<String>,
    description: Option<String>,
    bounds: Bounds,
    visible_bounds: Option<Bounds>,
    state: ElementState,
    confidence: Confidence,
    /// Source of the candidate the node was created from.
    origin: Source,
    sources: Vec<Source>,
    parent: Option<usize>,
    /// Absorbed into another node.
    absorbed: bool,
    evidence: ElementEvidence,
    native_id: Option<String>,
    relations: Vec<(RelationKind, CandidateId, Option<Score>)>,
}

impl Node {
    fn new(candidate: &ElementCandidate, parent: Option<usize>) -> Self {
        Self {
            role: candidate.role,
            name: candidate.name.clone(),
            value: candidate.value.clone(),
            description: candidate.description.clone(),
            bounds: candidate.bounds,
            visible_bounds: candidate.visible_bounds,
            state: candidate.state,
            confidence: candidate.confidence,
            origin: candidate.meta.source,
            sources: vec![candidate.meta.source],
            parent,
            absorbed: false,
            evidence: ElementEvidence {
                contributions: vec![contribution(candidate, Link::Primary)],
                conflicts: Vec::new(),
            },
            native_id: candidate.meta.native_id.clone(),
            relations: candidate
                .relations
                .iter()
                .map(|relation| (relation.kind, relation.target, relation.confidence))
                .collect(),
        }
    }

    /// The part of the node pixel evidence can be compared with, or `None`
    /// if the node is not visible.
    fn visible_area(&self) -> Option<Bounds> {
        (self.state.visible != Some(false)).then_some(self.visible_bounds.unwrap_or(self.bounds))
    }

    /// Created from pixels or inferred, rather than reported by a structured
    /// source.
    fn pixel_derived(&self) -> bool {
        matches!(self.origin, Source::Ocr | Source::Vision | Source::Derived)
    }

    fn add(&mut self, candidate: &ElementCandidate, link: Link) {
        self.evidence.contributions.push(contribution(candidate, link));
        if !self.sources.contains(&candidate.meta.source) {
            self.sources.push(candidate.meta.source);
        }
        self.confidence.element = max(self.confidence.element, candidate.confidence.element);
    }

    fn conflict(&mut self, property: Property, kept: String, rejected: Claim, lowers: bool) {
        let kept = Claim { source: self.origin, value: kept };
        self.evidence.conflicts.push(Conflict {
            property,
            kept,
            rejected,
            lowers_confidence: lowers,
        });
    }
}

fn contribution(candidate: &ElementCandidate, link: Link) -> Contribution {
    Contribution {
        source: candidate.meta.source,
        link,
        role: candidate.role,
        name: candidate.name.clone(),
        value: candidate.value.clone(),
        state: candidate.state,
        bounds: candidate.bounds,
        confidence: candidate.confidence,
        native_role: candidate.meta.native_role.clone(),
    }
}

#[derive(Debug, Default)]
struct Builder {
    nodes: Vec<Node>,
    /// Where each structured candidate went.
    placed: HashMap<(Source, CandidateId), usize>,
    /// Relations between nodes: `(from, kind, to, confidence)`.
    relations: Vec<(usize, RelationKind, usize, Option<Score>)>,
}

impl Builder {
    /// Turns the relations structured sources reported between their
    /// candidates into relations between nodes.
    fn resolve_relations(&mut self) {
        for from in 0..self.nodes.len() {
            let origin = self.nodes[from].origin;
            for (kind, target, confidence) in std::mem::take(&mut self.nodes[from].relations) {
                if let Some(&to) = self.placed.get(&(origin, target)) {
                    self.relations.push((from, kind, to, confidence));
                }
            }
        }
    }

    fn add_structured(&mut self, candidate: &ElementCandidate) {
        let source = candidate.meta.source;
        let parent =
            candidate.parent.and_then(|parent| self.placed.get(&(source, parent)).copied());
        self.placed.insert((source, candidate.id), self.nodes.len());
        self.nodes.push(Node::new(candidate, parent));
    }

    fn add_text(&mut self, lines: &[&ElementCandidate]) {
        let mut assigned: Vec<(usize, Vec<&ElementCandidate>)> = Vec::new();
        for &line in lines {
            match self.text_target(line) {
                Some(target) => match assigned.iter_mut().find(|(node, _)| *node == target) {
                    Some((_, group)) => group.push(line),
                    None => assigned.push((target, vec![line])),
                },
                None => {
                    let parent = self.container_for(&line.bounds);
                    self.nodes.push(Node::new(line, parent));
                }
            }
        }
        for (target, mut group) in assigned {
            reading_order(&mut group, |line| line.bounds);
            self.describe_with_text(target, &group);
        }
    }

    /// The element a recognized line describes: the smallest visible element
    /// carrying text that contains the line, preferring one whose text
    /// matches it.
    fn text_target(&self, line: &ElementCandidate) -> Option<usize> {
        let text = line.name.as_deref()?;
        let matches = |node: &Node| {
            [&node.name, &node.value].into_iter().flatten().any(|known| similar(text, known))
        };
        let structured = || self.nodes.iter().enumerate().filter(|(_, node)| !node.pixel_derived());

        let inside = structured()
            .filter_map(|(index, node)| {
                let area = node.visible_area()?;
                let overlap = iou(&line.bounds, &area);
                let inside = cover(&line.bounds, &area) >= TEXT_INSIDE || overlap >= TEXT_OVERLAP;
                let matches = matches(node);
                // Any element showing its own text; otherwise only elements
                // whose text is their label. A text box shows its value or
                // its placeholder; anything else inside it is not about it.
                let about = matches || (carries_text(node.role) && node.role != Role::TextBox);
                (inside && about).then_some((index, (matches, overlap)))
            })
            .max_by(|(_, a), (_, b)| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)))
            .map(|(index, _)| index);
        if inside.is_some() {
            return inside;
        }

        // The visible label of a control whose frame does not include it
        // (a radio button reported as just its circle).
        structured()
            .filter(|(_, node)| matches(node))
            .filter_map(|(index, node)| {
                let (distance, _) =
                    scene::placement(node.role, &node.visible_area()?, &line.bounds)?;
                Some((index, distance))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(index, _)| index)
    }

    /// Applies recognized lines (in reading order) to the element they lie on.
    fn describe_with_text(&mut self, target: usize, lines: &[&ElementCandidate]) {
        let node = &mut self.nodes[target];
        let known = node.name.clone().or_else(|| node.value.clone());
        let Some(known) = known else {
            // An unnamed element is named by the text on it.
            let text: Vec<&str> = lines.iter().filter_map(|line| line.name.as_deref()).collect();
            node.name = Some(text.join(" "));
            node.confidence.name = lines.iter().map(|line| line.confidence.element).reduce(min);
            for line in lines {
                node.add(line, Link::Text);
            }
            return;
        };

        for line in lines {
            node.add(line, Link::Text);
            let text = line.name.clone().unwrap_or_default();
            let confirms =
                [&node.name, &node.value].into_iter().flatten().any(|known| similar(&text, known));
            if confirms {
                continue;
            }
            let rejected = Claim { source: line.meta.source, value: text };
            if node.role == Role::Text {
                // For text, the displayed string is the name: a mismatch
                // means one source is wrong.
                node.confidence.name =
                    lowered(node.confidence.name, line.confidence.name, line.confidence.element);
                node.conflict(Property::Name, known.clone(), rejected, true);
            } else {
                node.conflict(Property::VisibleText, known.clone(), rejected, false);
            }
        }
    }

    fn add_visual(&mut self, candidate: &ElementCandidate) {
        if is_detail(candidate.role) {
            if let Some(text) = self.text_under(&candidate.bounds) {
                self.nodes[text].add(candidate, Link::Part);
                return;
            }
        }
        match self.visual_match(candidate) {
            Some((target, Link::Same)) => self.merge_same(target, candidate),
            Some((target, link)) => self.nodes[target].add(candidate, link),
            None => self.create_visual(candidate),
        }
    }

    /// The smallest text element a glyph lies on.
    fn text_under(&self, glyph: &Bounds) -> Option<usize> {
        self.smallest(|node| {
            node.role == Role::Text
                && node.visible_area().is_some_and(|area| cover(glyph, &area) >= GLYPH_ON_TEXT)
        })
    }

    /// The element a detection describes, and how.
    fn visual_match(&self, candidate: &ElementCandidate) -> Option<(usize, Link)> {
        let bounds = &candidate.bounds;
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| !node.absorbed && node.origin != Source::Vision)
            .filter_map(|(index, node)| {
                let area_of = node.visible_area()?;
                let overlap = iou(bounds, &area_of);
                let compatible = compatible(candidate.role, node.role);
                let relative = area(bounds) / area(&area_of).max(f32::MIN_POSITIVE);
                let inside_control = is_control(node.role) && cover(bounds, &area_of) >= INSIDE;
                // Scores rank the kinds of match first, then their quality.
                if (compatible && overlap >= SAME_IOU) || overlap >= SAME_IOU_CONFLICTING {
                    Some((index, Link::Same, 2.0 + overlap))
                } else if inside_control && compatible && !is_detail(candidate.role) {
                    Some((index, Link::Same, 1.0 + relative))
                } else if inside_control {
                    // Structured controls are atomic: anything drawn inside
                    // them (a glyph, a tag dot) is a detail of the control.
                    Some((index, Link::Part, relative))
                } else {
                    None
                }
            })
            .max_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(index, link, _)| (index, link))
    }

    fn merge_same(&mut self, target: usize, candidate: &ElementCandidate) {
        let node = &mut self.nodes[target];
        node.add(candidate, Link::Same);
        let source = candidate.meta.source;

        if node.role == Role::Unknown && candidate.role != Role::Unknown {
            node.role = candidate.role;
            node.confidence.role = candidate.confidence.role;
        } else if !compatible(candidate.role, node.role) {
            node.confidence.role = lowered(
                node.confidence.role,
                candidate.confidence.role,
                candidate.confidence.element,
            );
            let kept = role_name(node.role);
            let rejected = Claim { source, value: role_name(candidate.role) };
            node.conflict(Property::Role, kept, rejected, true);
        }

        let theirs = candidate.state;
        let state_confidence = candidate.confidence.state;
        let fallback = candidate.confidence.element;
        let mut adopted = false;
        let mut conflicts = Vec::new();
        merge_flag(
            &mut node.state.enabled,
            theirs.enabled,
            Property::Enabled,
            &mut adopted,
            &mut conflicts,
        );
        merge_flag(
            &mut node.state.selected,
            theirs.selected,
            Property::Selected,
            &mut adopted,
            &mut conflicts,
        );
        if is_checkable(node.role) {
            merge_flag(
                &mut node.state.checked,
                theirs.checked,
                Property::Checked,
                &mut adopted,
                &mut conflicts,
            );
        }
        if adopted {
            node.confidence.state = match node.confidence.state {
                Some(current) => Some(min(current, state_confidence.unwrap_or(fallback))),
                None => state_confidence.or(Some(fallback)),
            };
        }
        for (property, kept, rejected) in conflicts {
            node.confidence.state = lowered(node.confidence.state, state_confidence, fallback);
            node.conflict(property, kept, Claim { source, value: rejected }, true);
        }
    }

    fn create_visual(&mut self, candidate: &ElementCandidate) {
        let parent = self.container_for(&candidate.bounds);
        let index = self.nodes.len();
        self.nodes.push(Node::new(candidate, parent));

        // Pixel-derived siblings inside the new element belong to it.
        let inside: Vec<usize> = (0..index)
            .filter(|&other| {
                let node = &self.nodes[other];
                !node.absorbed
                    && node.pixel_derived()
                    && node.parent == parent
                    && area(&node.bounds) < area(&candidate.bounds)
                    && cover(&node.bounds, &candidate.bounds) >= ADOPT
            })
            .collect();

        let texts: Vec<usize> = inside
            .iter()
            .copied()
            .filter(|&other| self.nodes[other].origin == Source::Ocr)
            .collect();
        let label = named_by_text(candidate.role)
            && !texts.is_empty()
            && texts.len() == inside.len()
            && single_line(&texts.iter().map(|&t| self.nodes[t].bounds).collect::<Vec<_>>());
        if label {
            self.label_with(index, texts);
        } else if candidate.role == Role::TextBox && self.field_text(&texts) {
            // The text in a field is its contents; its name is its label
            // (see the scene graph). Other things inside (a search glass)
            // stay its children.
            self.fill_with(index, texts.clone());
            for other in inside.into_iter().filter(|other| !texts.contains(other)) {
                self.nodes[other].parent = Some(index);
            }
        } else {
            for other in inside {
                self.nodes[other].parent = Some(index);
            }
        }
    }

    /// Whether recognized texts read like a field's contents: one run of
    /// text per line. Separate texts side by side are columns or labels (a
    /// table header taken for a field), not something typed.
    fn field_text(&self, texts: &[usize]) -> bool {
        !texts.is_empty()
            && lines(texts.to_vec(), |&text| self.nodes[text].bounds)
                .iter()
                .all(|line| line.len() == 1)
    }

    /// Absorbs recognized text elements into the field `target` as its
    /// value, line by line.
    fn fill_with(&mut self, target: usize, texts: Vec<usize>) {
        let lines = lines(texts, |&text| self.nodes[text].bounds);
        let mut rows = Vec::new();
        let mut confidence: Option<Score> = None;
        let mut contributions = Vec::new();
        for line in &lines {
            let mut words = Vec::new();
            for &text in line {
                let node = &mut self.nodes[text];
                node.absorbed = true;
                words.extend(node.name.clone());
                let score = node.confidence.name.unwrap_or(node.confidence.element);
                confidence = Some(confidence.map_or(score, |current| min(current, score)));
                contributions.extend(
                    std::mem::take(&mut node.evidence.contributions)
                        .into_iter()
                        .map(|contribution| Contribution { link: Link::Text, ..contribution }),
                );
            }
            rows.push(words.join(" "));
        }
        let node = &mut self.nodes[target];
        node.value = Some(rows.join("\n"));
        // Pixels cannot tell typed text from a placeholder.
        node.confidence.value =
            confidence.map(|score| min(score, Score::new(FIELD_TEXT).expect("in range")));
        if !node.sources.contains(&Source::Ocr) {
            node.sources.push(Source::Ocr);
        }
        node.evidence.contributions.extend(contributions);
    }

    /// Absorbs recognized text elements into `target` as its name.
    fn label_with(&mut self, target: usize, mut texts: Vec<usize>) {
        reading_order(&mut texts, |&text| self.nodes[text].bounds);
        let mut words = Vec::new();
        let mut confidence: Option<Score> = None;
        for &text in &texts {
            let line = &mut self.nodes[text];
            line.absorbed = true;
            words.extend(line.name.clone());
            let score = line.confidence.name.unwrap_or(line.confidence.element);
            confidence = Some(confidence.map_or(score, |current| min(current, score)));
        }
        let contributions: Vec<Contribution> = texts
            .iter()
            .flat_map(|&text| std::mem::take(&mut self.nodes[text].evidence.contributions))
            .map(|contribution| Contribution { link: Link::Text, ..contribution })
            .collect();

        let node = &mut self.nodes[target];
        node.name = Some(words.join(" "));
        node.confidence.name = confidence;
        if !node.sources.contains(&Source::Ocr) {
            node.sources.push(Source::Ocr);
        }
        node.evidence.contributions.extend(contributions);
    }

    /// The smallest visible element that contains `bounds`.
    fn container_for(&self, bounds: &Bounds) -> Option<usize> {
        let size = area(bounds);
        self.smallest(|node| {
            node.visible_area()
                .is_some_and(|area_of| area(&area_of) > size && cover(bounds, &area_of) >= INSIDE)
        })
    }

    /// The smallest live node satisfying `predicate`; the deepest (latest)
    /// one on ties.
    fn smallest(&self, predicate: impl Fn(&Node) -> bool) -> Option<usize> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| !node.absorbed && predicate(node))
            .min_by(|(i, a), (j, b)| area(&a.bounds).total_cmp(&area(&b.bounds)).then(j.cmp(i)))
            .map(|(index, _)| index)
    }

    /// Orders the live nodes (depth-first; structured children in source
    /// order, then pixel-derived children in reading order) and builds the
    /// result.
    fn finish(mut self) -> Fusion {
        let live = |node: &Node| !node.absorbed;
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); self.nodes.len()];
        let mut roots = Vec::new();
        let mut parents = vec![None; self.nodes.len()];
        for (index, node) in self.nodes.iter().enumerate().filter(|(_, node)| live(node)) {
            let mut parent = node.parent;
            while let Some(absorbed) = parent.filter(|&parent| !live(&self.nodes[parent])) {
                parent = self.nodes[absorbed].parent;
            }
            parents[index] = parent;
            match parent {
                Some(parent) => children[parent].push(index),
                None => roots.push(index),
            }
        }
        let sort = |list: &mut Vec<usize>, nodes: &[Node]| {
            let (mut structured, mut pixels): (Vec<usize>, Vec<usize>) =
                list.iter().partition(|&&index| !nodes[index].pixel_derived());
            structured.sort_unstable();
            reading_order(&mut pixels, |&index| nodes[index].bounds);
            structured.append(&mut pixels);
            *list = structured;
        };
        sort(&mut roots, &self.nodes);
        for list in &mut children {
            sort(list, &self.nodes);
        }

        let mut order = Vec::with_capacity(self.nodes.len());
        let mut stack: Vec<usize> = roots.into_iter().rev().collect();
        while let Some(index) = stack.pop() {
            order.push(index);
            stack.extend(children[index].iter().rev());
        }
        let mut position = vec![usize::MAX; self.nodes.len()];
        for (output, &index) in order.iter().enumerate() {
            position[index] = output;
        }

        let mut relations: Vec<FusedRelation> = self
            .relations
            .iter()
            .filter(|(from, _, to, _)| position[*from] != usize::MAX && position[*to] != usize::MAX)
            .map(|&(from, kind, to, confidence)| FusedRelation {
                kind,
                from: position[from],
                to: position[to],
                confidence,
            })
            .collect();
        relations.sort_by_key(|relation| relation.from);

        let elements = order
            .iter()
            .map(|&index| {
                let node = &mut self.nodes[index];
                let mut sources = std::mem::take(&mut node.sources);
                sources.sort_unstable();
                FusedElement {
                    role: node.role,
                    name: node.name.take(),
                    value: node.value.take(),
                    description: node.description.take(),
                    bounds: node.bounds,
                    visible_bounds: node.visible_bounds,
                    state: node.state,
                    confidence: node.confidence,
                    sources,
                    parent: parents[index].map(|parent| position[parent]),
                    evidence: std::mem::take(&mut node.evidence),
                    native_id: node.native_id.take(),
                }
            })
            .collect();
        Fusion { elements, relations }
    }
}

/// Merges one state flag reported by a less authoritative source: adopts it
/// when unknown, records a conflict when different.
fn merge_flag<T: Copy + PartialEq + std::fmt::Debug>(
    ours: &mut Option<T>,
    theirs: Option<T>,
    property: Property,
    adopted: &mut bool,
    conflicts: &mut Vec<(Property, String, String)>,
) {
    match (*ours, theirs) {
        (None, Some(value)) => {
            *ours = Some(value);
            *adopted = true;
        }
        (Some(kept), Some(rejected)) if kept != rejected => {
            conflicts.push((property, flag_name(kept), flag_name(rejected)));
        }
        _ => {}
    }
}

/// `snake_case` text of a state value (`true`, `checked`, ...).
fn flag_name<T: std::fmt::Debug>(value: T) -> String {
    let text = format!("{value:?}");
    let mut name = String::with_capacity(text.len() + 2);
    for (i, c) in text.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                name.push('_');
            }
            name.extend(c.to_lowercase());
        } else {
            name.push(c);
        }
    }
    name
}

fn role_name(role: Role) -> String {
    flag_name(role)
}

/// Confidence of a kept value after a less authoritative source disagreed:
/// `kept × (1 − rejected / 2)`. An unassessed kept value stays unassessed.
fn lowered(kept: Option<Score>, rejected: Option<Score>, fallback: Score) -> Option<Score> {
    let rejected = rejected.unwrap_or(fallback).get();
    kept.map(|kept| Score::new(kept.get() * (1.0 - rejected / 2.0)).unwrap_or(kept))
}

fn max(a: Score, b: Score) -> Score {
    if b.get() > a.get() { b } else { a }
}

fn min(a: Score, b: Score) -> Score {
    if b.get() < a.get() { b } else { a }
}

#[cfg(test)]
mod tests {
    use argus_protocol::{CandidateRelation, CheckState, SourceMeta};

    use super::*;

    fn b(x: f32, y: f32, width: f32, height: f32) -> Bounds {
        Bounds::new(x, y, width, height).unwrap()
    }

    fn score(value: f32) -> Score {
        Score::new(value).unwrap()
    }

    fn ax(
        id: u32,
        parent: Option<u32>,
        role: Role,
        name: Option<&str>,
        bounds: Bounds,
    ) -> ElementCandidate {
        ElementCandidate {
            id: CandidateId(id),
            parent: parent.map(CandidateId),
            role,
            name: name.map(str::to_owned),
            value: None,
            description: None,
            bounds,
            visible_bounds: None,
            state: ElementState::default(),
            confidence: Confidence {
                role: Some(Score::CERTAIN),
                name: name.map(|_| Score::CERTAIN),
                ..Confidence::new(Score::CERTAIN)
            },
            relations: Vec::new(),
            meta: SourceMeta::new(Source::Accessibility),
        }
    }

    fn ocr(id: u32, text: &str, bounds: Bounds) -> ElementCandidate {
        ElementCandidate {
            name: Some(text.to_owned()),
            confidence: Confidence { name: Some(score(0.9)), ..Confidence::new(score(0.9)) },
            meta: SourceMeta::new(Source::Ocr),
            ..ax(id, None, Role::Text, None, bounds)
        }
    }

    fn vision(id: u32, role: Role, bounds: Bounds) -> ElementCandidate {
        ElementCandidate {
            confidence: Confidence { role: Some(score(0.5)), ..Confidence::new(score(0.5)) },
            meta: SourceMeta::new(Source::Vision),
            ..ax(id, None, role, None, bounds)
        }
    }

    fn window() -> ElementCandidate {
        ax(0, None, Role::Window, Some("Demo"), b(0.0, 0.0, 400.0, 300.0))
    }

    fn links(element: &FusedElement) -> Vec<(Source, Link)> {
        element.evidence.contributions.iter().map(|c| (c.source, c.link)).collect()
    }

    #[test]
    fn one_object_seen_by_three_sources_is_one_element() {
        let candidates = [
            window(),
            ax(1, Some(0), Role::Button, Some("Save"), b(10.0, 10.0, 80.0, 24.0)),
            ocr(0, "Save", b(32.0, 15.0, 34.0, 14.0)),
            vision(0, Role::Button, b(11.0, 10.0, 79.0, 24.0)),
        ];
        let fusion = fuse(&candidates);

        let [_, save] = fusion.elements.as_slice() else { panic!("{fusion:#?}") };
        assert_eq!((save.role, save.name.as_deref()), (Role::Button, Some("Save")));
        assert_eq!(save.sources, [Source::Accessibility, Source::Ocr, Source::Vision]);
        assert_eq!(save.bounds, b(10.0, 10.0, 80.0, 24.0), "accessibility geometry is kept");
        assert_eq!(save.parent, Some(0));
        assert_eq!(
            links(save),
            [
                (Source::Accessibility, Link::Primary),
                (Source::Ocr, Link::Text),
                (Source::Vision, Link::Same)
            ]
        );
        assert!(save.evidence.conflicts.is_empty());
        assert_eq!(save.confidence.role, Some(Score::CERTAIN), "agreement does not change it");
    }

    #[test]
    fn accessibility_only_is_unchanged() {
        let mut label = ax(2, Some(0), Role::Text, Some("Name"), b(10.0, 50.0, 40.0, 16.0));
        label.relations = vec![CandidateRelation {
            kind: RelationKind::LabelFor,
            target: CandidateId(1),
            confidence: None,
        }];
        let candidates = [
            window(),
            ax(1, Some(0), Role::TextBox, None, b(60.0, 50.0, 100.0, 16.0)),
            label,
            ax(3, Some(1), Role::Button, Some("Clear"), b(150.0, 50.0, 10.0, 16.0)),
        ];
        let fusion = fuse(&candidates);

        let summary: Vec<_> =
            fusion.elements.iter().map(|e| (e.role, e.parent, e.sources.clone())).collect();
        let only_ax = vec![Source::Accessibility];
        assert_eq!(
            summary,
            [
                (Role::Window, None, only_ax.clone()),
                (Role::TextBox, Some(0), only_ax.clone()),
                (Role::Button, Some(1), only_ax.clone()),
                (Role::Text, Some(0), only_ax),
            ],
            "depth-first order"
        );
        assert_eq!(
            fusion.relations,
            [FusedRelation { kind: RelationKind::LabelFor, from: 3, to: 1, confidence: None }]
        );
    }

    #[test]
    fn check_state_conflict_is_recorded_and_lowers_confidence() {
        let mut checkbox =
            ax(1, Some(0), Role::Checkbox, Some("Smart links"), b(10.0, 10.0, 120.0, 18.0));
        checkbox.state.checked = Some(CheckState::Checked);
        checkbox.confidence.state = Some(Score::CERTAIN);
        // Vision sees only the box, at the left end of the accessibility frame.
        let mut detected = vision(0, Role::Checkbox, b(11.0, 11.0, 16.0, 16.0));
        detected.state.checked = Some(CheckState::Unchecked);
        detected.confidence.state = Some(score(0.5));

        let fusion = fuse(&[window(), checkbox, detected]);
        let [_, element] = fusion.elements.as_slice() else { panic!("{fusion:#?}") };
        assert_eq!(element.state.checked, Some(CheckState::Checked), "accessibility wins");
        assert_eq!(element.confidence.state, Some(score(0.75)), "1.0 × (1 − 0.5 / 2)");
        assert_eq!(element.sources, [Source::Accessibility, Source::Vision]);
        assert_eq!(
            element.evidence.conflicts,
            [Conflict {
                property: Property::Checked,
                kept: Claim { source: Source::Accessibility, value: "checked".to_owned() },
                rejected: Claim { source: Source::Vision, value: "unchecked".to_owned() },
                lowers_confidence: true,
            }]
        );
    }

    #[test]
    fn unknown_states_are_filled_by_weaker_sources() {
        let checkbox = ax(1, Some(0), Role::Checkbox, None, b(10.0, 10.0, 16.0, 16.0));
        let mut detected = vision(0, Role::Checkbox, b(10.0, 10.0, 16.0, 16.0));
        detected.state.checked = Some(CheckState::Checked);
        detected.confidence.state = Some(score(0.45));

        let fusion = fuse(&[window(), checkbox, detected]);
        let element = &fusion.elements[1];
        assert_eq!(element.state.checked, Some(CheckState::Checked));
        assert_eq!(element.confidence.state, Some(score(0.45)), "weak evidence stays weak");
    }

    #[test]
    fn incompatible_roles_at_the_same_place_conflict() {
        let field = ax(1, Some(0), Role::TextBox, None, b(10.0, 10.0, 100.0, 22.0));
        let fusion = fuse(&[window(), field, vision(0, Role::Button, b(10.0, 10.0, 100.0, 22.0))]);
        let [_, element] = fusion.elements.as_slice() else { panic!("{fusion:#?}") };
        assert_eq!(element.role, Role::TextBox);
        assert_eq!(element.confidence.role, Some(score(0.75)));
        assert_eq!(element.evidence.conflicts[0].property, Property::Role);
        assert_eq!(element.evidence.conflicts[0].rejected.value, "button");

        // Coarse visual roles are not conflicts.
        let link = ax(1, Some(0), Role::Link, Some("Help"), b(10.0, 10.0, 60.0, 20.0));
        let fusion = fuse(&[window(), link, vision(0, Role::Button, b(10.0, 10.0, 60.0, 20.0))]);
        assert!(fusion.elements[1].evidence.conflicts.is_empty());
        assert_eq!(fusion.elements[1].role, Role::Link);
    }

    #[test]
    fn text_confirms_or_contradicts_accessibility() {
        let candidates = [
            window(),
            ax(1, Some(0), Role::Text, Some("Hello world"), b(10.0, 10.0, 200.0, 20.0)),
            ax(2, Some(0), Role::Text, Some("Total"), b(10.0, 40.0, 200.0, 20.0)),
            ax(3, Some(0), Role::Button, Some("Point"), b(10.0, 70.0, 40.0, 40.0)),
            ocr(0, "Hello wor1d", b(12.0, 12.0, 90.0, 16.0)),
            ocr(1, "Balance", b(12.0, 42.0, 100.0, 16.0)),
            ocr(2, ",", b(26.0, 85.0, 6.0, 10.0)),
        ];
        let fusion = fuse(&candidates);
        assert_eq!(fusion.elements.len(), 4, "every line describes an existing element");

        let hello = &fusion.elements[1];
        assert!(hello.evidence.conflicts.is_empty());
        assert_eq!(hello.sources, [Source::Accessibility, Source::Ocr]);

        let total = &fusion.elements[2];
        assert_eq!(total.name.as_deref(), Some("Total"), "accessibility wins");
        assert_eq!(total.confidence.name, Some(score(0.55)), "1.0 × (1 − 0.9 / 2)");
        assert_eq!(total.evidence.conflicts[0].property, Property::Name);

        // An accessible name may differ from the visible label.
        let point = &fusion.elements[3];
        assert_eq!(point.name.as_deref(), Some("Point"));
        assert_eq!(point.confidence.name, Some(Score::CERTAIN));
        assert_eq!(point.evidence.conflicts[0].property, Property::VisibleText);
        assert!(!point.evidence.conflicts[0].lowers_confidence);
    }

    #[test]
    fn text_confirms_labels_drawn_outside_the_frame() {
        let candidates = [
            window(),
            // A column header with an unmapped role, and a radio button whose
            // frame is just its circle.
            ax(1, Some(0), Role::Unknown, Some("Volume"), b(10.0, 10.0, 150.0, 20.0)),
            ax(2, Some(0), Role::RadioButton, Some("H.264 1080p"), b(10.0, 50.0, 16.0, 16.0)),
            ocr(0, "Volume", b(14.0, 14.0, 40.0, 12.0)),
            ocr(1, "H.264 1080p", b(32.0, 51.0, 90.0, 14.0)),
            // Nearby text that says something else stays separate.
            ocr(2, "Other", b(60.0, 14.0, 40.0, 12.0)),
        ];
        let fusion = fuse(&candidates);
        assert_eq!(fusion.elements.len(), 4, "{fusion:#?}");
        assert_eq!(fusion.elements[1].sources, [Source::Accessibility, Source::Ocr]);
        assert_eq!(fusion.elements[3].sources, [Source::Accessibility, Source::Ocr]);
        assert_eq!(fusion.elements[2].name.as_deref(), Some("Other"));
    }

    #[test]
    fn text_names_unnamed_controls() {
        let candidates = [
            window(),
            ax(1, Some(0), Role::Button, None, b(10.0, 10.0, 80.0, 24.0)),
            ocr(0, "Open", b(30.0, 15.0, 40.0, 14.0)),
        ];
        let fusion = fuse(&candidates);
        let button = &fusion.elements[1];
        assert_eq!(button.name.as_deref(), Some("Open"));
        assert_eq!(button.confidence.name, Some(score(0.9)));
        assert_eq!(button.confidence.role, Some(Score::CERTAIN));
    }

    #[test]
    fn text_boxes_only_take_matching_text() {
        let mut field = ax(1, Some(0), Role::TextBox, Some("Search"), b(10.0, 10.0, 200.0, 22.0));
        field.value = Some("cats".to_owned());
        let candidates = [
            window(),
            field,
            ocr(0, "cats", b(20.0, 14.0, 30.0, 14.0)),
            ocr(1, "12", b(180.0, 14.0, 20.0, 14.0)),
        ];
        let fusion = fuse(&candidates);
        let names: Vec<_> = fusion.elements.iter().map(|e| (e.role, e.parent)).collect();
        assert_eq!(names, [(Role::Window, None), (Role::TextBox, Some(0)), (Role::Text, Some(1))]);
        assert_eq!(fusion.elements[1].sources, [Source::Accessibility, Source::Ocr]);
        assert_eq!(fusion.elements[2].name.as_deref(), Some("12"));
    }

    #[test]
    fn pixels_recover_elements_accessibility_misses() {
        // An application that exposes only its window.
        let candidates = [
            window(),
            vision(0, Role::Group, b(10.0, 10.0, 200.0, 30.0)),
            vision(1, Role::Button, b(12.0, 12.0, 60.0, 26.0)),
            vision(2, Role::Button, b(80.0, 12.0, 60.0, 26.0)),
            ocr(0, "Run", b(28.0, 18.0, 28.0, 14.0)),
            ocr(1, "Debug", b(88.0, 18.0, 44.0, 14.0)),
            ocr(2, "Ready", b(10.0, 280.0, 40.0, 14.0)),
        ];
        let fusion = fuse(&candidates);
        let summary: Vec<_> = fusion
            .elements
            .iter()
            .map(|e| (e.role, e.name.as_deref(), e.parent, e.sources.clone()))
            .collect();
        let pixels = vec![Source::Ocr, Source::Vision];
        assert_eq!(
            summary,
            [
                (Role::Window, Some("Demo"), None, vec![Source::Accessibility]),
                (Role::Group, None, Some(0), vec![Source::Vision]),
                (Role::Button, Some("Run"), Some(1), pixels.clone()),
                (Role::Button, Some("Debug"), Some(1), pixels),
                (Role::Text, Some("Ready"), Some(0), vec![Source::Ocr]),
            ]
        );
        let run = &fusion.elements[2];
        assert_eq!(run.confidence.name, Some(score(0.9)));
        assert_eq!(run.confidence.role, Some(score(0.5)));
        assert_eq!(links(run), [(Source::Vision, Link::Primary), (Source::Ocr, Link::Text)]);
    }

    #[test]
    fn glyphs_are_parts_not_elements() {
        let candidates = [
            window(),
            ax(1, Some(0), Role::Text, Some("1234"), b(10.0, 10.0, 200.0, 40.0)),
            ax(2, Some(0), Role::Button, Some("Share"), b(10.0, 60.0, 32.0, 32.0)),
            vision(0, Role::Icon, b(150.0, 15.0, 18.0, 30.0)),
            vision(1, Role::Icon, b(170.0, 15.0, 18.0, 30.0)),
            vision(2, Role::Icon, b(16.0, 66.0, 20.0, 20.0)),
        ];
        let fusion = fuse(&candidates);
        assert_eq!(fusion.elements.len(), 3);
        assert_eq!(links(&fusion.elements[1])[1..], [(Source::Vision, Link::Part); 2]);
        // The button's glyph.
        assert_eq!(links(&fusion.elements[2])[1], (Source::Vision, Link::Part));
        assert!(fusion.elements.iter().all(|e| e.evidence.conflicts.is_empty()));
    }

    #[test]
    fn detections_inside_structured_controls_are_details() {
        // A colored tag dot in a sidebar row looks like a radio button.
        let row = ax(1, Some(0), Role::Cell, Some("Red"), b(10.0, 10.0, 150.0, 24.0));
        let fusion =
            fuse(&[window(), row, vision(0, Role::RadioButton, b(16.0, 16.0, 12.0, 12.0))]);
        assert_eq!(fusion.elements.len(), 2);
        assert_eq!(links(&fusion.elements[1])[1], (Source::Vision, Link::Part));
    }

    #[test]
    fn hidden_elements_are_not_matched_by_pixels() {
        let mut hidden = ax(1, Some(0), Role::Button, Some("Save"), b(10.0, 10.0, 80.0, 24.0));
        hidden.state.visible = Some(false);
        let fusion = fuse(&[window(), hidden, ocr(0, "Save", b(32.0, 15.0, 34.0, 14.0))]);
        assert_eq!(fusion.elements.len(), 3);
        assert_eq!(fusion.elements[1].sources, [Source::Accessibility]);
        assert_eq!(fusion.elements[2].role, Role::Text);
    }

    #[test]
    fn pixel_only_output_is_in_reading_order() {
        let candidates = [
            ocr(0, "second", b(100.0, 12.0, 50.0, 14.0)),
            ocr(1, "third", b(10.0, 40.0, 50.0, 14.0)),
            ocr(2, "first", b(10.0, 10.0, 50.0, 14.0)),
        ];
        let fusion = fuse(&candidates);
        let names: Vec<_> = fusion.elements.iter().map(|e| e.name.as_deref().unwrap()).collect();
        assert_eq!(names, ["first", "second", "third"]);
    }

    #[test]
    fn multi_line_text_stays_separate_inside_new_elements() {
        let candidates = [
            vision(0, Role::Button, b(0.0, 0.0, 200.0, 60.0)),
            ocr(0, "Title", b(10.0, 5.0, 60.0, 14.0)),
            ocr(1, "Details", b(10.0, 30.0, 60.0, 14.0)),
        ];
        let fusion = fuse(&candidates);
        assert_eq!(fusion.elements[0].name, None);
        assert_eq!(fusion.elements[1].parent, Some(0));
        assert_eq!(fusion.elements[2].parent, Some(0));
    }

    #[test]
    fn text_in_a_detected_field_is_its_value() {
        let candidates = [
            vision(0, Role::TextBox, b(0.0, 0.0, 300.0, 60.0)),
            vision(1, Role::Icon, b(270.0, 5.0, 20.0, 20.0)),
            ocr(0, "Ship by Friday.", b(10.0, 5.0, 120.0, 14.0)),
            ocr(1, "Review the results.", b(10.0, 30.0, 130.0, 14.0)),
        ];
        let fusion = fuse(&candidates);
        let field = &fusion.elements[0];
        assert_eq!(field.role, Role::TextBox);
        assert_eq!(field.value.as_deref(), Some("Ship by Friday.\nReview the results."));
        assert_eq!(field.name, None, "a field is named by its label, not its contents");
        assert!(field.confidence.value.unwrap().get() <= 0.7, "may be a placeholder");
        assert!(field.sources.contains(&Source::Ocr));
        // The text is absorbed; the icon stays a child.
        assert_eq!(fusion.elements.len(), 2, "{:#?}", fusion.elements);
        assert_eq!(fusion.elements[1].role, Role::Icon);
        assert_eq!(fusion.elements[1].parent, Some(0));
    }

    #[test]
    fn texts_side_by_side_are_not_a_fields_contents() {
        // A table header taken for a field: its column titles stay.
        let candidates = [
            vision(0, Role::TextBox, b(0.0, 0.0, 300.0, 24.0)),
            ocr(0, "Volume", b(10.0, 5.0, 40.0, 12.0)),
            ocr(1, "Folder", b(150.0, 5.0, 40.0, 12.0)),
        ];
        let fusion = fuse(&candidates);
        assert_eq!(fusion.elements[0].value, None);
        assert_eq!(fusion.elements.len(), 3);
    }

    #[test]
    fn evidence_serializes() {
        let fusion = fuse(&[window(), ocr(0, "Hi", b(10.0, 10.0, 20.0, 14.0))]);
        let json = serde_json::to_value(&fusion.elements[1].evidence).unwrap();
        assert_eq!(json["contributions"][0]["source"], "ocr");
        assert_eq!(json["contributions"][0]["link"], "primary");
        assert!(json.get("conflicts").is_none());
    }
}
