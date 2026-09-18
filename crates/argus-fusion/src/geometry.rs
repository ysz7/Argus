//! Box overlap measures used for matching.

use argus_protocol::Bounds;

pub(crate) fn area(bounds: &Bounds) -> f32 {
    bounds.width() * bounds.height()
}

fn overlap(a: &Bounds, b: &Bounds) -> f32 {
    a.intersection(b).map_or(0.0, |common| area(&common))
}

/// Intersection over union.
pub(crate) fn iou(a: &Bounds, b: &Bounds) -> f32 {
    let common = overlap(a, b);
    let union = area(a) + area(b) - common;
    if union > 0.0 { common / union } else { 0.0 }
}

/// Fraction of `inner` that lies inside `outer`. A degenerate `inner` (zero
/// area) counts as inside when its center is.
pub(crate) fn cover(inner: &Bounds, outer: &Bounds) -> f32 {
    let size = area(inner);
    if size > 0.0 {
        return overlap(inner, outer) / size;
    }
    let (x, y) = center(inner);
    let inside = (outer.x()..=outer.x() + outer.width()).contains(&x)
        && (outer.y()..=outer.y() + outer.height()).contains(&y);
    if inside { 1.0 } else { 0.0 }
}

pub(crate) fn center(bounds: &Bounds) -> (f32, f32) {
    (bounds.x() + bounds.width() / 2.0, bounds.y() + bounds.height() / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(x: f32, y: f32, width: f32, height: f32) -> Bounds {
        Bounds::new(x, y, width, height).unwrap()
    }

    #[test]
    fn measures_overlap() {
        assert_eq!(iou(&b(0.0, 0.0, 10.0, 10.0), &b(0.0, 0.0, 10.0, 10.0)), 1.0);
        assert_eq!(iou(&b(0.0, 0.0, 10.0, 10.0), &b(5.0, 0.0, 10.0, 10.0)), 50.0 / 150.0);
        assert_eq!(iou(&b(0.0, 0.0, 10.0, 10.0), &b(20.0, 0.0, 10.0, 10.0)), 0.0);
        assert_eq!(cover(&b(2.0, 2.0, 4.0, 4.0), &b(0.0, 0.0, 10.0, 10.0)), 1.0);
        assert_eq!(cover(&b(8.0, 0.0, 4.0, 10.0), &b(0.0, 0.0, 10.0, 10.0)), 0.5);
        assert_eq!(cover(&b(5.0, 5.0, 0.0, 0.0), &b(0.0, 0.0, 10.0, 10.0)), 1.0);
    }
}
