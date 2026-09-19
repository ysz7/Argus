//! Comparing an observation with the truth.
//!
//! Predicted and true elements are paired one to one, greedily by the
//! overlap of their bounds (IoU ≥ [`MATCH_IOU`], largest first); a true
//! control still unpaired then takes a prediction of the same role inside it
//! (a checkbox found by its box, whose true bounds include the label). Then:
//!
//! - **recall**: significant true elements that were paired;
//! - **precision**: of the predicted non-structural elements, those paired
//!   with a significant true element; a predicted element paired with an
//!   insignificant one (a container) or lying inside a true element (the
//!   text in a field) counts neither way, an unpaired one (invented or a
//!   duplicate) is a false positive;
//! - role, text, grounding, state and relation accuracy over the pairs;
//! - calibration of `confidence.element` against being a true positive.
//!
//! All counts are additive, so cases and steps aggregate by summing.

use std::collections::{BTreeMap, HashMap};

use argus_protocol::{Bounds, CheckState, Element, Observation, RelationKind, Role};
use serde::Serialize;

use crate::dataset::{Truth, TruthElement, is_structural, role_name};

/// Minimum overlap for a predicted element to be a true one.
pub const MATCH_IOU: f32 = 0.5;
/// Overlap that counts as precise grounding.
pub const PRECISE_IOU: f32 = 0.75;
/// Share of a prediction inside a true element for it to be a part of it.
pub const CONTAINED: f32 = 0.8;
/// Confidence bins for calibration.
pub const BINS: usize = 10;

/// Intersection over union of two rectangles.
pub fn iou(a: &Bounds, b: &Bounds) -> f32 {
    let common = a.intersection(b).map_or(0.0, |c| c.width() * c.height());
    let union = a.width() * a.height() + b.width() * b.height() - common;
    if union > 0.0 { common / union } else { 0.0 }
}

/// Correct, wrong and unreported values of one property.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Tally {
    pub correct: usize,
    pub wrong: usize,
    /// The truth has a value, the prediction none.
    pub missing: usize,
}

impl Tally {
    fn add(&mut self, other: Tally) {
        self.correct += other.correct;
        self.wrong += other.wrong;
        self.missing += other.missing;
    }

    fn record<T: PartialEq>(&mut self, truth: Option<T>, predicted: Option<T>) {
        match (truth, predicted) {
            (None, _) => {}
            (Some(_), None) => self.missing += 1,
            (Some(t), Some(p)) if t == p => self.correct += 1,
            (Some(_), Some(_)) => self.wrong += 1,
        }
    }

    /// Correct among the reported values.
    pub fn accuracy(&self) -> Option<f64> {
        ratio(self.correct, self.correct + self.wrong)
    }

    /// Reported among the values the truth has.
    pub fn coverage(&self) -> Option<f64> {
        ratio(self.correct + self.wrong, self.correct + self.wrong + self.missing)
    }

    /// Correct among the values the truth has.
    pub fn recall(&self) -> Option<f64> {
        ratio(self.correct, self.correct + self.wrong + self.missing)
    }
}

/// Exact and approximate agreement of a text property.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct TextTally {
    /// Pairs whose truth has the text.
    pub total: usize,
    pub exact: usize,
    /// Sum of `1 - edit distance / length` (0 when the prediction has none).
    pub similarity: f64,
}

impl TextTally {
    fn add(&mut self, other: TextTally) {
        self.total += other.total;
        self.exact += other.exact;
        self.similarity += other.similarity;
    }

    fn record(&mut self, truth: Option<&str>, predicted: Option<&str>) {
        let Some(truth) = truth.map(clean).filter(|text| !text.is_empty()) else {
            return;
        };
        let predicted = predicted.map(clean).unwrap_or_default();
        self.total += 1;
        if predicted == truth {
            self.exact += 1;
        }
        self.similarity += similarity(&truth, &predicted);
    }

    pub fn exact_rate(&self) -> Option<f64> {
        ratio(self.exact, self.total)
    }

    pub fn mean_similarity(&self) -> Option<f64> {
        (self.total > 0).then(|| self.similarity / self.total as f64)
    }
}

/// One confidence bin.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Bin {
    pub count: usize,
    pub correct: usize,
    pub confidence: f64,
}

