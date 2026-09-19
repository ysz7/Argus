//! Change detection between successive frames of one window.

use argus_protocol::{Frame, PixelBuffer, PixelRect};

/// Side of the square tiles frames are compared in, in pixels.
const TILE: u32 = 16;
/// Dirty tiles at most this many tiles apart are merged into one region.
const GAP: u32 = 1;

/// What changed between two frames of the same size.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameChange {
    /// Pixels whose color changed.
    pub changed_pixels: u64,
    /// Pixels in the frame.
    pub total_pixels: u64,
    /// Rectangles (in frame pixels, tile-aligned, disjoint) that contain
    /// every changed pixel.
    pub regions: Vec<PixelRect>,
}

impl FrameChange {
    /// The share of pixels that changed, `0..=1`.
    pub fn changed_ratio(&self) -> f32 {
        self.changed_pixels as f32 / self.total_pixels.max(1) as f32
    }

    /// The share of the frame covered by the changed regions, `0..=1`.
    pub fn region_ratio(&self) -> f32 {
        let area: f32 = self.regions.iter().map(|r| r.width * r.height).sum();
        area / self.total_pixels.max(1) as f32
    }

    /// Whether nothing changed.
    pub fn is_empty(&self) -> bool {
        self.changed_pixels == 0
    }
}

/// Compares two frames pixel by pixel. `None` if their pixel sizes differ
/// (a resized window: everything must be perceived again).
///
/// The frames' placement on screen is ignored: a moved window with the same
/// content has no changes.
pub fn frame_changes(previous: &Frame, current: &Frame) -> Option<FrameChange> {
    let (width, height) = (current.width(), current.height());
    if (previous.width(), previous.height()) != (width, height) {
        return None;
    }
    let (before, after) = (previous.pixels().as_bytes(), current.pixels().as_bytes());
    let columns = width.div_ceil(TILE);
    let rows = height.div_ceil(TILE);
    let mut dirty = vec![false; (columns * rows) as usize];
    let mut changed_pixels = 0u64;
    let row_bytes = width as usize * PixelBuffer::BYTES_PER_PIXEL;
    for y in 0..height {
        let line = y as usize * row_bytes;
        for column in 0..columns {
            let start = line + (column * TILE) as usize * PixelBuffer::BYTES_PER_PIXEL;
            let end =
                line + ((column + 1) * TILE).min(width) as usize * PixelBuffer::BYTES_PER_PIXEL;
            let (a, b) = (&before[start..end], &after[start..end]);
            if a != b {
                dirty[((y / TILE) * columns + column) as usize] = true;
                changed_pixels += a
                    .chunks_exact(PixelBuffer::BYTES_PER_PIXEL)
                    .zip(b.chunks_exact(PixelBuffer::BYTES_PER_PIXEL))
                    .filter(|(p, q)| p != q)
                    .count() as u64;
            }
        }
    }

    let regions = merge(components(&dirty, columns, rows))
        .into_iter()
        .map(|(left, top, right, bottom)| {
            let x = left * TILE;
            let y = top * TILE;
            PixelRect {
                x: x as f32,
                y: y as f32,
                width: ((right + 1) * TILE).min(width).saturating_sub(x) as f32,
                height: ((bottom + 1) * TILE).min(height).saturating_sub(y) as f32,
            }
        })
        .collect();
    Some(FrameChange {
        changed_pixels,
        total_pixels: u64::from(width) * u64::from(height),
        regions,
    })
}

