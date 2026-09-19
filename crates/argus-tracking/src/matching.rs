//! Scoring and assignment of current elements to known ones.

use std::collections::{HashMap, HashSet};

use argus_protocol::{ElementId, Role};

use crate::profile::{Profile, Rect};

/// Weight of the position and size of the element.
const GEOMETRY: f32 = 0.35;
/// Weight of the name (for an unnamed container: the names inside it).
const TEXT: f32 = 0.45;
/// Weight of the names inside an unnamed container.
const CONTENT: f32 = 0.2;
/// Weight of the parent's identity.
const PARENT: f32 = 0.2;
/// Weight of being the only element of its role in the same parent (the
/// display of a calculator, the title of a dialog).
const SOLE: f32 = 0.2;
/// Weight of the position among siblings of the same role, for elements
/// with nothing else to tell them apart (unnamed, empty).
const ORDER: f32 = 0.2;
/// Weight of a native identifier shared by both elements.
const NATIVE: f32 = 0.3;
/// Share of the name similarity credited to different names (a label that
/// changed in place, e.g. "Start" → "Stop").
const RENAMED: f32 = 0.3;

/// Score of a native identifier that is unique in both observations: the
/// application itself says the objects are the same.
const NATIVE_ANCHOR: f32 = 0.95;
/// Geometry similarity below which an element is elsewhere.
const FAR: f32 = 0.1;
/// Factor for a role change of an element seen only in pixels (visual role
/// hypotheses flip between frames). Structured roles never change.
const ROLE_CHANGE: f32 = 0.7;
/// Lowest score at which two elements are considered the same.
pub(crate) const MIN_SCORE: f32 = 0.5;
/// Offset, in element sizes, at which the geometry similarity falls to 1/e.
const SPREAD: f32 = 0.5;
/// Search reach, in element sizes.
const REACH: f32 = 3.0;
/// Smallest element size, in points: tiny glyphs jitter by a point or two.
const MIN_SIZE: f32 = 8.0;
/// Largest search radius, in points.
const MAX_RADIUS: f32 = 1500.0;
/// Names (and native identifiers) more frequent than this are not searched
/// for: they identify nothing.
const FANOUT: usize = 16;
/// Grid cell of the spatial indexes, in points.
const CELL: f32 = 32.0;
/// How far an anchor's motion is assumed to extend, in points.
const ANCHOR_RADIUS: f32 = 300.0;
/// Anchors whose motion is combined (median) for one element.
const ANCHORS: usize = 3;
/// Most passes with the parent term.
const PASSES: usize = 4;
/// Score margin over the best alternative assignment at which identity is
/// unambiguous. With no margin at all the identity is a coin toss.
const AMBIGUITY_SPAN: f32 = 0.25;

/// An element remembered from earlier observations.
#[derive(Debug, Clone)]
pub(crate) struct Known {
    pub(crate) id: ElementId,
    pub(crate) parent: Option<ElementId>,
    pub(crate) profile: Profile,
    /// Observations since the element was last seen (0: in the previous one).
    pub(crate) missed: u32,
}

/// A current element matched to a known one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Match {
    /// Index into the known elements.
    pub(crate) known: usize,
    /// Confidence that the identity holds.
    pub(crate) identity: f32,
}