/// All counts of a comparison.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Counts {
    /// Significant true elements.
    pub truth: usize,
    /// Of them, paired with a prediction (true positives).
    pub found: usize,
    /// Predicted non-structural elements without a true counterpart
    /// (invented, or duplicates of a found element).
    pub false_positives: usize,
    /// Of the false positives, duplicates of a true element.
    pub duplicates: usize,
    /// Unpaired predictions inside a true element (neutral).
    pub parts: usize,
    /// Pairs made by containment (second pass), not overlap.
    pub contained: usize,
    /// Significant true elements and those found, by role.
    pub by_role: BTreeMap<String, (usize, usize)>,
    pub role: Tally,
    pub name: TextTally,
    pub value: TextTally,
    /// Sum of IoU over the pairs.
    pub iou: f64,
    /// Pairs with IoU ≥ [`PRECISE_IOU`].
    pub precise: usize,
    pub states: BTreeMap<String, Tally>,
    /// Pairs whose true parent was also found: parent predicted right.
    pub parent: Tally,
    /// Pairs of found table/list cells: same row predicted as in the truth.
    pub rows: Tally,
    /// True label relations whose ends were found.
    pub labels: Tally,
    /// Element confidence against being a true positive.
    pub calibration: [Bin; BINS],
    /// Across steps: true elements found in both steps, with the same ID
    /// (`correct`), another element's ID (`wrong`), or a new ID (`missing`).
    pub identity: Tally,
}

impl Counts {
    pub fn add(&mut self, other: &Counts) {
        self.truth += other.truth;
        self.found += other.found;
        self.false_positives += other.false_positives;
        self.duplicates += other.duplicates;
        self.parts += other.parts;
        self.contained += other.contained;
        for (role, (truth, found)) in &other.by_role {
            let entry = self.by_role.entry(role.clone()).or_default();
            entry.0 += truth;
            entry.1 += found;
        }
        self.role.add(other.role);
        self.name.add(other.name);
        self.value.add(other.value);
        self.iou += other.iou;
        self.precise += other.precise;
        for (state, tally) in &other.states {
            self.states.entry(state.clone()).or_default().add(*tally);
        }
        self.parent.add(other.parent);
        self.rows.add(other.rows);
        self.labels.add(other.labels);
        for (bin, other) in self.calibration.iter_mut().zip(&other.calibration) {
            bin.count += other.count;
            bin.correct += other.correct;
            bin.confidence += other.confidence;
        }
        self.identity.add(other.identity);
    }

    pub fn recall(&self) -> Option<f64> {
        ratio(self.found, self.truth)
    }

    pub fn precision(&self) -> Option<f64> {
        ratio(self.found, self.found + self.false_positives)
    }

    pub fn mean_iou(&self) -> Option<f64> {
        (self.found > 0).then(|| self.iou / self.found as f64)
    }

    pub fn precise_rate(&self) -> Option<f64> {
        ratio(self.precise, self.found)
    }

    /// Expected calibration error: the mean gap between confidence and
    /// accuracy, weighted by bin size.
    pub fn calibration_error(&self) -> Option<f64> {
        let total: usize = self.calibration.iter().map(|bin| bin.count).sum();
        (total > 0).then(|| {
            self.calibration
                .iter()
                .filter(|bin| bin.count > 0)
                .map(|bin| {
                    let accuracy = bin.correct as f64 / bin.count as f64;
                    let confidence = bin.confidence / bin.count as f64;
                    bin.count as f64 / total as f64 * (accuracy - confidence).abs()
                })
                .sum()
        })
    }
}

pub(crate) fn ratio(part: usize, whole: usize) -> Option<f64> {
    (whole > 0).then(|| part as f64 / whole as f64)
}

/// Pairs of truth and prediction indices.
pub type Pairs = Vec<(usize, usize)>;

