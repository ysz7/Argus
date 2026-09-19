//! UI scene graph v0: structure inferred from the fused elements.
//!
//! Runs after fusion, on all sources together. Everything inferred here is
//! attributed to [`Source::Derived`] with modest confidence, and never
//! overrides what a structured source reported:
//!
//! - **labels** — an unnamed checkbox, radio button, text box or slider without
//!   a reported label is labelled by a text in the same container right of it
//!   (toggles) or left of or above it (fields); the text names it;
//! - **rows** — inside one container, consecutive lines of pixel-derived
//!   elements with the same shape (e.g. text, text, button) become `row`
//!   elements, so that identical buttons are told apart by their row;
//! - **spatial containment** — an element drawn inside another one that is not
//!   its ancestor gets a `contains` relation, when the two come from different
//!   kinds of sources (pixels vs. a structured tree) or the outer one is a
//!   control; overlapping layout frames within one tree are not reported.

use std::collections::{HashMap, HashSet};

use argus_protocol::{Bounds, Confidence, ElementState, RelationKind, Role, Score, Source};

use super::{Builder, Node, min};
use crate::evidence::{Contribution, ElementEvidence, Link};
use crate::geometry::{area, center, cover};
use crate::order::lines;

/// Confidence that a text beside a checkbox or radio button labels it.
const LABEL_BESIDE_TOGGLE: f32 = 0.6;
/// Confidence that a text left of a field labels it.
const LABEL_LEFT_OF_FIELD: f32 = 0.5;
/// Confidence that a text above a field labels it.
const LABEL_ABOVE_FIELD: f32 = 0.4;
/// Largest gap between a toggle and its label.
const TOGGLE_GAP: f32 = 16.0;
/// Largest gap between a label and the field right of it.
const FIELD_GAP: f32 = 48.0;
/// Largest gap between a label and the field below it.
const ABOVE_GAP: f32 = 12.0;
/// Largest offset between the left edges of a label and the field below it.
const ABOVE_ALIGN: f32 = 16.0;
/// Confidence that an inferred row exists.
const ROW_CONFIDENCE: f32 = 0.5;
/// A leaf drawn inside another element when this much of it is inside.
const CONTAINED: f32 = 0.9;

impl Builder {
    /// Infers labels, rows and spatial containment.
    pub(super) fn build_scene(&mut self) {
        self.label_controls();
        self.derive_rows();
        self.spatial_containment();
    }

    fn live(&self) -> impl Iterator<Item = (usize, &Node)> {
        self.nodes.iter().enumerate().filter(|(_, node)| !node.absorbed)
    }

