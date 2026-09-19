//! Crops of observed frames: the images of weak regions shown to agents.

use argus_protocol::{Bounds, Frame};

/// Largest side of a crop, in image pixels.
const MAX_SIDE: u32 = 1600;

/// An RGBA image (straight alpha) cropped from a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crop {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Row-major RGBA bytes.
    pub rgba: Vec<u8>,
}

impl Crop {
    /// The image as PNG.
    pub fn png(&self) -> Option<Vec<u8>> {
        let mut png = Vec::new();
        let mut encoder = png::Encoder::new(&mut png, self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(&self.rgba).ok()?;
        writer.finish().ok()?;
        Some(png)
    }

    /// Whether `other` looks different: another size, or more than
    /// `threshold` (a fraction) of its pixels changed visibly. Anti-aliasing
    /// and cursor blinks stay below a small threshold.
    pub fn differs(&self, other: &Crop, threshold: f32) -> bool {
        if (self.width, self.height) != (other.width, other.height) {
            return true;
        }
        let pixels = (self.width * self.height).max(1) as f32;
        let changed = self
            .rgba
            .chunks_exact(4)
            .zip(other.rgba.chunks_exact(4))
            .filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > 24))
            .count();
        changed as f32 / pixels > threshold
    }
}

/// The part of `frames` covering `region` (global points), at `scale`
/// image pixels per point, as a PNG. `None` if no frame covers it.
pub fn crop_png(frames: &[Frame], region: Bounds, scale: f32) -> Option<Vec<u8>> {
    crop(frames, region, scale)?.png()
}

/// The part of `frames` covering `region` (global points), at `scale`
/// image pixels per point. `None` if no frame covers it.
pub fn crop(frames: &[Frame], region: Bounds, scale: f32) -> Option<Crop> {
    let area = |bounds: Option<Bounds>| bounds.map_or(0.0, |b| b.width() * b.height());
    let frame = frames.iter().max_by(|a, b| {
        area(a.bounds().intersection(&region)).total_cmp(&area(b.bounds().intersection(&region)))
    })?;
    let visible = frame.bounds().intersection(&region)?;
    let factor = frame.scale_factor();
    let to_pixels = |value: f32| (value * factor).round().max(0.0) as u32;
    let x = to_pixels(visible.x() - frame.bounds().x()).min(frame.width().saturating_sub(1));
    let y = to_pixels(visible.y() - frame.bounds().y()).min(frame.height().saturating_sub(1));
    let width = to_pixels(visible.width()).clamp(1, frame.width() - x);
    let height = to_pixels(visible.height()).clamp(1, frame.height() - y);
    let cropped = frame.crop(x, y, width, height).ok()?;

    // Box-filter down to the requested density, within the size limit.
    let mut step = (factor / scale.max(0.1)).max(1.0);
    step = step.max(width as f32 / MAX_SIDE as f32).max(height as f32 / MAX_SIDE as f32);
    let out_width = ((width as f32 / step).round() as u32).max(1);
    let out_height = ((height as f32 / step).round() as u32).max(1);
    let source = cropped.pixels();
    let mut data = Vec::with_capacity((out_width * out_height * 4) as usize);
    for oy in 0..out_height {
        let (y0, y1) = span(oy, step, height);
        for ox in 0..out_width {
            let (x0, x1) = span(ox, step, width);
            let mut sum = [0u32; 4];
            let mut count = 0;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    if let Some(pixel) = source.pixel(sx, sy) {
                        for (total, channel) in sum.iter_mut().zip(pixel) {
                            *total += u32::from(channel);
                        }
                        count += 1;
                    }
                }
            }
            let count = count.max(1);
            let [r, g, b, a] = sum.map(|total| (total / count) as u8);
            // Premultiplied → straight alpha, as PNG expects.
            let straight =
                |c: u8| if a == 0 { 0 } else { (u32::from(c) * 255 / u32::from(a)).min(255) as u8 };
            data.extend_from_slice(&[straight(r), straight(g), straight(b), a]);
        }
    }
    Some(Crop { width: out_width, height: out_height, rgba: data })
}

/// The source pixels `[start, end)` covered by output pixel `index`.
fn span(index: u32, step: f32, limit: u32) -> (u32, u32) {
    let start = ((index as f32 * step) as u32).min(limit.saturating_sub(1));
    let end = (((index + 1) as f32 * step).ceil() as u32).clamp(start + 1, limit);
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_protocol::{FrameId, PixelBuffer, Timestamp};

    #[test]
    fn crops_at_point_density() {
        // A 20x10 point frame at 2x: 40x20 pixels, left half red, right half blue.
        let mut data = Vec::new();
        for _ in 0..20 {
            for x in 0..40 {
                data.extend_from_slice(if x < 20 { &[255, 0, 0, 255] } else { &[0, 0, 255, 255] });
            }
        }
        let pixels = PixelBuffer::new(40, 20, data).unwrap();
        let frame = Frame::new(
            FrameId(1),
            Timestamp(0),
            Bounds::new(100.0, 50.0, 20.0, 10.0).unwrap(),
            2.0,
            pixels,
        )
        .unwrap();
        let png = crop_png(&[frame], Bounds::new(105.0, 50.0, 10.0, 10.0).unwrap(), 1.0).unwrap();
        let mut reader = png::Decoder::new(std::io::Cursor::new(png)).read_info().unwrap();
        let mut image = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut image).unwrap();
        assert_eq!((info.width, info.height), (10, 10));
        assert_eq!(&image[0..4], &[255, 0, 0, 255]);
        assert_eq!(&image[36..40], &[0, 0, 255, 255]);
    }

    #[test]
    fn small_changes_do_not_count_as_different() {
        let image = |changed: usize| {
            let mut rgba = vec![255u8; 100 * 100 * 4];
            for pixel in rgba.chunks_exact_mut(4).take(changed) {
                pixel[..3].copy_from_slice(&[0, 0, 0]);
            }
            Crop { width: 100, height: 100, rgba }
        };
        assert!(!image(0).differs(&image(10), 0.005), "0.1% changed: a blinking caret");
        assert!(image(0).differs(&image(200), 0.005), "2% changed");
        assert!(image(0).differs(&Crop { width: 1, height: 1, rgba: vec![0; 4] }, 0.005));
    }
}