/// Pairs true and predicted elements one to one by overlap.
pub fn pair(truth: &Truth, observation: &Observation) -> Pairs {
    let mut candidates: Vec<(f32, usize, usize)> = Vec::new();
    for (t, truth) in truth.elements.iter().enumerate() {
        for (p, predicted) in observation.elements.iter().enumerate() {
            let overlap = iou(&truth.bounds, shown(predicted));
            if overlap >= MATCH_IOU {
                candidates.push((overlap, t, p));
            }
        }
    }
    // Largest overlap first; among equals, the significant truth first.
    candidates.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| truth.elements[b.1].significant.cmp(&truth.elements[a.1].significant))
    });
    let mut truth_used = vec![false; truth.elements.len()];
    let mut predicted_used = vec![false; observation.elements.len()];
    let mut pairs = Vec::new();
    for (_, t, p) in candidates {
        if !truth_used[t] && !predicted_used[p] {
            truth_used[t] = true;
            predicted_used[p] = true;
            pairs.push((t, p));
        }
    }
    // Second pass: a control found by its visual part only (the box of a
    // checkbox whose accessibility bounds include the label) — a prediction
    // of the same role lying inside it.
    for (t, expected) in truth.elements.iter().enumerate() {
        if truth_used[t] || !expected.significant {
            continue;
        }
        let inside = observation
            .elements
            .iter()
            .enumerate()
            .filter(|(p, predicted)| {
                !predicted_used[*p]
                    && predicted.role == expected.role
                    && inside(shown(predicted), &expected.bounds) >= CONTAINED
            })
            .max_by(|a, b| {
                iou(&expected.bounds, shown(a.1)).total_cmp(&iou(&expected.bounds, shown(b.1)))
            });
        if let Some((p, _)) = inside {
            truth_used[t] = true;
            predicted_used[p] = true;
            pairs.push((t, p));
        }
    }
    pairs.sort_unstable();
    pairs
}

/// The share of `part` that lies inside `whole`.
fn inside(part: &Bounds, whole: &Bounds) -> f32 {
    let area = part.width() * part.height();
    let common = part.intersection(whole).map_or(0.0, |c| c.width() * c.height());
    if area > 0.0 { common / area } else { 0.0 }
}

/// The on-screen part of an element.
fn shown(element: &Element) -> &Bounds {
    element.visible_bounds.as_ref().unwrap_or(&element.bounds)
}

/// Compares one observation with the truth of its step.
pub fn compare(truth: &Truth, observation: &Observation, pairs: &Pairs) -> Counts {
    let mut counts = Counts::default();
    let by_truth: HashMap<usize, usize> = pairs.iter().copied().collect();
    let by_prediction: HashMap<usize, usize> = pairs.iter().map(|&(t, p)| (p, t)).collect();
    let truth_index: HashMap<&str, usize> =
        truth.elements.iter().enumerate().map(|(i, e)| (e.id.as_str(), i)).collect();
    let predicted_index: HashMap<&str, usize> =
        observation.elements.iter().enumerate().map(|(i, e)| (e.id.as_str(), i)).collect();

    for (t, expected) in truth.elements.iter().enumerate() {
        if !expected.significant {
            continue;
        }
        counts.truth += 1;
        let role = counts.by_role.entry(role_name(expected.role)).or_default();
        role.0 += 1;
        let Some(&p) = by_truth.get(&t) else { continue };
        role.1 += 1;
        counts.found += 1;
        let predicted = &observation.elements[p];
        counts.role.record(Some(expected.role), Some(predicted.role));
        counts.name.record(expected.name.as_deref(), predicted.name.as_deref());
        counts.value.record(expected.value.as_deref(), predicted.value.as_deref());
        let overlap = iou(&expected.bounds, shown(predicted));
        if overlap < MATCH_IOU {
            counts.contained += 1;
        }
        counts.iou += f64::from(overlap);
        if overlap >= PRECISE_IOU {
            counts.precise += 1;
        }
        compare_states(&mut counts, expected, predicted);

        // The parent, when it was found.
        let parent = expected.parent.as_deref().and_then(|id| truth_index.get(id));
        if let Some(parent) = parent.and_then(|parent| by_truth.get(parent)) {
            let predicted_parent =
                predicted.parent.as_ref().and_then(|id| predicted_index.get(id.as_str()));
            counts.parent.record(Some(*parent), predicted_parent.copied());
        }
    }

    // Row membership, over pairs of found elements that belong to rows.
    let rows: Vec<(usize, usize, usize)> = pairs
        .iter()
        .filter(|(t, _)| truth.elements[*t].significant)
        .filter_map(|&(t, p)| {
            let truth_row = row_of_truth(truth, &truth_index, t)?;
            Some((t, p, truth_row))
        })
        .collect();
    for (i, &(_, p, row)) in rows.iter().enumerate() {
        for &(_, q, other_row) in &rows[i + 1..] {
            let same = row == other_row;
            let predicted = match (
                row_of(observation, &predicted_index, p),
                row_of(observation, &predicted_index, q),
            ) {
                (Some(a), Some(b)) => Some(a == b),
                // Without rows, nothing is together.
                _ => Some(false),
            };
            counts.rows.record(Some(same), predicted);
        }
    }

    // Label relations of the truth.
    for relation in &truth.relations {
        if relation.kind != RelationKind::LabelFor {
            continue;
        }
        let (Some(&from), Some(&to)) =
            (truth_index.get(relation.from.as_str()), truth_index.get(relation.to.as_str()))
        else {
            continue;
        };
        let (Some(&from), Some(&to)) = (by_truth.get(&from), by_truth.get(&to)) else {
            continue;
        };
        let (from, to) = (&observation.elements[from].id, &observation.elements[to].id);
        let found = observation
            .relations
            .iter()
            .any(|r| r.kind == RelationKind::LabelFor && &r.from == from && &r.to == to);
        counts.labels.record(Some(true), found.then_some(true));
    }

    // Precision and calibration over predicted non-structural elements.
    for (p, predicted) in observation.elements.iter().enumerate() {
        let correct = match by_prediction.get(&p) {
            Some(&t) if truth.elements[t].significant => true,
            Some(_) => continue, // a container of the truth: neutral
            None if is_structural(predicted.role) => continue,
            None => {
                let bounds = shown(predicted);
                let duplicate = truth
                    .elements
                    .iter()
                    .any(|t| t.significant && iou(&t.bounds, bounds) >= MATCH_IOU);
                let part = truth
                    .elements
                    .iter()
                    .any(|t| !is_structural(t.role) && inside(bounds, &t.bounds) >= CONTAINED);
                if part && !duplicate {
                    // A part of a real element (the label inside a
                    // checkbox, the text in a field): not invented.
                    counts.parts += 1;
                    continue;
                }
                if duplicate {
                    counts.duplicates += 1;
                }
                counts.false_positives += 1;
                false
            }
        };
        let confidence = f64::from(predicted.confidence.element.get());
        let bin = &mut counts.calibration[((confidence * BINS as f64) as usize).min(BINS - 1)];
        bin.count += 1;
        bin.confidence += confidence;
        if correct {
            bin.correct += 1;
        }
    }
    counts
}