/// Bounding boxes `(left, top, right, bottom)` (inclusive, in tiles) of the
/// 8-connected groups of dirty tiles.
fn components(dirty: &[bool], columns: u32, rows: u32) -> Vec<(u32, u32, u32, u32)> {
    let mut seen = vec![false; dirty.len()];
    let mut boxes = Vec::new();
    let mut stack = Vec::new();
    for start in 0..dirty.len() {
        if !dirty[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let (x, y) = ((start as u32) % columns, (start as u32) / columns);
        let mut bounds = (x, y, x, y);
        while let Some(index) = stack.pop() {
            let (x, y) = ((index as u32) % columns, (index as u32) / columns);
            bounds = (bounds.0.min(x), bounds.1.min(y), bounds.2.max(x), bounds.3.max(y));
            for ny in y.saturating_sub(1)..=(y + 1).min(rows - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(columns - 1) {
                    let next = (ny * columns + nx) as usize;
                    if dirty[next] && !seen[next] {
                        seen[next] = true;
                        stack.push(next);
                    }
                }
            }
        }
        boxes.push(bounds);
    }
    boxes
}

/// Merges boxes that overlap or lie within [`GAP`] tiles of each other,
/// until all are disjoint.
fn merge(mut boxes: Vec<(u32, u32, u32, u32)>) -> Vec<(u32, u32, u32, u32)> {
    let near = |a: &(u32, u32, u32, u32), b: &(u32, u32, u32, u32)| {
        a.0 <= b.2 + GAP + 1 && b.0 <= a.2 + GAP + 1 && a.1 <= b.3 + GAP + 1 && b.1 <= a.3 + GAP + 1
    };
    let mut merged = true;
    while merged {
        merged = false;
        'outer: for i in 0..boxes.len() {
            for j in i + 1..boxes.len() {
                if near(&boxes[i], &boxes[j]) {
                    let b = boxes.swap_remove(j);
                    let a = &mut boxes[i];
                    *a = (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3));
                    merged = true;
                    break 'outer;
                }
            }
        }
    }
    boxes.sort_by_key(|b| (b.1, b.0));
    boxes
}

#[cfg(test)]
mod tests {
    use argus_protocol::{Bounds, FrameId, Timestamp};

    use super::*;

    fn frame(x: f32, pixels: Vec<u8>) -> Frame {
        let bounds = Bounds::new(x, 0.0, 50.0, 30.0).unwrap();
        Frame::new(
            FrameId(1),
            Timestamp(0),
            bounds,
            2.0,
            PixelBuffer::new(100, 60, pixels).unwrap(),
        )
        .unwrap()
    }

    fn paint(pixels: &mut [u8], x0: usize, y0: usize, width: usize, height: usize) {
        for y in y0..y0 + height {
            for x in x0..x0 + width {
                pixels[(y * 100 + x) * 4..(y * 100 + x) * 4 + 4].copy_from_slice(&[255, 0, 0, 255]);
            }
        }
    }

    #[test]
    fn identical_frames_have_no_changes_even_when_moved() {
        let pixels = vec![7; 100 * 60 * 4];
        let change = frame_changes(&frame(0.0, pixels.clone()), &frame(300.0, pixels)).unwrap();
        assert!(change.is_empty());
        assert!(change.regions.is_empty());
        assert_eq!(change.total_pixels, 6000);
    }

    #[test]
    fn changed_pixels_are_covered_by_tile_aligned_regions() {
        let before = vec![0; 100 * 60 * 4];
        let mut after = before.clone();
        paint(&mut after, 3, 2, 4, 3); // 12 pixels in tile (0, 0)
        paint(&mut after, 90, 50, 5, 5); // 25 pixels in the far corner
        let change = frame_changes(&frame(0.0, before), &frame(0.0, after)).unwrap();
        assert_eq!(change.changed_pixels, 37);
        assert_eq!(
            change.regions,
            [
                PixelRect { x: 0.0, y: 0.0, width: 16.0, height: 16.0 },
                PixelRect { x: 80.0, y: 48.0, width: 16.0, height: 12.0 },
            ]
        );
        assert!((change.changed_ratio() - 37.0 / 6000.0).abs() < 1e-6);
    }

    #[test]
    fn nearby_changes_merge() {
        let before = vec![0; 100 * 60 * 4];
        let mut after = before.clone();
        paint(&mut after, 1, 1, 2, 2);
        paint(&mut after, 33, 1, 2, 2); // two tiles away: one tile gap
        let change = frame_changes(&frame(0.0, before), &frame(0.0, after)).unwrap();
        assert_eq!(change.regions, [PixelRect { x: 0.0, y: 0.0, width: 48.0, height: 16.0 }]);
    }

    #[test]
    fn resized_frames_are_not_compared() {
        let small = Frame::new(
            FrameId(1),
            Timestamp(0),
            Bounds::new(0.0, 0.0, 5.0, 5.0).unwrap(),
            2.0,
            PixelBuffer::new(10, 10, vec![0; 400]).unwrap(),
        )
        .unwrap();
        assert!(frame_changes(&small, &frame(0.0, vec![0; 100 * 60 * 4])).is_none());
    }
}
