//! What tracking compares about an element.

use std::collections::HashMap;

use argus_protocol::{Observation, Role, Source};

/// Most descendant names kept as an element's content.
const CONTENT_LIMIT: usize = 16;

/// A rectangle relative to the window origin, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rect {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

impl Rect {
    pub(crate) fn center(&self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

/// The properties of one element that identify it across observations.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Profile {
    pub(crate) role: Role,
    /// Reported by a structured source, whose roles are facts rather than
    /// visual hypotheses.
    pub(crate) structured: bool,
    pub(crate) name: Option<String>,
    pub(crate) native_id: Option<String>,
    /// Bounds relative to the window, so that moving the window moves
    /// nothing.
    pub(crate) bounds: Rect,
    /// Names of the first descendants, sorted and deduplicated: what an
    /// unnamed container (a row, a group) shows.
    pub(crate) content: Vec<String>,
    /// The only element of its role among its siblings.
    pub(crate) sole: bool,
    /// Position among the siblings of the same role, counted from the first
    /// and from the last.
    pub(crate) order: (usize, usize),
}

/// Profiles of every element of `observation` and the index of each
/// element's parent.
pub(crate) fn profiles(
    observation: &Observation,
    native_ids: &[Option<String>],
) -> (Vec<Profile>, Vec<Option<usize>>) {
    let (origin_x, origin_y) = observation
        .window
        .as_ref()
        .and_then(|window| window.bounds)
        .map_or((0.0, 0.0), |b| (b.x(), b.y()));
    let index: HashMap<&str, usize> = observation
        .elements
        .iter()
        .enumerate()
        .map(|(position, element)| (element.id.as_str(), position))
        .collect();
    let parents: Vec<Option<usize>> = observation
        .elements
        .iter()
        .map(|element| {
            element.parent.as_ref().and_then(|parent| index.get(parent.as_str())).copied()
        })
        .collect();

    let mut profiles: Vec<Profile> = observation
        .elements
        .iter()
        .enumerate()
        .map(|(position, element)| Profile {
            role: element.role,
            structured: element
                .sources
                .iter()
                .any(|source| matches!(source, Source::Accessibility | Source::ApplicationApi)),
            name: element.name.clone(),
            native_id: native_ids.get(position).cloned().flatten(),
            bounds: Rect {
                x: element.bounds.x() - origin_x,
                y: element.bounds.y() - origin_y,
                width: element.bounds.width(),
                height: element.bounds.height(),
            },
            content: Vec::new(),
            sole: false,
            order: (0, 0),
        })
        .collect();

    let mut roles: HashMap<(Option<usize>, Role), usize> = HashMap::new();
    for (position, element) in observation.elements.iter().enumerate() {
        let count = roles.entry((parents[position], element.role)).or_default();
        profiles[position].order.0 = *count;
        *count += 1;
    }
    for (position, element) in observation.elements.iter().enumerate() {
        let count = roles[&(parents[position], element.role)];
        let profile = &mut profiles[position];
        profile.sole = parents[position].is_some() && count == 1;
        profile.order.1 = count - 1 - profile.order.0;
    }

    // Elements are listed parents first, so the first descendants reach
    // their ancestors first.
    for (position, element) in observation.elements.iter().enumerate() {
        let Some(name) = &element.name else { continue };
        let mut ancestor = parents[position];
        let mut steps = 0;
        while let Some(current) = ancestor {
            if profiles[current].content.len() < CONTENT_LIMIT {
                profiles[current].content.push(name.clone());
            }
            ancestor = parents[current];
            steps += 1;
            if steps > observation.elements.len() {
                break; // a cycle in an invalid observation
            }
        }
    }
    for profile in &mut profiles {
        profile.content.sort_unstable();
        profile.content.dedup();
    }
    (profiles, parents)
}
