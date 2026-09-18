use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{Bounds, Error, Timestamp};

/// Identifier of a captured frame, unique within one Argus process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FrameId(pub u64);

impl fmt::Display for FrameId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "frame_{}", self.0)
    }
}

/// An image of part of the screen together with the metadata needed to ground
/// it in global screen coordinates.
///
/// A frame is the in-memory input of pixel-based perception. It is not part
/// of the JSON protocol and is never persisted unless explicitly requested
/// for debugging.
///
/// The frame covers [`Frame::bounds`] (global logical points). Pixel `(0, 0)`
/// is the top-left corner of those bounds, and one point spans
/// [`Frame::scale_factor`] pixels along each axis.
#[derive(Clone, PartialEq)]
pub struct Frame {
    id: FrameId,
    timestamp: Timestamp,
    bounds: Bounds,
    scale_factor: f32,
    pixels: PixelBuffer,
}

impl Frame {
    /// Maximum allowed difference, in pixels, between the pixel size and
    /// `bounds × scale_factor` (covers rounding of fractional point sizes).
    const SIZE_TOLERANCE: f32 = 1.0;

    /// Creates a frame, checking that the pixel dimensions match
    /// `bounds × scale_factor`.
    pub fn new(
        id: FrameId,
        timestamp: Timestamp,
        bounds: Bounds,
        scale_factor: f32,
        pixels: PixelBuffer,
    ) -> crate::Result<Self> {
        if !(scale_factor.is_finite() && scale_factor > 0.0) {
            return Err(Error::InvalidFrame(format!(
                "scale factor must be positive, got {scale_factor}"
            )));
        }
        let expected_width = bounds.width() * scale_factor;
        let expected_height = bounds.height() * scale_factor;
        if (pixels.width() as f32 - expected_width).abs() > Self::SIZE_TOLERANCE
            || (pixels.height() as f32 - expected_height).abs() > Self::SIZE_TOLERANCE
        {
            return Err(Error::InvalidFrame(format!(
                "pixel size {}x{} does not match bounds {}x{} at scale {scale_factor}",
                pixels.width(),
                pixels.height(),
                bounds.width(),
                bounds.height(),
            )));
        }
        Ok(Self { id, timestamp, bounds, scale_factor, pixels })
    }

    /// Frame identifier.
    pub fn id(&self) -> FrameId {
        self.id
    }

    /// When the pixels were captured.
    pub fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// Region of the global screen space covered by the frame, in points.
    pub fn bounds(&self) -> Bounds {
        self.bounds
    }

    /// Pixels per point.
    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.pixels.width()
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.pixels.height()
    }

    /// The pixel data.
    pub fn pixels(&self) -> &PixelBuffer {
        &self.pixels
    }

    /// Converts a position in frame pixels to global screen points.
    pub fn pixel_to_point(&self, x: f32, y: f32) -> (f32, f32) {
        (self.bounds.x() + x / self.scale_factor, self.bounds.y() + y / self.scale_factor)
    }

    /// Converts a rectangle in frame pixels to global screen [`Bounds`].
    pub fn pixel_rect_to_bounds(
        &self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> crate::Result<Bounds> {
        let (left, top) = self.pixel_to_point(x, y);
        Bounds::new(left, top, width / self.scale_factor, height / self.scale_factor)
    }
}

impl fmt::Debug for Frame {
    // Pixel data is deliberately left out: it is large and may contain
    // sensitive screen content.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("id", &self.id)
            .field("timestamp", &self.timestamp)
            .field("bounds", &self.bounds)
            .field("scale_factor", &self.scale_factor)
            .field("width", &self.width())
            .field("height", &self.height())
            .finish()
    }
}

/// An 8-bit RGBA image.
///
/// Rows are stored top to bottom without padding (`width × 4` bytes per row).
/// Colors are sRGB with premultiplied alpha.
#[derive(Clone, PartialEq, Eq)]
pub struct PixelBuffer {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl PixelBuffer {
    /// Bytes per pixel.
    pub const BYTES_PER_PIXEL: usize = 4;