/// Matches every current element to at most one known element, one to one.
pub(crate) fn assign(
    known: &[Known],
    current: &[Profile],
    parents: &[Option<usize>],
) -> Vec<Option<Match>> {
    let scorer = Scorer::new(known, current, parents);
    let candidates = scorer.candidates();

    // First without the parent term, then with the parents found, until the
    // assignment settles (a container recognized by its parent lets its own
    // children be recognized in the next pass).
    let scored = |matches: Option<&[Option<usize>]>| -> Vec<(usize, usize, f32)> {
        candidates
            .iter()
            .filter_map(|&(k, c)| Some((k, c, scorer.score(k, c, matches)?)))
            .filter(|&(_, _, score)| score >= MIN_SCORE)
            .collect()
    };
    let mut pairs = scored(None);
    let (mut by_current, mut by_known) = greedy(&pairs, known.len(), current.len());
    for _ in 0..PASSES {
        let next_pairs = scored(Some(&by_current));
        let (next, next_by_known) = greedy(&next_pairs, known.len(), current.len());
        let settled = next == by_current;
        (pairs, by_current, by_known) = (next_pairs, next, next_by_known);
        if settled {
            break;
        }
    }

    let scores: HashMap<(usize, usize), f32> =
        pairs.iter().map(|&(k, c, score)| ((k, c), score)).collect();
    let score = |k: usize, c: usize| scores.get(&(k, c)).copied().unwrap_or(0.0);
    let mut alternatives_of_known: Vec<Vec<usize>> = vec![Vec::new(); known.len()];
    let mut alternatives_of_current: Vec<Vec<usize>> = vec![Vec::new(); current.len()];
    for &(k, c, _) in &pairs {
        alternatives_of_known[k].push(c);
        alternatives_of_current[c].push(k);
    }

    by_current
        .iter()
        .enumerate()
        .map(|(c, matched)| {
            let k = (*matched)?;
            let kept = score(k, c);
            // How much worse the best assignment gets if this element
            // swapped partners with a competitor.
            let via_current =
                alternatives_of_known[k].iter().filter(|&&other| other != c).map(|&other| {
                    let partner = by_current[other];
                    let kept_other = partner.map_or(0.0, |p| score(p, other));
                    let swapped_other = partner.map_or(0.0, |p| score(p, c));
                    kept + kept_other - score(k, other) - swapped_other
                });
            let via_known =
                alternatives_of_current[c].iter().filter(|&&other| other != k).map(|&other| {
                    let partner = by_known[other];
                    let kept_other = partner.map_or(0.0, |p| score(other, p));
                    let swapped_other = partner.map_or(0.0, |p| score(k, p));
                    kept + kept_other - score(other, c) - swapped_other
                });
            let margin = via_current.chain(via_known).fold(f32::INFINITY, f32::min);
            let certainty = 0.5 + 0.5 * (margin / AMBIGUITY_SPAN).clamp(0.0, 1.0);
            Some(Match { known: k, identity: kept * certainty })
        })
        .collect()
}

/// Accepts pairs best first, each element at most once. Ties are broken by
/// position, so the result is deterministic.
fn greedy(
    pairs: &[(usize, usize, f32)],
    known: usize,
    current: usize,
) -> (Vec<Option<usize>>, Vec<Option<usize>>) {
    let mut order: Vec<&(usize, usize, f32)> = pairs.iter().collect();
    order.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
    let mut by_current = vec![None; current];
    let mut by_known = vec![None; known];
    for &&(k, c, _) in &order {
        if by_current[c].is_none() && by_known[k].is_none() {
            by_current[c] = Some(k);
            by_known[k] = Some(c);
        }
    }
    (by_current, by_known)
}

struct Scorer<'a> {
    known: &'a [Known],
    current: &'a [Profile],
    parents: &'a [Option<usize>],
    /// Native identifiers that occur once among the known and once among the
    /// current elements.
    unique_native: HashSet<&'a str>,
    /// Roles and names that occur once among the known and once among the
    /// current elements.
    unique_names: HashSet<(Role, &'a str)>,
    /// For every current element, the local motion of the content around it
    /// (e.g. a scrolled list), estimated from anchors.
    motion: Vec<(f32, f32)>,
}

/// Indexes of the elements carrying each key, among the known and the
/// current elements.
type Occurrences<K> = HashMap<K, (Vec<usize>, Vec<usize>)>;

impl<'a> Scorer<'a> {
    fn new(known: &'a [Known], current: &'a [Profile], parents: &'a [Option<usize>]) -> Self {
        let mut natives: Occurrences<&str> = HashMap::new();
        let mut names: Occurrences<(Role, &str)> = HashMap::new();
        for (k, known) in known.iter().enumerate() {
            let profile = &known.profile;
            if let Some(id) = &profile.native_id {
                natives.entry(id).or_default().0.push(k);
            }
            if let Some(name) = &profile.name {
                names.entry((profile.role, name)).or_default().0.push(k);
            }
        }
        for (c, profile) in current.iter().enumerate() {
            if let Some(id) = &profile.native_id {
                natives.entry(id).or_default().1.push(c);
            }
            if let Some(name) = &profile.name {
                names.entry((profile.role, name)).or_default().1.push(c);
            }
        }
        let unique = |(known, current): &(Vec<usize>, Vec<usize>)| {
            (known.len() == 1 && current.len() == 1).then(|| (known[0], current[0]))
        };
        let unique_native: HashSet<&str> = natives
            .iter()
            .filter(|(_, found)| unique(found).is_some())
            .map(|(id, _)| *id)
            .collect();
        let unique_names: HashSet<(Role, &str)> = names
            .iter()
            .filter(|(_, found)| unique(found).is_some())
            .map(|(key, _)| *key)
            .collect();

        // Anchors: leaf elements that are unmistakably the same in both
        // observations. Their displacement is the motion of their
        // surroundings. (A container's center says little about where its
        // content went.)
        let leaf = |profile: &Profile| profile.content.is_empty();
        let mut anchors: Vec<(usize, usize)> = natives
            .values()
            .chain(names.values())
            .filter_map(unique)
            .filter(|&(k, c)| {
                let (a, b) = (&known[k].profile, &current[c]);
                a.role == b.role && leaf(a) && leaf(b)
            })
            .collect();
        anchors.sort_unstable();
        anchors.dedup();
        let motion = motion(known, current, &anchors);
        Self { known, current, parents, unique_native, unique_names, motion }
    }

