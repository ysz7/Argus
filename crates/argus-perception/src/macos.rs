//! OCR with Apple Vision (`VNRecognizeTextRequest`).
//!
//! Recognition runs on-device; frames never leave the process.

use std::ptr;
use std::time::Instant;

use argus_protocol::{Frame, PixelBuffer, Score};
use objc2::AnyThread;
use objc2::rc::autoreleasepool;
use objc2_core_foundation::{CFData, CFRetained};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, kCGColorSpaceSRGB,
};
use objc2_foundation::{NSArray, NSDictionary};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
};

use crate::ocr::pixel_rect_from_normalized;
use crate::{Error, OcrBackend, OcrRegion, Result};

/// OCR backend based on Apple Vision.
#[derive(Debug, Default)]
pub struct VisionOcr {
    _private: (),
}

impl VisionOcr {
    /// Creates the backend (accurate recognition, automatic language
    /// detection).
    pub fn new() -> Self {
        Self::default()
    }
}

impl OcrBackend for VisionOcr {
    fn detect(&self, frame: &Frame) -> Result<Vec<OcrRegion>> {
        // Vision returns autoreleased objects; drain them per frame so a
        // long-running process does not accumulate them.
        autoreleasepool(|_| recognize(frame))
    }
}

fn recognize(frame: &Frame) -> Result<Vec<OcrRegion>> {
    let started = Instant::now();
    let image = cg_image(frame.pixels())?;

    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    request.setUsesLanguageCorrection(true);
    request.setAutomaticallyDetectsLanguage(true);

    // SAFETY: `image` is a valid image and the options dictionary is empty.
    let handler = unsafe {
        VNImageRequestHandler::initWithCGImage_options(
            VNImageRequestHandler::alloc(),
            &image,
            &NSDictionary::new(),
        )
    };
    let as_request: &VNRequest = &request;
    handler.performRequests_error(&NSArray::from_slice(&[as_request])).map_err(|error| {
        Error::Platform(format!("text recognition failed: {}", error.localizedDescription()))
    })?;

    let Some(observations) = request.results() else {
        return Ok(Vec::new());
    };
    let mut regions = Vec::with_capacity(observations.len());
    for observation in observations.iter() {
        let Some(best) = observation.topCandidates(1).firstObject() else {
            continue;
        };
        let Ok(confidence) = Score::new(best.confidence().clamp(0.0, 1.0)) else {
            continue; // NaN
        };
        // SAFETY: plain property read on a valid observation.
        let rect = unsafe { observation.boundingBox() };
        regions.push(OcrRegion {
            text: best.string().to_string(),
            rect: pixel_rect_from_normalized(
                rect.origin.x,
                rect.origin.y,
                rect.size.width,
                rect.size.height,
                frame.width(),
                frame.height(),
            ),
            confidence,
        });
    }
    tracing::debug!(
        lines = regions.len(),
        width = frame.width(),
        height = frame.height(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "recognized text"
    );
    Ok(regions)
}

/// Wraps the frame's RGBA pixels in a `CGImage` (the bytes are copied).
fn cg_image(pixels: &PixelBuffer) -> Result<CFRetained<CGImage>> {
    let width = pixels.width() as usize;
    let height = pixels.height() as usize;
    let data = CFData::from_bytes(pixels.as_bytes());
    let provider = CGDataProvider::with_cf_data(Some(&data))
        .ok_or_else(|| Error::Platform("cannot create image data provider".to_owned()))?;
    // SAFETY: `kCGColorSpaceSRGB` is an immutable constant.
    let color_space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
        .ok_or_else(|| Error::Platform("sRGB color space unavailable".to_owned()))?;
    let bitmap_info = CGBitmapInfo::from_bits_retain(
        CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0,
    );
    // SAFETY: the provider holds `width * height * 4` bytes laid out as
    // described (8-bit RGBA rows without padding); `decode` may be null.
    unsafe {
        CGImage::new(
            width,
            height,
            8,
            8 * PixelBuffer::BYTES_PER_PIXEL,
            width * PixelBuffer::BYTES_PER_PIXEL,
            Some(&color_space),
            bitmap_info,
            Some(&provider),
            ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
    .ok_or_else(|| Error::Platform("cannot create image".to_owned()))
}