fn compare_states(counts: &mut Counts, expected: &TruthElement, predicted: &Element) {
    let (t, p) = (&expected.state, &predicted.state);
    let mut record = |name: &str, truth: Option<bool>, predicted: Option<bool>| {
        counts.states.entry(name.to_owned()).or_default().record(truth, predicted);
    };
    record("enabled", t.enabled, p.enabled);
    record("selected", t.selected, p.selected);
    record("focused", t.focused, p.focused);
    record("expanded", t.expanded, p.expanded);
    let checked = |state: Option<CheckState>| state.map(|s| s == CheckState::Checked);
    record("checked", checked(t.checked), checked(p.checked));
}

/// The row a true element belongs to: its nearest ancestor with the row
/// role.
fn row_of_truth(truth: &Truth, index: &HashMap<&str, usize>, mut t: usize) -> Option<usize> {
    for _ in 0..64 {
        let parent = *index.get(truth.elements[t].parent.as_deref()?)?;
        if truth.elements[parent].role == Role::Row {
            return Some(parent);
        }
        t = parent;
    }
    None
}

/// The row a predicted element belongs to: its nearest row ancestor, or
/// the target of its `belongs_to_row` relation.
fn row_of(observation: &Observation, index: &HashMap<&str, usize>, p: usize) -> Option<usize> {
    let id = &observation.elements[p].id;
    if let Some(relation) =
        observation.relations.iter().find(|r| r.kind == RelationKind::BelongsToRow && &r.from == id)
    {
        return index.get(relation.to.as_str()).copied();
    }
    let mut current = p;
    for _ in 0..64 {
        let parent = *index.get(observation.elements[current].parent.as_ref()?.as_str())?;
        if observation.elements[parent].role == Role::Row {
            return Some(parent);
        }
        current = parent;
    }
    None
}

/// Collapses whitespace and trims.
fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `1 - edit distance / longer length`, over characters.
fn similarity(a: &str, b: &str) -> f64 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let longest = a.len().max(b.len());
    if longest == 0 {
        return 1.0;
    }
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous + usize::from(ca != cb);
            previous = row[j + 1];
            row[j + 1] = substitution.min(row[j] + 1).min(row[j + 1] + 1);
        }
    }
    1.0 - row[b.len()] as f64 / longest as f64
}