    /// Pairs worth scoring: elements near where the current element is, or
    /// was before the local motion, and elements sharing a name or a native
    /// identifier.
    fn candidates(&self) -> Vec<(usize, usize)> {
        let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        let mut names: HashMap<&str, Vec<usize>> = HashMap::new();
        let mut natives: HashMap<&str, Vec<usize>> = HashMap::new();
        for (k, known) in self.known.iter().enumerate() {
            let profile = &known.profile;
            grid.entry(cell(profile.bounds.center())).or_default().push(k);
            if let Some(name) = &profile.name {
                names.entry(name).or_default().push(k);
            }
            if let Some(id) = &profile.native_id {
                natives.entry(id).or_default().push(k);
            }
        }

        let mut pairs = Vec::new();
        let mut found = Vec::new();
        for (c, profile) in self.current.iter().enumerate() {
            found.clear();
            let reach_x = (REACH * profile.bounds.width.max(MIN_SIZE)).min(MAX_RADIUS);
            let reach_y = (REACH * profile.bounds.height.max(MIN_SIZE)).min(MAX_RADIUS);
            let (x, y) = profile.bounds.center();
            let (dx, dy) = self.motion[c];
            let mut search = |x: f32, y: f32| {
                let (low_x, low_y) = cell((x - reach_x, y - reach_y));
                let (high_x, high_y) = cell((x + reach_x, y + reach_y));
                for cell_x in low_x..=high_x {
                    for cell_y in low_y..=high_y {
                        if let Some(list) = grid.get(&(cell_x, cell_y)) {
                            found.extend_from_slice(list);
                        }
                    }
                }
            };
            search(x, y);
            if (dx, dy) != (0.0, 0.0) {
                search(x - dx, y - dy);
            }
            for list in [
                profile.name.as_deref().and_then(|name| names.get(name)),
                profile.native_id.as_deref().and_then(|id| natives.get(id)),
            ]
            .into_iter()
            .flatten()
            {
                if list.len() <= FANOUT {
                    found.extend_from_slice(list);
                }
            }
            found.sort_unstable();
            found.dedup();
            pairs.extend(found.iter().map(|&k| (k, c)));
        }
        pairs
    }

