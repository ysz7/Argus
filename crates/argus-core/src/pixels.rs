//! Pixel perception of successive frames: full, or incremental where only
//! part of the window changed.
//!
//! ```text
//! frame N + frame N+1 → change detection → dirty regions
//!     → partial OCR / visual detection → reconciliation with frame N's results
//! ```
//!
//! The raw results of the last frame (text lines and detections, in frame
//! pixels) are remembered per window. An unchanged frame reuses them all;
//! a frame with small changes re-perceives only the changed regions; a large
//! change, a resized window or another window is perceived in full.

use argus_perception::{
    FrameChange, OcrBackend, OcrRegion, VisualCandidate, VisualPerceptionBackend, frame_changes,
    ocr_candidates, update_detections, update_ocr, vision_candidates,
};
use argus_protocol::{Frame, PixelRect, SourceCandidate};

use crate::Result;

/// Above this share of the frame covered by changed regions, the frame is
/// perceived in full.
const FULL_CHANGE: f32 = 0.5;
/// Smallest overlap (intersection over union) at which a partial result
/// matches a full one during verification.
const VERIFY_OVERLAP: f32 = 0.7;

/// How the pixels of an observation were perceived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerceptionMode {
    /// Everything was perceived.
    Full,
    /// Nothing changed since the previous frame of the window; its results
    /// were reused.
    Unchanged,
    /// Only the changed regions were perceived again.
    Partial,
}

/// What pixel perception did for one observation.
#[derive(Debug, Clone, PartialEq)]
pub struct PerceptionReport {
    /// How the frame was perceived.
    pub mode: PerceptionMode,
    /// Share of pixels that changed since the previous frame, if compared.
    pub changed_ratio: Option<f32>,
    /// Changed regions found.
    pub dirty_regions: usize,
    /// Text lines and detections reused from the previous frame.
    pub reused: usize,
    /// Text lines and detections perceived for this frame.
    pub perceived: usize,
    /// Share of the frame perceived again (1 for a full perception), the
    /// largest over OCR and visual detection.
    pub perceived_share: f32,
    /// Differences from a full perception of the same frame, when verified.
    pub verification: Option<Verification>,
}

/// Differences between incremental and full perception of one frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Verification {
    /// Text lines full perception found and incremental perception did not.
    pub ocr_missing: usize,
    /// Text lines only incremental perception found.
    pub ocr_extra: usize,
    /// Detections full perception found and incremental perception did not.
    pub vision_missing: usize,
    /// Detections only incremental perception found.
    pub vision_extra: usize,
}

impl Verification {
    /// Whether incremental and full perception agree.
    pub fn agrees(&self) -> bool {
        *self == Self::default()
    }
}

/// Settings of pixel perception.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Settings {
    /// Re-perceive only what changed.
    pub(crate) incremental: bool,
    /// After an incremental perception, perceive the frame in full as well
    /// and report the differences (slow; for development).
    pub(crate) verify: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { incremental: true, verify: false }
    }
}

/// The raw results of the last frame.
#[derive(Debug, Default)]
pub(crate) struct PixelMemory {
    last: Option<Remembered>,
}

#[derive(Debug)]
struct Remembered {
    window: u32,
    frame: Frame,
    ocr: Option<Vec<OcrRegion>>,
    vision: Option<Vec<VisualCandidate>>,
}

impl PixelMemory {
    /// Forgets the last frame.
    pub(crate) fn clear(&mut self) {
        self.last = None;
    }
}

/// Candidates perceived in one frame, per requested source.
#[derive(Debug, Default)]
pub(crate) struct Perceived {
    pub(crate) ocr: Option<Vec<SourceCandidate>>,
    pub(crate) vision: Option<Vec<SourceCandidate>>,
    pub(crate) report: Option<PerceptionReport>,
    /// Milliseconds spent in OCR and in visual detection.
    pub(crate) ocr_ms: u64,
    pub(crate) vision_ms: u64,
}