/// Human-readable mistakes of one observation: true elements not found,
/// wrong texts of found ones, and predictions that are false positives.
pub fn explain(truth: &Truth, observation: &Observation, pairs: &Pairs) -> Vec<String> {
    let found: std::collections::HashSet<usize> = pairs.iter().map(|&(t, _)| t).collect();
    let paired: std::collections::HashSet<usize> = pairs.iter().map(|&(_, p)| p).collect();
    let describe = |role: Role, name: Option<&str>, bounds: &Bounds| {
        format!(
            "{} {:?} at ({:.0}, {:.0}, {:.0}×{:.0})",
            role_name(role),
            name.unwrap_or(""),
            bounds.x(),
            bounds.y(),
            bounds.width(),
            bounds.height()
        )
    };
    let mut lines = Vec::new();
    for (t, expected) in truth.elements.iter().enumerate() {
        if expected.significant && !found.contains(&t) {
            lines.push(format!(
                "missed          {} [{}]",
                describe(expected.role, expected.name.as_deref(), &expected.bounds),
                expected.id
            ));
        }
    }
    for &(t, p) in pairs {
        let (expected, predicted) = (&truth.elements[t], &observation.elements[p]);
        if !expected.significant {
            continue;
        }
        for (property, truth_text, predicted_text) in [
            ("name", &expected.name, &predicted.name),
            ("value", &expected.value, &predicted.value),
        ] {
            let Some(truth_text) = truth_text.as_deref().map(clean).filter(|t| !t.is_empty())
            else {
                continue;
            };
            let predicted_text = predicted_text.as_deref().map(clean).unwrap_or_default();
            if truth_text != predicted_text {
                lines.push(format!(
                    "wrong {property:<9} {} {truth_text:?} → {predicted_text:?} [{}]",
                    role_name(expected.role),
                    expected.id
                ));
            }
        }
    }
    for (p, predicted) in observation.elements.iter().enumerate() {
        if paired.contains(&p) || is_structural(predicted.role) {
            continue;
        }
        let bounds = shown(predicted);
        let part = truth
            .elements
            .iter()
            .any(|t| !is_structural(t.role) && inside(bounds, &t.bounds) >= CONTAINED);
        let duplicate =
            truth.elements.iter().any(|t| t.significant && iou(&t.bounds, bounds) >= MATCH_IOU);
        if part && !duplicate {
            continue;
        }
        lines.push(format!(
            "{}{} {} conf {:.2} {:?}",
            if duplicate { "duplicate       " } else { "false positive  " },
            predicted.id,
            describe(predicted.role, predicted.name.as_deref(), bounds),
            predicted.confidence.element.get(),
            predicted.sources
        ));
    }
    lines
}

/// Identity across two consecutive steps: for true elements found in both,
/// whether the predicted IDs agree.
pub fn compare_identity(
    before: (&Truth, &Observation, &Pairs),
    after: (&Truth, &Observation, &Pairs),
) -> Tally {
    let ids =
        |(truth, observation, pairs): (&Truth, &Observation, &Pairs)| -> HashMap<String, String> {
            pairs
                .iter()
                .filter(|(t, _)| truth.elements[*t].significant)
                .map(|&(t, p)| {
                    (truth.elements[t].id.clone(), observation.elements[p].id.as_str().to_owned())
                })
                .collect()
        };
    let (before, after) = (ids(before), ids(after));
    let owner: HashMap<&str, &str> =
        before.iter().map(|(truth, predicted)| (predicted.as_str(), truth.as_str())).collect();
    let mut tally = Tally::default();
    for (truth, predicted) in &after {
        let Some(earlier) = before.get(truth) else {
            // New in this step: wrong only if it took another element's ID.
            if owner.contains_key(predicted.as_str()) {
                tally.wrong += 1;
            }
            continue;
        };
        if earlier == predicted {
            tally.correct += 1;
        } else if owner.get(predicted.as_str()).is_some_and(|other| other != truth) {
            tally.wrong += 1;
        } else {
            tally.missing += 1;
        }
    }
    tally
}

#[cfg(test)]
mod tests {
    use argus_protocol::{Confidence, ElementId, ElementState, ObservationId, Score, Timestamp};

    use super::*;

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds {
        Bounds::new(x, y, w, h).unwrap()
    }

    fn truth(id: &str, role: Role, b: Bounds, significant: bool) -> TruthElement {
        TruthElement {
            id: id.to_owned(),
            role,
            name: Some(id.to_owned()),
            value: None,
            bounds: b,
            state: ElementState::default(),
            parent: None,
            significant,
            note: None,
        }
    }

