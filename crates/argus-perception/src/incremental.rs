//! Partial perception: re-running OCR and visual detection only where a frame
//! changed, and reusing the earlier results everywhere else.
//!
//! A changed region is grown until no earlier result is cut by it: every
//! text line or control it touches is perceived again as a whole. Only a
//! container (a group) that encloses a region with margin is kept, because
//! its outline did not change. The grown regions, with a margin of context,
//! are cropped from the frame and perceived on their own; of their results,
//! those touching the grown region are new, the rest (in the margin) are
//! already known. Results elsewhere are reused as they are.

use argus_protocol::{Frame, PixelRect, Role};

use crate::{FrameChange, OcrBackend, OcrRegion, Result, VisualCandidate, VisualPerceptionBackend};

/// Context kept around a changed region for text, in points. Text
/// recognition misses single glyphs in small images; context helps.
const TEXT_MARGIN: f32 = 24.0;
/// Context kept around a changed region for visual detection, in points
/// (outlines need their surroundings to be found).
const VISION_MARGIN: f32 = 8.0;
/// Above this share of the frame, the regions are perceived as one full
/// frame.
const FULL_SHARE: f32 = 0.5;

/// Results after a partial update.
#[derive(Debug, Clone, PartialEq)]
pub struct Update<T> {
    /// All results for the new frame.
    pub items: Vec<T>,
    /// How many earlier results were reused.
    pub reused: usize,
    /// The regions perceived again, in frame pixels.
    pub regions: Vec<PixelRect>,
}

impl<T> Update<T> {
    /// The share of the frame perceived again, `0..=1`.
    pub fn recomputed_share(&self, frame: &Frame) -> f32 {
        let area: f32 = self.regions.iter().map(|r| r.width * r.height).sum();
        area / (frame.width() as f32 * frame.height() as f32)
    }
}

/// Updates the text lines of the previous frame for `frame`.
pub fn update_ocr(
    backend: &dyn OcrBackend,
    frame: &Frame,
    previous: &[OcrRegion],
    change: &FrameChange,
) -> Result<Update<OcrRegion>> {
    update(
        frame,
        previous,
        change,
        TEXT_MARGIN,
        |region| (region.rect, false),
        |crop| backend.detect(crop),
        |region, (dx, dy)| {
            region.rect.x += dx;
            region.rect.y += dy;
        },
    )
}

/// Updates the detections of the previous frame for `frame`.
pub fn update_detections(
    backend: &dyn VisualPerceptionBackend,
    frame: &Frame,
    previous: &[VisualCandidate],
    change: &FrameChange,
) -> Result<Update<VisualCandidate>> {
    update(
        frame,
        previous,
        change,
        VISION_MARGIN,
        |detection| (detection.rect, is_container(detection.role)),
        |crop| backend.detect(crop),
        |detection, (dx, dy)| {
            detection.rect.x += dx;
            detection.rect.y += dy;
        },
    )
}

fn is_container(role: Role) -> bool {
    matches!(role, Role::Group | Role::Window | Role::List | Role::Table)
}