/// Perceives `frame` of `window` with the given backends (`None`: source not
/// requested), incrementally when the previous frame of the window allows.
pub(crate) fn perceive(
    memory: &mut PixelMemory,
    settings: Settings,
    window: u32,
    frame: Frame,
    ocr: Option<&dyn OcrBackend>,
    vision: Option<&dyn VisualPerceptionBackend>,
) -> Result<Perceived> {
    let previous = memory.last.take().filter(|last| settings.incremental && last.window == window);
    let change = previous.as_ref().and_then(|last| frame_changes(&last.frame, &frame));
    let usable = change.as_ref().filter(|change| change.region_ratio() <= FULL_CHANGE);
    let mode = match usable {
        None => PerceptionMode::Full,
        Some(change) if change.is_empty() => PerceptionMode::Unchanged,
        Some(_) => PerceptionMode::Partial,
    };
    let mut report = PerceptionReport {
        mode,
        changed_ratio: change.as_ref().map(FrameChange::changed_ratio),
        dirty_regions: change.as_ref().map_or(0, |change| change.regions.len()),
        reused: 0,
        perceived: 0,
        perceived_share: if mode == PerceptionMode::Full { 1.0 } else { 0.0 },
        verification: None,
    };
    let mut verification = Verification::default();

    let mut perceived = Perceived::default();
    let ocr_regions = match ocr {
        None => None,
        Some(backend) => {
            let started = std::time::Instant::now();
            let earlier = previous.as_ref().and_then(|last| last.ocr.as_deref());
            let regions = step(
                &frame,
                usable,
                earlier,
                &mut report,
                |previous, change| update_ocr(backend, &frame, previous, change),
                || backend.detect(&frame),
            )?;
            perceived.ocr_ms = started.elapsed().as_millis() as u64;
            if settings.verify && mode != PerceptionMode::Full {
                let full = backend.detect(&frame)?;
                let same =
                    |a: &OcrRegion, b: &OcrRegion| a.text == b.text && close(&a.rect, &b.rect);
                verification.ocr_missing = missing(&full, &regions, same);
                verification.ocr_extra = missing(&regions, &full, same);
                for region in full.iter().filter(|a| !regions.iter().any(|b| same(a, b))) {
                    tracing::debug!(text = region.text, rect = ?region.rect, "only full OCR found");
                }
                for region in regions.iter().filter(|a| !full.iter().any(|b| same(a, b))) {
                    tracing::debug!(text = region.text, rect = ?region.rect, "only incremental OCR found");
                }
            }
            perceived.ocr = Some(ocr_candidates(&frame, &regions));
            Some(regions)
        }
    };
    let detections = match vision {
        None => None,
        Some(backend) => {
            let started = std::time::Instant::now();
            let earlier = previous.as_ref().and_then(|last| last.vision.as_deref());
            let detections = step(
                &frame,
                usable,
                earlier,
                &mut report,
                |previous, change| update_detections(backend, &frame, previous, change),
                || backend.detect(&frame),
            )?;
            perceived.vision_ms = started.elapsed().as_millis() as u64;
            if settings.verify && mode != PerceptionMode::Full {
                let full = backend.detect(&frame)?;
                let same = |a: &VisualCandidate, b: &VisualCandidate| {
                    a.role == b.role && a.state == b.state && close(&a.rect, &b.rect)
                };
                verification.vision_missing = missing(&full, &detections, same);
                verification.vision_extra = missing(&detections, &full, same);
            }
            perceived.vision = Some(vision_candidates(&frame, &detections));
            Some(detections)
        }
    };
    if settings.verify && mode != PerceptionMode::Full {
        report.verification = Some(verification);
    }
    if report.perceived_share >= 1.0 {
        report.mode = PerceptionMode::Full; // a source had nothing to reuse
    }
    perceived.report = Some(report);
    memory.last = Some(Remembered { window, frame, ocr: ocr_regions, vision: detections });
    Ok(perceived)
}

/// One source: reuse, update or perceive in full.
fn step<T: Clone>(
    frame: &Frame,
    change: Option<&FrameChange>,
    previous: Option<&[T]>,
    report: &mut PerceptionReport,
    update: impl FnOnce(&[T], &FrameChange) -> argus_perception::Result<argus_perception::Update<T>>,
    full: impl FnOnce() -> argus_perception::Result<Vec<T>>,
) -> Result<Vec<T>> {
    match (change, previous) {
        (Some(change), Some(previous)) if change.is_empty() => {
            report.reused += previous.len();
            Ok(previous.to_vec())
        }
        (Some(change), Some(previous)) => {
            let update = update(previous, change)?;
            report.reused += update.reused;
            report.perceived += update.items.len() - update.reused;
            report.perceived_share = report.perceived_share.max(update.recomputed_share(frame));
            Ok(update.items)
        }
        _ => {
            let items = full()?;
            report.perceived += items.len();
            report.perceived_share = 1.0;
            Ok(items)
        }
    }
}

fn close(a: &PixelRect, b: &PixelRect) -> bool {
    let width = (a.x + a.width).min(b.x + b.width) - a.x.max(b.x);
    let height = (a.y + a.height).min(b.y + b.height) - a.y.max(b.y);
    let common = width.max(0.0) * height.max(0.0);
    let union = a.width * a.height + b.width * b.height - common;
    union > 0.0 && common / union >= VERIFY_OVERLAP
}