    /// How likely known element `k` and current element `c` are the same
    /// object, or `None` if they cannot be. `matches` maps current elements
    /// to known ones and enables the parent term.
    fn score(&self, k: usize, c: usize, matches: Option<&[Option<usize>]>) -> Option<f32> {
        let known = &self.known[k];
        let (a, b) = (&known.profile, &self.current[c]);

        let role = if a.role == b.role {
            1.0
        } else if a.structured && b.structured {
            return None;
        } else {
            ROLE_CHANGE
        };
        // A shared identifier is strong evidence. Different identifiers are
        // not evidence against: applications encode state in them (e.g.
        // `StandardInputView;value:42`).
        let native = match (&a.native_id, &b.native_id) {
            (Some(x), Some(y)) if x == y => Some(x.as_str()),
            _ => None,
        };
        // An element whose identifier is unique in both observations belongs
        // with its namesake.
        let anchored = |profile: &Profile| {
            profile.native_id.as_deref().is_some_and(|id| self.unique_native.contains(id))
        };
        if native.is_none() && (anchored(a) || anchored(b)) {
            return None;
        }

        let mut total = 0.0;
        let mut weight = 0.0;
        let mut add = |w: f32, value: f32| {
            total += w * value;
            weight += w;
        };
        // Where the element is, or would be had it moved with the content
        // around it or with its parent.
        let fitted = a.role == Role::Text && b.role == Role::Text;
        let shifted = |(dx, dy): (f32, f32)| {
            let moved = Rect { x: a.bounds.x + dx, y: a.bounds.y + dy, ..a.bounds };
            geometry(&moved, &b.bounds, fitted)
        };
        let mut place = geometry(&a.bounds, &b.bounds, fitted).max(shifted(self.motion[c]));
        if let Some(parent) = self.parents[c] {
            if let Some(matched) = matches.and_then(|matches| matches[parent]) {
                let (x, y) = self.current[parent].bounds.center();
                let (kx, ky) = self.known[matched].profile.bounds.center();
                place = place.max(shifted((x - kx, y - ky)));
            }
        }
        // Far from where it was, only something distinctive is recognized.
        let distinctive = native.is_some_and(|id| self.unique_native.contains(id))
            || a.name.as_deref().is_some_and(|name| {
                a.name == b.name && self.unique_names.contains(&(a.role, name))
            });
        if place < FAR && !distinctive {
            return None;
        }
        add(GEOMETRY, place);
        if a.name.is_some() || b.name.is_some() {
            add(TEXT, text(a.name.as_deref(), b.name.as_deref()));
        } else if !a.content.is_empty() || !b.content.is_empty() {
            // What an unnamed container shows identifies it, but content
            // changes (a display, a list) more often than names do.
            add(CONTENT, overlap(&a.content, &b.content));
        }
        if let Some(matches) = matches {
            match (&known.parent, self.parents[c]) {
                (None, None) => add(PARENT, 1.0),
                (Some(parent), Some(current)) => {
                    // Unknown when the current parent is itself new.
                    if let Some(matched) = matches[current] {
                        let same = self.known[matched].id == *parent;
                        add(PARENT, f32::from(u8::from(same)));
                        if same && a.sole && b.sole {
                            add(SOLE, 1.0);
                        }
                        // Pixel elements are ordered by position, so their
                        // order tells nothing new.
                        let featureless =
                            |p: &Profile| p.structured && p.name.is_none() && p.content.is_empty();
                        if same && featureless(a) && featureless(b) {
                            let first_or_last = a.order.0 == b.order.0 || a.order.1 == b.order.1;
                            add(ORDER, f32::from(u8::from(first_or_last)));
                        }
                    }
                }
                _ => add(PARENT, 0.0),
            }
        }
        if native.is_some() {
            add(NATIVE, 1.0);
        }

        let mut score = role * total / weight;
        if native.is_some_and(|id| self.unique_native.contains(id)) {
            score = score.max(NATIVE_ANCHOR);
        }
        Some(score)
    }
}

/// The local motion around every current element: the median displacement
/// of the anchors inside it, or else of the nearest anchors within
/// [`ANCHOR_RADIUS`] of its bounds, or none.
fn motion(known: &[Known], current: &[Profile], anchors: &[(usize, usize)]) -> Vec<(f32, f32)> {
    const ANCHOR_CELL: f32 = 64.0;
    let key = |value: f32| (value / ANCHOR_CELL).floor() as i32;
    /// An anchor's current center and displacement.
    struct Anchor {
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
    }
    let mut grid: HashMap<(i32, i32), Vec<Anchor>> = HashMap::new();
    for &(k, c) in anchors {
        let (x, y) = current[c].bounds.center();
        let (kx, ky) = known[k].profile.bounds.center();
        grid.entry((key(x), key(y))).or_default().push(Anchor { x, y, dx: x - kx, dy: y - ky });
    }
    let mut near: Vec<(f32, f32, f32)> = Vec::new();
    current
        .iter()
        .map(|profile| {
            let r = &profile.bounds;
            near.clear();
            for gx in key(r.x - ANCHOR_RADIUS)..=key(r.x + r.width + ANCHOR_RADIUS) {
                for gy in key(r.y - ANCHOR_RADIUS)..=key(r.y + r.height + ANCHOR_RADIUS) {
                    for anchor in grid.get(&(gx, gy)).into_iter().flatten() {
                        let outside_x = (r.x - anchor.x).max(anchor.x - r.x - r.width).max(0.0);
                        let outside_y = (r.y - anchor.y).max(anchor.y - r.y - r.height).max(0.0);
                        let distance = outside_x.hypot(outside_y);
                        if distance <= ANCHOR_RADIUS {
                            near.push((distance, anchor.dx, anchor.dy));
                        }
                    }
                }
            }
            if near.is_empty() {
                return (0.0, 0.0);
            }
            near.sort_by(|a, b| {
                a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.total_cmp(&b.2))
            });
            let inside = near.iter().take_while(|n| n.0 == 0.0).count();
            near.truncate(inside.max(ANCHORS));
            let median = |mut values: Vec<f32>| {
                values.sort_by(f32::total_cmp);
                values[values.len() / 2]
            };
            (median(near.iter().map(|n| n.1).collect()), median(near.iter().map(|n| n.2).collect()))
        })
        .collect()
}

fn cell((x, y): (f32, f32)) -> (i32, i32) {
    ((x / CELL).floor() as i32, (y / CELL).floor() as i32)
}

