use serde::{Deserialize, Serialize};

use crate::Error;

/// An axis-aligned rectangle in global screen coordinates.
///
/// Units are logical points (not physical pixels). The origin is the top-left
/// corner of the primary display, `x` grows to the right and `y` grows
/// downward. Coordinates on other displays may be negative. See
/// `spec/ARGUS_PROTOCOL.md`, "Coordinate system".
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawBounds")]
pub struct Bounds {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl Bounds {
    /// Creates bounds, rejecting non-finite values and negative sizes.
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> crate::Result<Self> {
        let finite = [x, y, width, height].iter().all(|v| v.is_finite());
        if finite && width >= 0.0 && height >= 0.0 {
            Ok(Self { x, y, width, height })
        } else {
            Err(Error::InvalidBounds { x, y, width, height })
        }
    }

    /// Left edge.
    pub fn x(&self) -> f32 {
        self.x
    }

    /// Top edge.
    pub fn y(&self) -> f32 {
        self.y
    }

    /// Width.
    pub fn width(&self) -> f32 {
        self.width
    }

    /// Height.
    pub fn height(&self) -> f32 {
        self.height
    }

    /// The overlapping area of two rectangles, or `None` if they do not
    /// overlap with a positive area.
    pub fn intersection(&self, other: &Bounds) -> Option<Bounds> {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = (self.x + self.width).min(other.x + other.width);
        let bottom = (self.y + self.height).min(other.y + other.height);
        (right > left && bottom > top).then_some(Bounds {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        })
    }
}

#[derive(Deserialize)]
struct RawBounds {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl TryFrom<RawBounds> for Bounds {
    type Error = Error;

    fn try_from(raw: RawBounds) -> crate::Result<Self> {
        Self::new(raw.x, raw.y, raw.width, raw.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_negative_origin_and_empty_size() {
        let bounds = Bounds::new(-1440.0, -200.0, 0.0, 0.0).unwrap();
        assert_eq!((bounds.x(), bounds.y()), (-1440.0, -200.0));
    }

    #[test]
    fn rejects_negative_size_and_non_finite() {
        assert!(Bounds::new(0.0, 0.0, -1.0, 10.0).is_err());
        assert!(Bounds::new(0.0, 0.0, 10.0, -1.0).is_err());
        assert!(Bounds::new(f32::NAN, 0.0, 10.0, 10.0).is_err());
        assert!(Bounds::new(0.0, f32::INFINITY, 10.0, 10.0).is_err());
    }

    #[test]
    fn intersects_overlapping_rectangles() {
        let a = Bounds::new(0.0, 0.0, 100.0, 50.0).unwrap();
        let b = Bounds::new(80.0, -10.0, 40.0, 30.0).unwrap();
        assert_eq!(a.intersection(&b), Some(Bounds::new(80.0, 0.0, 20.0, 20.0).unwrap()));
        assert_eq!(a.intersection(&a), Some(a));
    }

    #[test]
    fn touching_or_empty_rectangles_do_not_intersect() {
        let a = Bounds::new(0.0, 0.0, 100.0, 50.0).unwrap();
        assert_eq!(a.intersection(&Bounds::new(100.0, 0.0, 10.0, 10.0).unwrap()), None);
        assert_eq!(a.intersection(&Bounds::new(10.0, 10.0, 0.0, 0.0).unwrap()), None);
        assert_eq!(a.intersection(&Bounds::new(-50.0, 60.0, 10.0, 10.0).unwrap()), None);
    }

    #[test]
    fn deserialization_enforces_invariants() {
        let json = r#"{"x": 0, "y": 0, "width": -5, "height": 10}"#;
        assert!(serde_json::from_str::<Bounds>(json).is_err());
    }
}
