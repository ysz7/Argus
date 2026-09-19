//! Changes between successive observations.

use argus_protocol::{Observation, ObservationDelta};

/// The changes from `from` to `to`, or `None` if `to` does not directly
/// continue the tracking session of `from` (its element IDs would mean
/// nothing relative to `from`).
pub fn delta(from: &Observation, to: &Observation) -> Option<ObservationDelta> {
    (to.previous.as_ref() == Some(&from.id)).then(|| ObservationDelta::between(from, to))
}

#[cfg(test)]
mod tests {
    use argus_protocol::{ObservationId, Timestamp};

    use super::*;

    #[test]
    fn only_continued_observations_have_a_delta() {
        let first = Observation::new(ObservationId::new("obs_1").unwrap(), Timestamp(1));
        let mut second = Observation::new(ObservationId::new("obs_2").unwrap(), Timestamp(2));
        assert!(delta(&first, &second).is_none(), "a new session");
        second.previous = Some(first.id.clone());
        let changes = delta(&first, &second).unwrap();
        assert!(changes.is_empty());
        assert_eq!(changes.timestamp, Some(Timestamp(2)));
    }
}
