//! Reading order of elements that have no structural order.

use argus_protocol::Bounds;

use crate::geometry::center;

/// Sorts items top-to-bottom into lines, and each line left-to-right.
pub(crate) fn reading_order<T>(items: &mut Vec<T>, bounds: impl Fn(&T) -> Bounds) {
    *items = lines(std::mem::take(items), bounds).into_iter().flatten().collect();
}

/// Groups items into lines, top to bottom, each sorted left to right. An item
/// belongs to the current line when its vertical center lies above the
/// bottom of the line's first item.
pub(crate) fn lines<T>(mut items: Vec<T>, bounds: impl Fn(&T) -> Bounds) -> Vec<Vec<T>> {
    items.sort_by(|a, b| {
        let (a, b) = (bounds(a), bounds(b));
        a.y().total_cmp(&b.y()).then(a.x().total_cmp(&b.x()))
    });

    let mut lines: Vec<Vec<T>> = Vec::new();
    let mut line_bottom = f32::NEG_INFINITY;
    for item in items {
        let item_bounds = bounds(&item);
        if center(&item_bounds).1 > line_bottom || lines.is_empty() {
            lines.push(Vec::new());
            line_bottom = item_bounds.y() + item_bounds.height();
        }
        lines.last_mut().expect("a line was just pushed").push(item);
    }
    for line in &mut lines {
        line.sort_by(|a, b| bounds(a).x().total_cmp(&bounds(b).x()));
    }
    lines
}

/// Whether all items lie on a single line of text.
pub(crate) fn single_line(items: &[Bounds]) -> bool {
    let Some(first) = items.iter().min_by(|a, b| a.y().total_cmp(&b.y())) else {
        return true;
    };
    let bottom = first.y() + first.height();
    items.iter().all(|item| center(item).1 <= bottom)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(x: f32, y: f32) -> Bounds {
        Bounds::new(x, y, 20.0, 10.0).unwrap()
    }

    #[test]
    fn orders_lines_then_columns() {
        // Second item sits 2 pt higher but on the same line.
        let mut items = vec![b(50.0, 2.0), b(10.0, 4.0), b(10.0, 30.0), b(80.0, 0.0)];
        reading_order(&mut items, |item| *item);
        assert_eq!(items, [b(10.0, 4.0), b(50.0, 2.0), b(80.0, 0.0), b(10.0, 30.0)]);
        assert!(single_line(&items[..3]));
        assert!(!single_line(&items));
    }
}