    fn element(id: &str, role: Role, b: Bounds, name: &str, confidence: f32) -> Element {
        Element {
            id: ElementId::new(id).unwrap(),
            role,
            name: Some(name.to_owned()),
            value: None,
            description: None,
            bounds: b,
            visible_bounds: None,
            state: ElementState::default(),
            confidence: Confidence::new(Score::new(confidence).unwrap()),
            sources: Vec::new(),
            parent: None,
            children: Vec::new(),
        }
    }

    fn observation(elements: Vec<Element>) -> Observation {
        let mut observation = Observation::new(ObservationId::new("o").unwrap(), Timestamp(0));
        observation.elements = elements;
        observation
    }

    #[test]
    fn pairs_count_as_found_and_the_rest_as_false_positives() {
        let truth = Truth {
            elements: vec![
                truth("window", Role::Window, bounds(0.0, 0.0, 200.0, 100.0), false),
                truth("ok", Role::Button, bounds(10.0, 10.0, 40.0, 20.0), true),
                truth("cancel", Role::Button, bounds(60.0, 10.0, 40.0, 20.0), true),
            ],
            relations: Vec::new(),
        };
        let predicted = observation(vec![
            element("e_1", Role::Window, bounds(0.0, 0.0, 200.0, 100.0), "window", 1.0),
            // Found, slightly off, misnamed and with another role.
            element("e_2", Role::Text, bounds(11.0, 10.0, 40.0, 20.0), "OK", 0.95),
            // A duplicate of the same button.
            element("e_3", Role::Button, bounds(12.0, 11.0, 38.0, 19.0), "ok", 0.35),
            // Invented.
            element("e_4", Role::Button, bounds(150.0, 60.0, 20.0, 20.0), "?", 0.25),
        ]);
        let pairs = pair(&truth, &predicted);
        let counts = compare(&truth, &predicted, &pairs);
        assert_eq!((counts.truth, counts.found, counts.false_positives), (2, 1, 2));
        assert_eq!(counts.recall(), Some(0.5));
        assert_eq!(counts.precision(), Some(1.0 / 3.0));
        assert_eq!((counts.role.correct, counts.role.wrong), (0, 1));
        assert_eq!(counts.name.exact, 0);
        assert_eq!(counts.name.similarity, 0.0, "case-sensitive: ok vs OK");
        assert_eq!(counts.by_role["button"], (2, 1));
        // Calibration: 0.95 correct, 0.35 and 0.25 wrong.
        assert_eq!(counts.calibration[9].correct, 1);
        assert_eq!(counts.calibration[3].count, 1);
        assert!(counts.calibration_error().unwrap() > 0.0);
    }

    #[test]
    fn identities_are_compared_across_steps() {
        let truth = |ids: &[&str]| Truth {
            elements: ids
                .iter()
                .enumerate()
                .map(|(i, id)| {
                    truth(id, Role::Button, bounds(i as f32 * 50.0, 0.0, 40.0, 20.0), true)
                })
                .collect(),
            relations: Vec::new(),
        };
        let predicted = |ids: &[&str]| {
            observation(
                ids.iter()
                    .enumerate()
                    .map(|(i, id)| {
                        element(id, Role::Button, bounds(i as f32 * 50.0, 0.0, 40.0, 20.0), "", 1.0)
                    })
                    .collect(),
            )
        };
        let (t1, p1) = (truth(&["a", "b", "c"]), predicted(&["e_1", "e_2", "e_3"]));
        // b kept its ID, a took c's ID, c got a new one, d is new with a's.
        let (t2, p2) = (truth(&["a", "b", "c", "d"]), predicted(&["e_3", "e_2", "e_9", "e_1"]));
        let (q1, q2) = (pair(&t1, &p1), pair(&t2, &p2));
        let tally = compare_identity((&t1, &p1, &q1), (&t2, &p2, &q2));
        assert_eq!((tally.correct, tally.wrong, tally.missing), (1, 2, 1));
    }

    #[test]
    fn similarity_is_edit_based() {
        assert_eq!(similarity("Save", "Save"), 1.0);
        assert_eq!(similarity("Save", ""), 0.0);
        assert!((similarity("Delete", "Delet") - 5.0 / 6.0).abs() < 1e-9);
        assert_eq!(clean("  a \n b "), "a b");
    }
}
