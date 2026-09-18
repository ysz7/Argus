//! A frame reduced to one pixel per point, with an edge map.

use argus_protocol::{Frame, PixelBuffer};

/// Frame pixels averaged down to (roughly) one pixel per point, so that
/// detection thresholds can be expressed in points on any display.
pub(crate) struct Raster {
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// Frame pixels per raster pixel.
    pub(crate) factor: usize,
    rgb: Vec<[u8; 3]>,
}

impl Raster {
    pub(crate) fn from_frame(frame: &Frame) -> Self {
        let factor = (frame.scale_factor().round() as usize).max(1);
        let source = frame.pixels();
        let (source_width, bytes) = (source.width() as usize, source.as_bytes());
        let width = source_width / factor;
        let height = source.height() as usize / factor;
        let area = (factor * factor) as u32;

        let mut rgb = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let mut sum = [0u32; 3];
                for dy in 0..factor {
                    let row = (y * factor + dy) * source_width;
                    for dx in 0..factor {
                        let offset = (row + x * factor + dx) * PixelBuffer::BYTES_PER_PIXEL;
                        for (channel, total) in sum.iter_mut().enumerate() {
                            *total += u32::from(bytes[offset + channel]);
                        }
                    }
                }
                rgb.push(sum.map(|total| (total / area) as u8));
            }
        }
        Self { width, height, factor, rgb }
    }

    pub(crate) fn rgb(&self, x: usize, y: usize) -> [u8; 3] {
        self.rgb[y * self.width + x]
    }

    /// Marks pixels whose color differs from the right or lower neighbor by
    /// at least `threshold` in any channel. Flat UI surfaces produce no
    /// edges; outlines, fills and glyphs do.
    pub(crate) fn edges(&self, threshold: u8) -> Vec<bool> {
        let mut edges = vec![false; self.width * self.height];
        for y in 0..self.height {
            for x in 0..self.width {
                let here = self.rgb(x, y);
                let differs = |other: [u8; 3]| {
                    here.iter().zip(other).any(|(a, b)| a.abs_diff(b) >= threshold)
                };
                edges[y * self.width + x] = (x + 1 < self.width && differs(self.rgb(x + 1, y)))
                    || (y + 1 < self.height && differs(self.rgb(x, y + 1)));
            }
        }
        edges
    }
}