/// Similarity of place and size, `0..=1`. Offsets are measured in element
/// sizes per axis, so a row that moved by its own height is elsewhere even
/// though it is wide. The offset of an axis is the smallest of its start,
/// center and end offsets: right-aligned text that grows keeps its end.
/// The width of `fitted` elements (text) follows their content and is not
/// compared.
fn geometry(a: &Rect, b: &Rect, fitted: bool) -> f32 {
    let width = ((a.width + b.width) / 2.0).max(MIN_SIZE);
    let height = ((a.height + b.height) / 2.0).max(MIN_SIZE);
    let offset = |start_a: f32, size_a: f32, start_b: f32, size_b: f32| {
        let start = (start_a - start_b).abs();
        let center = (start_a + size_a / 2.0 - start_b - size_b / 2.0).abs();
        let end = (start_a + size_a - start_b - size_b).abs();
        start.min(center).min(end)
    };
    let dx = offset(a.x, a.width, b.x, b.width) / width / SPREAD;
    let dy = offset(a.y, a.height, b.y, b.height) / height / SPREAD;
    let ratio = |p: f32, q: f32| (p.min(q) + 1.0) / (p.max(q) + 1.0);
    let width_ratio = if fitted { 1.0 } else { ratio(a.width, b.width) };
    let size = width_ratio * ratio(a.height, b.height);
    (-(dx * dx + dy * dy)).exp() * size.sqrt()
}

/// Similarity of two names, `0..=1`. A missing name is unknown, not
/// different.
fn text(a: Option<&str>, b: Option<&str>) -> f32 {
    match (a, b) {
        (Some(a), Some(b)) if a == b => 1.0,
        (Some(a), Some(b)) if a.to_lowercase() == b.to_lowercase() => 0.9,
        (Some(a), Some(b)) => RENAMED * dice(a, b),
        _ => 0.5,
    }
}

/// Dice coefficient of character bigrams.
fn dice(a: &str, b: &str) -> f32 {
    let bigrams = |text: &str| {
        let chars: Vec<char> = text.to_lowercase().chars().collect();
        let mut pairs: Vec<(char, char)> = chars.windows(2).map(|w| (w[0], w[1])).collect();
        pairs.sort_unstable();
        pairs
    };
    let (a, b) = (bigrams(a), bigrams(b));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let (mut i, mut j, mut common) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                common += 1;
                i += 1;
                j += 1;
            }
        }
    }
    2.0 * common as f32 / (a.len() + b.len()) as f32
}

/// Jaccard similarity of two sorted, deduplicated lists.
fn overlap(a: &[String], b: &[String]) -> f32 {
    let common = a.iter().filter(|name| b.binary_search(name).is_ok()).count();
    let union = a.len() + b.len() - common;
    if union == 0 { 1.0 } else { common as f32 / union as f32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect { x, y, width, height }
    }

    #[test]
    fn geometry_tolerates_jitter_but_not_moves() {
        let button = rect(10.0, 10.0, 50.0, 50.0);
        assert!((geometry(&button, &button, false) - 1.0).abs() < 1e-6);
        assert!(geometry(&button, &rect(12.0, 11.0, 49.0, 50.0), false) > 0.95);
        assert!(geometry(&button, &rect(60.0, 10.0, 50.0, 50.0), false) < 0.05, "the next key");
        assert!(geometry(&button, &rect(10.0, 10.0, 200.0, 50.0), false) < 0.6, "another shape");
        let grown = geometry(&rect(100.0, 0.0, 20.0, 36.0), &rect(64.0, 0.0, 56.0, 36.0), true);
        assert_eq!(grown, 1.0, "right-aligned text that grew");
        let row = rect(0.0, 0.0, 400.0, 20.0);
        assert!(geometry(&row, &rect(0.0, 20.0, 400.0, 20.0), false) < 0.05, "the next row");
    }

    #[test]
    fn text_similarity() {
        assert_eq!(text(Some("Save"), Some("Save")), 1.0);
        assert_eq!(text(Some("Save"), Some("save")), 0.9);
        assert_eq!(text(Some("7"), Some("8")), 0.0);
        assert_eq!(text(Some("Save"), None), 0.5);
        let edited = text(Some("Untitled"), Some("Untitled — Edited"));
        assert!(edited > 0.1 && edited < RENAMED, "{edited}");
    }

    #[test]
    fn content_overlap() {
        let list = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
        assert_eq!(overlap(&list(&["a", "b"]), &list(&["a", "b"])), 1.0);
        assert_eq!(overlap(&list(&["a", "b"]), &list(&["b", "c"])), 1.0 / 3.0);
        assert_eq!(overlap(&[], &[]), 1.0);
    }
}
