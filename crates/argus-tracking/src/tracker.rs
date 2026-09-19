//! Tracking sessions: remembered elements and ID assignment.

use std::collections::HashMap;

use argus_protocol::{Application, ElementId, Observation, ObservationId, Score};

use crate::matching::{Known, assign};
use crate::profile::profiles;

/// How many observations an element that disappeared is remembered, so that
/// an element one source missed for a moment keeps its ID.
pub const MEMORY: u32 = 3;

/// Identity confidence below which a kept ID counts as uncertain in a
/// [`TrackingReport`].
pub const UNCERTAIN: f32 = 0.75;

/// Assigns element IDs that persist across the observations of a session.
///
/// A session follows one application. Observing another application starts
/// a new session; IDs are never reused within the lifetime of a tracker, so
/// an ID never denotes two different elements.
#[derive(Debug, Default)]
pub struct Tracker {
    session: Option<Session>,
    /// Number of element IDs issued so far.
    issued: u64,
}

#[derive(Debug)]
struct Session {
    application: Option<Application>,
    previous: ObservationId,
    known: Vec<Known>,
}

/// What tracking did with one observation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrackingReport {
    /// The observation continued an earlier one.
    pub continued: bool,
    /// Elements of the previous observation that kept their ID.
    pub kept: usize,
    /// Elements that reappeared after being missed, with their old ID.
    pub restored: usize,
    /// Elements seen for the first time, with a new ID.
    pub new: usize,
    /// Elements of the previous observation that were not found.
    pub lost: usize,
    /// Kept or restored IDs whose identity confidence is below
    /// [`UNCERTAIN`].
    pub uncertain: usize,
}

impl Tracker {
    /// Creates a tracker with no session.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ends the current session: the next observation starts a new one.
    pub fn reset(&mut self) {
        self.session = None;
    }

    /// Replaces the element IDs of `observation` with tracked ones and sets
    /// [`Observation::previous`] and the identity confidence of every
    /// element that was seen before.
    ///
    /// `native_ids[i]` is the platform identifier of `observation.elements[i]`,
    /// if any. The order of elements is not changed.
    pub fn track(
        &mut self,
        observation: &mut Observation,
        native_ids: &[Option<String>],
    ) -> TrackingReport {
        let continues = self.session.as_ref().is_some_and(|session| {
            same_application(&session.application, &observation.application)
        });
        if !continues {
            self.session = None;
        }
        let known = self.session.as_ref().map_or(&[][..], |session| &session.known[..]);

        let (current, parents) = profiles(observation, native_ids);
        let matches = assign(known, &current, &parents);

        let mut report = TrackingReport { continued: continues, ..TrackingReport::default() };
        let mut matched = vec![false; known.len()];
        let mut ids = Vec::with_capacity(current.len());
        for (element, found) in observation.elements.iter_mut().zip(&matches) {
            match found {
                Some(found) => {
                    matched[found.known] = true;
                    let known = &known[found.known];
                    if known.missed == 0 {
                        report.kept += 1;
                    } else {
                        report.restored += 1;
                    }
                    if found.identity < UNCERTAIN {
                        report.uncertain += 1;
                    }
                    ids.push(known.id.clone());
                    element.confidence.identity =
                        Some(Score::new(found.identity).expect("identity is in range"));
                }
                None => {
                    report.new += 1;
                    self.issued += 1;
                    ids.push(
                        ElementId::new(format!("e_{}", self.issued))
                            .expect("generated ids are non-empty"),
                    );
                    element.confidence.identity = None;
                }
            }
        }
        report.lost = known
            .iter()
            .zip(&matched)
            .filter(|(known, matched)| known.missed == 0 && !**matched)
            .count();

        rename(observation, &ids);
        observation.previous = self.session.as_ref().map(|session| session.previous.clone());

        let mut remembered: Vec<Known> = current
            .into_iter()
            .enumerate()
            .map(|(index, profile)| Known {
                id: ids[index].clone(),
                parent: parents[index].map(|parent| ids[parent].clone()),
                profile,
                missed: 0,
            })
            .collect();
        remembered.extend(
            known
                .iter()
                .zip(&matched)
                .filter(|(known, matched)| !**matched && known.missed < MEMORY)
                .map(|(known, _)| Known { missed: known.missed + 1, ..known.clone() }),
        );
        self.session = Some(Session {
            application: observation.application.clone(),
            previous: observation.id.clone(),
            known: remembered,
        });
        report
    }
}

/// Gives `observation.elements[i]` the ID `ids[i]`, updating every
/// reference.
fn rename(observation: &mut Observation, ids: &[ElementId]) {
    let renamed: HashMap<ElementId, ElementId> = observation
        .elements
        .iter()
        .zip(ids)
        .map(|(element, id)| (element.id.clone(), id.clone()))
        .collect();
    let map = |id: &mut ElementId| {
        if let Some(new) = renamed.get(id) {
            *id = new.clone();
        }
    };
    for element in &mut observation.elements {
        map(&mut element.id);
        if let Some(parent) = &mut element.parent {
            map(parent);
        }
        element.children.iter_mut().for_each(map);
    }
    for relation in &mut observation.relations {
        map(&mut relation.from);
        map(&mut relation.to);
    }
}

/// Whether two observations are of the same application, as far as they
/// tell.
fn same_application(a: &Option<Application>, b: &Option<Application>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            let same = |x: &Option<_>, y: &Option<_>| match (x, y) {
                (Some(x), Some(y)) => Some(x == y),
                _ => None,
            };
            let pid = match (a.pid, b.pid) {
                (Some(x), Some(y)) => Some(x == y),
                _ => None,
            };
            pid.or_else(|| same(&a.bundle_id, &b.bundle_id))
                .or_else(|| same(&a.name, &b.name))
                .unwrap_or(true)
        }
        _ => false,
    }
}
