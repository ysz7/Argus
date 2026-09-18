use serde::{Deserialize, Serialize};

use crate::Error;

/// A confidence value in `0.0..=1.0`.
///
/// `1.0` means Argus is certain; `0.0` means the claim is almost certainly
/// wrong. A score is never silently clamped: weak evidence stays weak.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct Score(f32);

impl Score {
    /// Full certainty.
    pub const CERTAIN: Score = Score(1.0);

    /// Creates a score, rejecting values outside `0.0..=1.0` and non-finite
    /// values.
    pub fn new(value: f32) -> crate::Result<Self> {
        if value.is_finite() && (0.0..=1.0).contains(&value) {
            Ok(Self(value))
        } else {
            Err(Error::InvalidScore(value))
        }
    }

    /// The score as a number.
    pub fn get(self) -> f32 {
        self.0
    }
}

impl TryFrom<f32> for Score {
    type Error = Error;

    fn try_from(value: f32) -> crate::Result<Self> {
        Self::new(value)
    }
}

impl From<Score> for f32 {
    fn from(score: Score) -> f32 {
        score.0
    }
}

/// Property-level confidence of an [`Element`](crate::Element).
///
/// `element` is the confidence that the element exists at all. Every other
/// field is the confidence of the corresponding property; `None` means the
/// property was not assessed (for example because it is absent).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Confidence {
    /// Confidence that the element exists.
    pub element: Score,
    /// Confidence of [`Element::role`](crate::Element::role).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Score>,
    /// Confidence of [`Element::name`](crate::Element::name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<Score>,
    /// Confidence of [`Element::value`](crate::Element::value).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Score>,
    /// Confidence of [`Element::bounds`](crate::Element::bounds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<Score>,
    /// Confidence of [`Element::state`](crate::Element::state).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<Score>,
}

impl Confidence {
    /// Confidence with only element existence assessed.
    pub fn new(element: Score) -> Self {
        Self { element, role: None, name: None, value: None, bounds: None, state: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_range_bounds() {
        assert_eq!(Score::new(0.0).unwrap().get(), 0.0);
        assert_eq!(Score::new(1.0).unwrap(), Score::CERTAIN);
        assert_eq!(Score::new(0.48).unwrap().get(), 0.48);
    }

    #[test]
    fn rejects_out_of_range_and_non_finite() {
        for value in [-0.01, 1.01, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(Score::new(value).is_err(), "{value} must be rejected");
        }
        assert!(serde_json::from_str::<Score>("1.5").is_err());
        assert!(serde_json::from_str::<Score>("-1").is_err());
    }

    #[test]
    fn deserializes_integers() {
        assert_eq!(serde_json::from_str::<Score>("1").unwrap(), Score::CERTAIN);
    }
}