    /// Wraps RGBA data, checking that its length matches the dimensions.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> crate::Result<Self> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(Self::BYTES_PER_PIXEL));
        if width == 0 || height == 0 || expected != Some(data.len()) {
            return Err(Error::InvalidFrame(format!(
                "{} bytes do not form a non-empty {width}x{height} RGBA image",
                data.len()
            )));
        }
        Ok(Self { width, height, data })
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Raw RGBA bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// The RGBA value of one pixel, or `None` outside the image.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = (y as usize * self.width as usize + x as usize) * Self::BYTES_PER_PIXEL;
        self.data[offset..offset + Self::BYTES_PER_PIXEL].try_into().ok()
    }
}

impl fmt::Debug for PixelBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PixelBuffer")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(width: u32, height: u32) -> PixelBuffer {
        PixelBuffer::new(width, height, vec![0; (width * height * 4) as usize]).unwrap()
    }

    /// A Retina frame of a window on a secondary display left of the primary.
    fn retina_frame() -> Frame {
        let bounds = Bounds::new(-1200.0, 100.0, 400.0, 300.0).unwrap();
        Frame::new(FrameId(1), Timestamp(0), bounds, 2.0, buffer(800, 600)).unwrap()
    }

    #[test]
    fn rejects_mismatched_buffers() {
        assert!(PixelBuffer::new(2, 2, vec![0; 15]).is_err());
        assert!(PixelBuffer::new(0, 2, Vec::new()).is_err());
        assert!(PixelBuffer::new(u32::MAX, u32::MAX, Vec::new()).is_err());
    }

    #[test]
    fn reads_pixels_row_major() {
        let mut data = vec![0; 2 * 2 * 4];
        data[12..16].copy_from_slice(&[1, 2, 3, 4]); // (1, 1)
        let pixels = PixelBuffer::new(2, 2, data).unwrap();
        assert_eq!(pixels.pixel(1, 1), Some([1, 2, 3, 4]));
        assert_eq!(pixels.pixel(0, 1), Some([0, 0, 0, 0]));
        assert_eq!(pixels.pixel(2, 0), None);
    }

    #[test]
    fn rejects_size_scale_mismatch() {
        let bounds = Bounds::new(0.0, 0.0, 400.0, 300.0).unwrap();
        // A 1x capture labelled as Retina would mis-ground every element.
        assert!(Frame::new(FrameId(1), Timestamp(0), bounds, 2.0, buffer(400, 300)).is_err());
        assert!(Frame::new(FrameId(1), Timestamp(0), bounds, 0.0, buffer(400, 300)).is_err());
        assert!(Frame::new(FrameId(1), Timestamp(0), bounds, 1.0, buffer(400, 300)).is_ok());
    }

    #[test]
    fn tolerates_rounding_of_fractional_point_sizes() {
        let bounds = Bounds::new(0.0, 0.0, 100.5, 50.25).unwrap();
        assert!(Frame::new(FrameId(1), Timestamp(0), bounds, 2.0, buffer(201, 101)).is_ok());
    }

    #[test]
    fn converts_pixels_to_global_points() {
        let frame = retina_frame();
        assert_eq!(frame.pixel_to_point(0.0, 0.0), (-1200.0, 100.0));
        assert_eq!(frame.pixel_to_point(800.0, 600.0), (-800.0, 400.0));

        let bounds = frame.pixel_rect_to_bounds(100.0, 50.0, 40.0, 20.0).unwrap();
        assert_eq!(
            (bounds.x(), bounds.y(), bounds.width(), bounds.height()),
            (-1150.0, 125.0, 20.0, 10.0)
        );
    }

    #[test]
    fn debug_output_omits_pixel_data() {
        let debug = format!("{:?}", retina_frame());
        assert!(debug.contains("width: 800") && !debug.contains("data"));
    }
}
