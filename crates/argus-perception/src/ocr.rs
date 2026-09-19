//! Optical character recognition.

use argus_protocol::{
    CandidateId, Confidence, ElementState, Frame, PixelRect, Region, Role, Score, Source,
    SourceCandidate, SourceMeta,
};

use crate::Result;

/// A text recognizer working on frames.
pub trait OcrBackend {
    /// Finds lines of text in the frame.
    fn detect(&self, frame: &Frame) -> Result<Vec<OcrRegion>>;
}

/// One recognized line of text.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrRegion {
    /// The recognized text.
    pub text: String,
    /// Where the text is, in frame pixels.
    pub rect: PixelRect,
    /// Recognizer confidence that `text` is correct.
    pub confidence: Score,
}

/// Converts OCR regions of `frame` into source candidates.
///
/// Every region becomes a flat `text` candidate: OCR reports *that* text is
/// there and *where*, never what kind of control carries it.
pub fn ocr_candidates(frame: &Frame, regions: &[OcrRegion]) -> Vec<SourceCandidate> {
    let geometry = frame.geometry();
    regions
        .iter()
        .enumerate()
        .map(|(index, region)| SourceCandidate {
            id: CandidateId(index as u32),
            parent: None,
            role: Role::Text,
            name: Some(region.text.clone()),
            value: None,
            text: None,
            description: None,
            region: Region::Frame { geometry, rect: region.rect },
            clip: None,
            // Text recognized in the captured pixels is visible in them.
            state: ElementState { visible: Some(true), ..ElementState::default() },
            confidence: Confidence {
                name: Some(region.confidence),
                ..Confidence::new(region.confidence)
            },
            relations: Vec::new(),
            meta: SourceMeta::new(Source::Ocr),
        })
        .collect()
}

/// Converts a normalized rectangle with a bottom-left origin (as used by
/// Apple Vision) into frame pixels with a top-left origin.
pub(crate) fn pixel_rect_from_normalized(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    frame_width: u32,
    frame_height: u32,
) -> PixelRect {
    let (frame_width, frame_height) = (f64::from(frame_width), f64::from(frame_height));
    PixelRect {
        x: (x * frame_width) as f32,
        y: ((1.0 - y - height) * frame_height) as f32,
        width: (width * frame_width) as f32,
        height: (height * frame_height) as f32,
    }
}

#[cfg(test)]
mod tests {
    use argus_protocol::{Bounds, FrameId, PixelBuffer, Timestamp};

    use super::*;

    #[test]
    fn flips_normalized_rects_to_top_left_pixels() {
        // A box in the top-left quarter of a 200x100 frame.
        let rect = pixel_rect_from_normalized(0.0, 0.5, 0.5, 0.5, 200, 100);
        assert_eq!(rect, PixelRect { x: 0.0, y: 0.0, width: 100.0, height: 50.0 });

        // A box touching the bottom-right corner.
        let rect = pixel_rect_from_normalized(0.75, 0.0, 0.25, 0.1, 200, 100);
        assert_eq!(rect, PixelRect { x: 150.0, y: 90.0, width: 50.0, height: 10.0 });
    }

    #[test]
    fn regions_become_frame_grounded_text_candidates() {
        let bounds = Bounds::new(100.0, 50.0, 100.0, 50.0).unwrap();
        let pixels = PixelBuffer::new(200, 100, vec![0; 200 * 100 * 4]).unwrap();
        let frame = Frame::new(FrameId(7), Timestamp(0), bounds, 2.0, pixels).unwrap();
        let confidence = Score::new(0.5).unwrap();
        let region = OcrRegion {
            text: "Save".to_owned(),
            rect: PixelRect { x: 20.0, y: 10.0, width: 40.0, height: 16.0 },
            confidence,
        };

        let candidates = ocr_candidates(&frame, std::slice::from_ref(&region));
        let [candidate] = candidates.as_slice() else { panic!("{candidates:?}") };
        assert_eq!(candidate.role, Role::Text);
        assert_eq!(candidate.name.as_deref(), Some("Save"));
        assert_eq!(
            candidate.region,
            Region::Frame { geometry: frame.geometry(), rect: region.rect }
        );
        assert_eq!(candidate.confidence.element, confidence);
        assert_eq!(candidate.confidence.name, Some(confidence));
        assert_eq!(candidate.confidence.role, None, "OCR does not assess roles");
        assert_eq!(candidate.meta.source, Source::Ocr);
        assert_eq!(candidate.parent, None);
    }
}