    fn label_controls(&mut self) {
        let labelled: HashSet<usize> = self.labels().map(|(_, to)| to).collect();
        let used: HashSet<usize> = self.labels().map(|(from, _)| from).collect();
        let controls: Vec<usize> = self
            .live()
            .filter(|(index, node)| {
                matches!(
                    node.role,
                    Role::Checkbox | Role::RadioButton | Role::TextBox | Role::Slider
                ) && node.name.is_none()
                    && !labelled.contains(index)
                    && node.visible_area().is_some()
            })
            .map(|(index, _)| index)
            .collect();
        let texts: Vec<usize> = self
            .live()
            .filter(|(index, node)| {
                node.role == Role::Text
                    && node.name.is_some()
                    && !used.contains(index)
                    && node.visible_area().is_some()
                    && !node.parent.is_some_and(|parent| self.is_named_control(parent))
            })
            .map(|(index, _)| index)
            .collect();

        // Closest pairs first; every text labels one control at most.
        let mut pairs = Vec::new();
        for &control in &controls {
            // A label lives in the same container as its control: a sidebar
            // heading is not the label of a field in the pane next to it.
            let parent = self.nodes[control].parent;
            for &text in texts.iter().filter(|&&text| self.nodes[text].parent == parent) {
                let (control_area, text_area) = (
                    self.nodes[control].visible_area().expect("filtered"),
                    self.nodes[text].visible_area().expect("filtered"),
                );
                if let Some((distance, confidence)) =
                    placement(self.nodes[control].role, &control_area, &text_area)
                {
                    pairs.push((distance, control, text, confidence));
                }
            }
        }
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));

        let (mut done_controls, mut done_texts) = (HashSet::new(), HashSet::new());
        for (_, control, text, confidence) in pairs {
            if !done_controls.insert(control) || !done_texts.insert(text) {
                continue;
            }
            let score = Score::new(confidence).expect("constant in range");
            self.relations.push((text, RelationKind::LabelFor, control, Some(score)));

            let label = &self.nodes[text];
            let name = label.name.clone();
            let name_confidence = label.confidence.name.map_or(score, |text| min(text, score));
            let evidence = Contribution {
                source: Source::Derived,
                link: Link::Label,
                role: Role::Text,
                name: name.clone(),
                value: None,
                state: ElementState::default(),
                bounds: label.bounds,
                confidence: Confidence { name: Some(name_confidence), ..Confidence::new(score) },
                native_role: None,
            };
            let node = &mut self.nodes[control];
            node.name = name;
            node.confidence.name = Some(name_confidence);
            node.evidence.contributions.push(evidence);
            if !node.sources.contains(&Source::Derived) {
                node.sources.push(Source::Derived);
            }
        }
    }

    /// `(label, labelled)` pairs of the known `label_for` relations.
    fn labels(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.relations
            .iter()
            .filter(|(_, kind, _, _)| *kind == RelationKind::LabelFor)
            .map(|&(from, _, to, _)| (from, to))
    }

    /// Text inside a named control belongs to it, not to its neighbours.
    fn is_named_control(&self, index: usize) -> bool {
        let node = &self.nodes[index];
        node.name.is_some() && crate::roles::is_control(node.role)
    }

    fn derive_rows(&mut self) {
        let labels: HashSet<usize> = self.labels().map(|(from, _)| from).collect();
        // Pixel-derived children of every container (`None`: the roots of an
        // observation without a window element).
        let mut children: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
        for (index, node) in self.live().filter(|(_, node)| node.pixel_derived()) {
            children.entry(node.parent).or_default().push(index);
        }
        let mut containers: Vec<(Option<usize>, Vec<usize>)> = children.into_iter().collect();
        containers.sort_by_key(|(container, _)| *container);
        for (container, members) in containers {
            if members.len() < 4 {
                continue;
            }
            let lines = lines(members, |&index| self.nodes[index].bounds);
            let shape = |line: &Vec<usize>| -> Vec<Role> {
                line.iter()
                    .filter(|index| !labels.contains(index))
                    .map(|&index| self.nodes[index].role)
                    .collect()
            };
            let shapes: Vec<Vec<Role>> = lines.iter().map(shape).collect();

            let mut start = 0;
            while start < lines.len() {
                let mut end = start + 1;
                while end < lines.len() && shapes[end] == shapes[start] {
                    end += 1;
                }
                let tabular = shapes[start].len() >= 2 && shapes[start].contains(&Role::Text);
                if end - start >= 2 && tabular {
                    for line in &lines[start..end] {
                        self.add_row(container, line);
                    }
                }
                start = end;
            }
        }
    }

    fn add_row(&mut self, container: Option<usize>, members: &[usize]) {
        let bounds = members
            .iter()
            .map(|&index| self.nodes[index].bounds)
            .reduce(|a, b| union(&a, &b))
            .expect("rows have members");
        let score = Score::new(ROW_CONFIDENCE).expect("constant in range");
        let confidence = Confidence { role: Some(score), ..Confidence::new(score) };
        let row = self.nodes.len();
        self.nodes.push(Node {
            role: Role::Row,
            name: None,
            value: None,
            description: None,
            bounds,
            visible_bounds: None,
            state: ElementState::default(),
            confidence,
            origin: Source::Derived,
            sources: vec![Source::Derived],
            parent: container,
            absorbed: false,
            native_id: None,
            evidence: ElementEvidence {
                contributions: vec![Contribution {
                    source: Source::Derived,
                    link: Link::Primary,
                    role: Role::Row,
                    name: None,
                    value: None,
                    state: ElementState::default(),
                    bounds,
                    confidence,
                    native_role: Some("row".to_owned()),
                }],
                conflicts: Vec::new(),
            },
            relations: Vec::new(),
        });
        for &member in members {
            self.nodes[member].parent = Some(row);
        }
    }

    fn spatial_containment(&mut self) {
        let mut has_children = vec![false; self.nodes.len()];
        for (_, node) in self.live() {
            if let Some(parent) = node.parent {
                has_children[parent] = true;
            }
        }
        let areas: Vec<(usize, Bounds)> =
            self.live().filter_map(|(index, node)| Some((index, node.visible_area()?))).collect();

        // Anything containing most of a leaf covers the leaf's center, so a
        // grid of cells lists the few candidates worth checking.
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (slot, (_, bounds)) in areas.iter().enumerate() {
            let (x0, y0) = cell(bounds.x(), bounds.y());
            let (x1, y1) = cell(bounds.x() + bounds.width(), bounds.y() + bounds.height());
            for x in x0..=x1 {
                for y in y0..=y1 {
                    grid.entry((x, y)).or_default().push(slot);
                }
            }
        }

        let mut found = Vec::new();
        for &(leaf, leaf_area) in areas.iter().filter(|(index, _)| !has_children[*index]) {
            let size = area(&leaf_area);
            let (cx, cy) = center(&leaf_area);
            let outer = grid
                .get(&cell(cx, cy))
                .into_iter()
                .flatten()
                .map(|&slot| areas[slot])
                .filter(|&(other, other_area)| {
                    other != leaf
                        && area(&other_area) > size
                        && cover(&leaf_area, &other_area) >= CONTAINED
                })
                .min_by(|a, b| area(&a.1).total_cmp(&area(&b.1)).then(b.0.cmp(&a.0)));
            if let Some((outer, _)) = outer
                && self.meaningful_containment(outer, leaf)
                && !self.is_ancestor(outer, leaf)
            {
                found.push((outer, RelationKind::Contains, leaf, None));
            }
        }
        self.relations.extend(found);
    }

    /// Overlapping frames within one structured tree are mostly layout
    /// (text runs of a web view overlap each other); containment is
    /// reported when it relates evidence across sources, or when something
    /// is drawn inside a control.
    fn meaningful_containment(&self, outer: usize, leaf: usize) -> bool {
        let (outer, leaf) = (&self.nodes[outer], &self.nodes[leaf]);
        outer.pixel_derived() != leaf.pixel_derived() || crate::roles::is_control(outer.role)
    }

    fn is_ancestor(&self, ancestor: usize, mut node: usize) -> bool {
        let mut steps = 0;
        while let Some(parent) = self.nodes[node].parent {
            if parent == ancestor {
                return true;
            }
            node = parent;
            steps += 1;
            if steps > self.nodes.len() {
                return false;
            }
        }
        false
    }
}