/// How many items of `left` have no counterpart in `right`.
fn missing<T>(left: &[T], right: &[T], same: impl Fn(&T, &T) -> bool) -> usize {
    left.iter().filter(|a| !right.iter().any(|b| same(a, b))).count()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use argus_protocol::{Bounds, FrameId, PixelBuffer, Score, Timestamp};

    use super::*;

    /// Reads one "line" per non-black 4×4 block and remembers the sizes of
    /// the images it was given.
    #[derive(Default)]
    struct BlockOcr {
        calls: RefCell<Vec<(u32, u32)>>,
    }

    impl OcrBackend for BlockOcr {
        fn detect(&self, frame: &Frame) -> argus_perception::Result<Vec<OcrRegion>> {
            self.calls.borrow_mut().push((frame.width(), frame.height()));
            let mut lines = Vec::new();
            for y in (0..frame.height()).step_by(4) {
                for x in (0..frame.width()).step_by(4) {
                    let [red, ..] = frame.pixels().pixel(x, y).unwrap();
                    if red != 0 {
                        lines.push(OcrRegion {
                            text: format!("{red}"),
                            rect: PixelRect { x: x as f32, y: y as f32, width: 4.0, height: 4.0 },
                            confidence: Score::CERTAIN,
                        });
                    }
                }
            }
            Ok(lines)
        }
    }

    /// A 200×200-pixel frame at `x` with 4×4 blocks of the given red values.
    fn frame(x: f32, blocks: &[(u32, u32, u8)]) -> Frame {
        let mut data = vec![0; 200 * 200 * 4];
        for &(bx, by, red) in blocks {
            for y in by..by + 4 {
                for x in bx..bx + 4 {
                    data[((y * 200 + x) * 4) as usize] = red;
                }
            }
        }
        let bounds = Bounds::new(x, 0.0, 100.0, 100.0).unwrap();
        Frame::new(FrameId(1), Timestamp(0), bounds, 2.0, PixelBuffer::new(200, 200, data).unwrap())
            .unwrap()
    }

    fn texts(perceived: &Perceived) -> Vec<String> {
        let mut texts: Vec<String> =
            perceived.ocr.iter().flatten().filter_map(|candidate| candidate.name.clone()).collect();
        texts.sort();
        texts
    }

    #[test]
    fn only_what_changed_is_perceived_again() {
        let ocr = BlockOcr::default();
        let mut memory = PixelMemory::default();
        let settings = Settings { incremental: true, verify: true };
        let blocks = [(20, 20, 1), (160, 160, 2)];

        let first =
            perceive(&mut memory, settings, 7, frame(0.0, &blocks), Some(&ocr), None).unwrap();
        assert_eq!(first.report.as_ref().unwrap().mode, PerceptionMode::Full);
        assert_eq!(texts(&first), ["1", "2"]);

        // The same pixels in a moved window: nothing to perceive.
        let moved =
            perceive(&mut memory, settings, 7, frame(500.0, &blocks), Some(&ocr), None).unwrap();
        let report = moved.report.unwrap();
        assert_eq!(
            (report.mode, report.reused, report.perceived),
            (PerceptionMode::Unchanged, 2, 0)
        );
        assert_eq!(ocr.calls.borrow().len(), 1 + 1, "only the verification ran");

        // One block changes: only a crop around it is read.
        ocr.calls.borrow_mut().clear();
        let changed = [(20, 20, 1), (160, 160, 3)];
        let partial =
            perceive(&mut memory, settings, 7, frame(500.0, &changed), Some(&ocr), None).unwrap();
        assert_eq!(texts(&partial), ["1", "3"]);
        let report = partial.report.unwrap();
        assert_eq!((report.mode, report.reused, report.perceived), (PerceptionMode::Partial, 1, 1));
        assert!(report.verification.unwrap().agrees());
        let calls = ocr.calls.borrow();
        assert!(calls[0].0 < 100 && calls[0].1 < 100, "a crop: {calls:?}");
        // Grounded in the new window position.
        let line = partial.ocr.as_ref().unwrap().iter().find(|c| c.name.as_deref() == Some("3"));
        let argus_protocol::Region::Frame { geometry, rect } = line.unwrap().region else {
            panic!("pixel region")
        };
        assert_eq!(geometry.bounds().x(), 500.0);
        assert_eq!((rect.x, rect.y), (160.0, 160.0));
    }

    #[test]
    fn another_window_or_disabled_increments_are_full() {
        let ocr = BlockOcr::default();
        let mut memory = PixelMemory::default();
        let blocks = [(20, 20, 1)];
        let incremental = Settings::default();
        perceive(&mut memory, incremental, 7, frame(0.0, &blocks), Some(&ocr), None).unwrap();
        let other =
            perceive(&mut memory, incremental, 8, frame(0.0, &blocks), Some(&ocr), None).unwrap();
        assert_eq!(other.report.unwrap().mode, PerceptionMode::Full);

        let full = Settings { incremental: false, verify: false };
        let again = perceive(&mut memory, full, 8, frame(0.0, &blocks), Some(&ocr), None).unwrap();
        assert_eq!(again.report.unwrap().mode, PerceptionMode::Full);
    }
}