/// The generic update. `describe` gives an item's rectangle and whether it is
/// a container; `detect` perceives a crop; `shift` moves an item from crop to
/// frame pixels.
fn update<T: Clone>(
    frame: &Frame,
    previous: &[T],
    change: &FrameChange,
    margin: f32,
    describe: impl Fn(&T) -> (PixelRect, bool),
    mut detect: impl FnMut(&Frame) -> Result<Vec<T>>,
    shift: impl Fn(&mut T, (f32, f32)),
) -> Result<Update<T>> {
    let scale = frame.scale_factor();
    let margin = margin * scale;
    let whole =
        PixelRect { x: 0.0, y: 0.0, width: frame.width() as f32, height: frame.height() as f32 };
    // Cores: the changed pixels and every earlier result they cut. Margins
    // only give the crops context; they invalidate nothing, or neighbours
    // closer than the margin would be pulled in one after another.
    let mut cores: Vec<PixelRect> = change.regions.clone();
    let mut invalid = vec![false; previous.len()];
    loop {
        let mut grown = false;
        for (index, item) in previous.iter().enumerate() {
            if invalid[index] {
                continue;
            }
            let (rect, container) = describe(item);
            for core in &mut cores {
                if !intersects(&rect, core) {
                    continue;
                }
                if container && contains(&rect, &expand(core, margin)) {
                    continue; // its outline is untouched
                }
                invalid[index] = true;
                tracing::trace!(?rect, container, "earlier result cut by a changed region");
                *core = union(core, &rect);
                grown = true;
            }
        }
        cores = merge(cores);
        if !grown {
            break;
        }
    }
    let crops: Vec<PixelRect> = merge(cores.iter().map(|core| expand(core, margin)).collect())
        .iter()
        .filter_map(|crop| clip(&align(crop, scale), &whole))
        .collect();

    let area: f32 = crops.iter().map(|r| r.width * r.height).sum();
    tracing::debug!(changed = ?change.regions, perceived = ?crops, "regions to perceive again");
    if area > FULL_SHARE * whole.width * whole.height {
        return Ok(Update { items: detect(frame)?, reused: 0, regions: vec![whole] });
    }

    let mut items: Vec<T> = previous
        .iter()
        .zip(&invalid)
        .filter(|(_, invalid)| !**invalid)
        .map(|(item, _)| item.clone())
        .collect();
    let reused = items.len();
    for crop in &crops {
        let image =
            frame.crop(crop.x as u32, crop.y as u32, crop.width as u32, crop.height as u32)?;
        for mut item in detect(&image)? {
            shift(&mut item, (crop.x, crop.y));
            // Results in the margin only are already known.
            if cores.iter().any(|core| intersects(&describe(&item).0, core)) {
                items.push(item);
            }
        }
    }
    Ok(Update { items, reused, regions: crops })
}

fn expand(rect: &PixelRect, margin: f32) -> PixelRect {
    PixelRect {
        x: rect.x - margin,
        y: rect.y - margin,
        width: rect.width + 2.0 * margin,
        height: rect.height + 2.0 * margin,
    }
}

fn intersects(a: &PixelRect, b: &PixelRect) -> bool {
    a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}

fn contains(outer: &PixelRect, inner: &PixelRect) -> bool {
    outer.x <= inner.x
        && outer.y <= inner.y
        && inner.x + inner.width <= outer.x + outer.width
        && inner.y + inner.height <= outer.y + outer.height
}

fn union(a: &PixelRect, b: &PixelRect) -> PixelRect {
    let (x, y) = (a.x.min(b.x), a.y.min(b.y));
    PixelRect {
        x,
        y,
        width: (a.x + a.width).max(b.x + b.width) - x,
        height: (a.y + a.height).max(b.y + b.height) - y,
    }
}

/// Merges overlapping rectangles until all are disjoint.
fn merge(mut rects: Vec<PixelRect>) -> Vec<PixelRect> {
    let mut merged = true;
    while merged {
        merged = false;
        'outer: for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                if intersects(&rects[i], &rects[j]) {
                    let other = rects.swap_remove(j);
                    rects[i] = union(&rects[i], &other);
                    merged = true;
                    break 'outer;
                }
            }
        }
    }
    rects
}

/// Grows a rectangle outward to whole points, so that a crop's pixel grid
/// lines up with the frame's point grid (visual detection works in points).
fn align(rect: &PixelRect, scale: f32) -> PixelRect {
    let x = (rect.x / scale).floor() * scale;
    let y = (rect.y / scale).floor() * scale;
    PixelRect {
        x,
        y,
        width: ((rect.x + rect.width) / scale).ceil() * scale - x,
        height: ((rect.y + rect.height) / scale).ceil() * scale - y,
    }
}

fn clip(rect: &PixelRect, whole: &PixelRect) -> Option<PixelRect> {
    let x = rect.x.max(whole.x);
    let y = rect.y.max(whole.y);
    let right = (rect.x + rect.width).min(whole.x + whole.width);
    let bottom = (rect.y + rect.height).min(whole.y + whole.height);
    (right - x >= 1.0 && bottom - y >= 1.0).then_some(PixelRect {
        x,
        y,
        width: right - x,
        height: bottom - y,
    })
}