/// Where a text sits relative to a control, if it can be the control's
/// label: `(distance, confidence)`.
pub(super) fn placement(role: Role, control: &Bounds, text: &Bounds) -> Option<(f32, f32)> {
    let same_line =
        (center(control).1 - center(text).1).abs() <= control.height().max(text.height()) / 2.0;
    let right_of_control = text.x() - (control.x() + control.width());
    let left_of_control = control.x() - (text.x() + text.width());
    let above_control = control.y() - (text.y() + text.height());
    match role {
        Role::Checkbox | Role::RadioButton => (same_line
            && (-2.0..=TOGGLE_GAP).contains(&right_of_control))
        .then_some((right_of_control, LABEL_BESIDE_TOGGLE)),
        Role::TextBox | Role::Slider => {
            if same_line && (-2.0..=FIELD_GAP).contains(&left_of_control) {
                Some((left_of_control, LABEL_LEFT_OF_FIELD))
            } else if (-2.0..=ABOVE_GAP).contains(&above_control)
                && (text.x() - control.x()).abs() <= ABOVE_ALIGN
            {
                Some((above_control + FIELD_GAP, LABEL_ABOVE_FIELD))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Side of a spatial index cell, in points.
const CELL: f32 = 64.0;

fn cell(x: f32, y: f32) -> (i64, i64) {
    ((x / CELL).floor() as i64, (y / CELL).floor() as i64)
}

fn union(a: &Bounds, b: &Bounds) -> Bounds {
    let left = a.x().min(b.x());
    let top = a.y().min(b.y());
    let right = (a.x() + a.width()).max(b.x() + b.width());
    let bottom = (a.y() + a.height()).max(b.y() + b.height());
    Bounds::new(left, top, right - left, bottom - top).unwrap_or(*a)
}

#[cfg(test)]
mod tests {
    use argus_protocol::{CandidateId, CandidateRelation, ElementCandidate, SourceMeta};

    use crate::fuse::fuse;
    use crate::{FusedElement, FusedRelation, Fusion};

    use super::*;

    fn b(x: f32, y: f32, width: f32, height: f32) -> Bounds {
        Bounds::new(x, y, width, height).unwrap()
    }

    fn candidate(
        source: Source,
        id: u32,
        role: Role,
        name: Option<&str>,
        bounds: Bounds,
    ) -> ElementCandidate {
        let score = Score::new(if source == Source::Accessibility { 1.0 } else { 0.5 }).unwrap();
        ElementCandidate {
            id: CandidateId(id),
            parent: None,
            role,
            name: name.map(str::to_owned),
            value: None,
            description: None,
            bounds,
            visible_bounds: None,
            state: ElementState::default(),
            confidence: Confidence {
                role: Some(score),
                name: name.map(|_| score),
                ..Confidence::new(score)
            },
            relations: Vec::new(),
            meta: SourceMeta::new(source),
        }
    }

    fn text(id: u32, name: &str, bounds: Bounds) -> ElementCandidate {
        candidate(Source::Ocr, id, Role::Text, Some(name), bounds)
    }

    fn vision(id: u32, role: Role, bounds: Bounds) -> ElementCandidate {
        candidate(Source::Vision, id, role, None, bounds)
    }

    fn named<'a>(fusion: &'a Fusion, name: &str) -> (usize, &'a FusedElement) {
        fusion
            .elements
            .iter()
            .enumerate()
            .find(|(_, e)| e.name.as_deref() == Some(name) && e.role == Role::Text)
            .unwrap_or_else(|| panic!("no text {name}"))
    }

    fn labels(fusion: &Fusion) -> Vec<(usize, usize)> {
        fusion
            .relations
            .iter()
            .filter(|r| r.kind == RelationKind::LabelFor)
            .map(|r| (r.from, r.to))
            .collect()
    }

    #[test]
    fn toggles_and_fields_are_labelled_by_nearby_text() {
        let fusion = fuse(&[
            vision(0, Role::RadioButton, b(10.0, 10.0, 16.0, 16.0)),
            text(0, "Fast", b(32.0, 11.0, 40.0, 14.0)),
            vision(1, Role::TextBox, b(80.0, 40.0, 120.0, 22.0)),
            text(1, "Width:", b(30.0, 44.0, 42.0, 14.0)),
            vision(2, Role::TextBox, b(10.0, 100.0, 200.0, 22.0)),
            text(2, "Notes", b(12.0, 82.0, 40.0, 14.0)),
            // Too far from anything.
            text(3, "Footer", b(300.0, 300.0, 40.0, 14.0)),
        ]);
        let control = |role: Role, y: f32| {
            fusion.elements.iter().position(|e| e.role == role && e.bounds.y() == y).unwrap()
        };
        let (radio, width, notes) = (
            control(Role::RadioButton, 10.0),
            control(Role::TextBox, 40.0),
            control(Role::TextBox, 100.0),
        );
        let mut expected = vec![
            (named(&fusion, "Fast").0, radio),
            (named(&fusion, "Width:").0, width),
            (named(&fusion, "Notes").0, notes),
        ];
        expected.sort();
        let mut actual = labels(&fusion);
        actual.sort();
        assert_eq!(actual, expected);

        let radio = &fusion.elements[radio];
        assert_eq!(radio.name.as_deref(), Some("Fast"));
        assert_eq!(radio.confidence.name, Some(Score::new(0.5).unwrap()), "min(OCR, relation)");
        assert_eq!(radio.sources, [Source::Vision, Source::Derived]);
        assert_eq!(fusion.elements[notes].confidence.name, Some(Score::new(0.4).unwrap()));
    }

    #[test]
    fn labels_stay_in_their_container() {
        let mut sidebar =
            candidate(Source::Accessibility, 0, Role::List, None, b(0.0, 0.0, 150.0, 400.0));
        sidebar.parent = None;
        let mut heading = candidate(
            Source::Accessibility,
            1,
            Role::Text,
            Some("Locations"),
            b(10.0, 100.0, 130.0, 20.0),
        );
        heading.parent = Some(CandidateId(0));
        let mut pane =
            candidate(Source::Accessibility, 2, Role::Group, None, b(150.0, 0.0, 300.0, 400.0));
        pane.parent = None;
        let mut field =
            candidate(Source::Accessibility, 3, Role::TextBox, None, b(170.0, 100.0, 80.0, 20.0));
        field.parent = Some(CandidateId(2));
        let fusion = fuse(&[sidebar, heading, pane, field]);
        assert!(labels(&fusion).is_empty());
        assert!(fusion.elements.iter().all(|e| e.role != Role::TextBox || e.name.is_none()));
    }

    #[test]
    fn reported_labels_are_not_second_guessed() {
        let mut label = candidate(
            Source::Accessibility,
            1,
            Role::Text,
            Some("Name"),
            b(10.0, 10.0, 40.0, 16.0),
        );
        label.relations = vec![CandidateRelation {
            kind: RelationKind::LabelFor,
            target: CandidateId(2),
            confidence: Some(Score::CERTAIN),
        }];
        let field = candidate(
            Source::Accessibility,
            2,
            Role::TextBox,
            Some("Name"),
            b(60.0, 10.0, 100.0, 22.0),
        );
        let other =
            candidate(Source::Accessibility, 3, Role::TextBox, None, b(200.0, 10.0, 100.0, 22.0));
        let fusion = fuse(&[label, field, other]);
        assert_eq!(labels(&fusion), [(0, 1)], "the reported label stays the only one");
        assert_eq!(fusion.elements[2].name, None, "no nearby unused text");
    }

    #[test]
    fn repeated_lines_become_rows() {
        let row = |id: u32, y: f32, server: &str, status: &str| {
            [
                text(id * 2, server, b(20.0, y, 60.0, 14.0)),
                text(id * 2 + 1, status, b(120.0, y, 50.0, 14.0)),
                vision(id, Role::Button, b(220.0, y - 4.0, 70.0, 22.0)),
            ]
        };
        let mut candidates = vec![vision(9, Role::Group, b(10.0, 10.0, 300.0, 120.0))];
        candidates.extend(row(0, 30.0, "Server A", "Online"));
        candidates.extend(row(1, 60.0, "Server B", "Offline"));
        candidates.extend(row(2, 90.0, "Server C", "Online"));
        let fusion = fuse(&candidates);

        let rows: Vec<usize> =
            (0..fusion.elements.len()).filter(|&i| fusion.elements[i].role == Role::Row).collect();
        assert_eq!(rows.len(), 3);
        for &row in &rows {
            let members: Vec<Role> =
                fusion.elements.iter().filter(|e| e.parent == Some(row)).map(|e| e.role).collect();
            assert_eq!(members, [Role::Text, Role::Text, Role::Button]);
            assert_eq!(fusion.elements[row].sources, [Source::Derived]);
            assert_eq!(fusion.elements[fusion.elements[row].parent.unwrap()].role, Role::Group);
        }
        // Identical buttons are told apart by their row.
        let (b_server, _) = named(&fusion, "Server B");
        let row_b = fusion.elements[b_server].parent.unwrap();
        assert!(fusion.elements.iter().any(|e| e.role == Role::Button && e.parent == Some(row_b)));
    }

    #[test]
    fn forms_and_button_bars_are_not_rows() {
        let fusion = fuse(&[
            vision(0, Role::Group, b(0.0, 0.0, 400.0, 200.0)),
            vision(1, Role::Button, b(10.0, 10.0, 60.0, 22.0)),
            vision(2, Role::Button, b(80.0, 10.0, 60.0, 22.0)),
            vision(3, Role::Button, b(10.0, 40.0, 60.0, 22.0)),
            vision(4, Role::Button, b(80.0, 40.0, 60.0, 22.0)),
            vision(5, Role::Checkbox, b(10.0, 80.0, 16.0, 16.0)),
            text(0, "Bold", b(32.0, 81.0, 40.0, 14.0)),
            vision(6, Role::Checkbox, b(10.0, 110.0, 16.0, 16.0)),
            text(1, "Italic", b(32.0, 111.0, 40.0, 14.0)),
        ]);
        assert!(fusion.elements.iter().all(|e| e.role != Role::Row));
    }

    #[test]
    fn spatial_containment_across_the_tree() {
        let mut bar =
            candidate(Source::Accessibility, 0, Role::Group, None, b(0.0, 0.0, 300.0, 50.0));
        bar.parent = None;
        let mut close = candidate(
            Source::Accessibility,
            1,
            Role::Button,
            Some("Close"),
            b(10.0, 10.0, 16.0, 16.0),
        );
        close.parent = None;
        let mut glyph =
            candidate(Source::Accessibility, 2, Role::Image, None, b(12.0, 12.0, 12.0, 12.0));
        glyph.parent = None;
        let mut overlapping_text =
            candidate(Source::Accessibility, 3, Role::Text, Some("a"), b(100.0, 10.0, 20.0, 16.0));
        overlapping_text.parent = None;
        let fusion = fuse(&[bar, close, glyph, overlapping_text]);
        // An image drawn inside a button is reported; the group's layout
        // overlap with its neighbours is not.
        assert_eq!(
            fusion.relations,
            [FusedRelation { kind: RelationKind::Contains, from: 1, to: 2, confidence: None }]
        );
    }
}
