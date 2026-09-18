//! Debug overlay: element boxes drawn on top of a captured frame.
//!
//! Used to verify grounding by eye: every box must sit exactly on the control
//! it describes.

use argus_protocol::{Bounds, Frame, Observation, PixelBuffer, Role};

/// Returns a copy of the frame's pixels with the observation's visible
/// elements outlined, colored by role.
pub(crate) fn draw(frame: &Frame, observation: &Observation) -> Vec<u8> {
    let mut pixels = frame.pixels().as_bytes().to_vec();
    let thickness = frame.scale_factor().round().max(1.0) as i64;
    for element in &observation.elements {
        if element.state.visible == Some(false) {
            continue;
        }
        let rect = to_pixels(frame, element.visible_bounds.unwrap_or(element.bounds));
        outline(&mut pixels, frame.width(), frame.height(), rect, thickness, color(element.role));
    }
    pixels
}

fn color(role: Role) -> [u8; 4] {
    match role {
        Role::Button | Role::Link | Role::MenuItem => [255, 59, 48, 255],
        Role::Text => [0, 122, 255, 255],
        Role::TextBox => [52, 199, 89, 255],
        Role::Checkbox | Role::RadioButton | Role::Tab | Role::Slider => [255, 149, 0, 255],
        _ => [142, 142, 147, 255],
    }
}

/// `(left, top, right, bottom)` in frame pixels.
fn to_pixels(frame: &Frame, bounds: Bounds) -> (i64, i64, i64, i64) {
    let origin = frame.bounds();
    let scale = frame.scale_factor();
    let px = |value: f32| value.round() as i64;
    let left = px((bounds.x() - origin.x()) * scale);
    let top = px((bounds.y() - origin.y()) * scale);
    let right = px((bounds.x() + bounds.width() - origin.x()) * scale);
    let bottom = px((bounds.y() + bounds.height() - origin.y()) * scale);
    (left, top, right, bottom)
}

fn outline(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    (left, top, right, bottom): (i64, i64, i64, i64),
    thickness: i64,
    color: [u8; 4],
) {
    let mut fill = |x0: i64, y0: i64, x1: i64, y1: i64| {
        for y in y0.max(0)..y1.min(i64::from(height)) {
            for x in x0.max(0)..x1.min(i64::from(width)) {
                let offset =
                    (y as usize * width as usize + x as usize) * PixelBuffer::BYTES_PER_PIXEL;
                pixels[offset..offset + PixelBuffer::BYTES_PER_PIXEL].copy_from_slice(&color);
            }
        }
    };
    fill(left, top, right, top + thickness);
    fill(left, bottom - thickness, right, bottom);
    fill(left, top, left + thickness, bottom);
    fill(right - thickness, top, right, bottom);
}

#[cfg(test)]
mod tests {
    use argus_protocol::{
        Confidence, Element, ElementId, ElementState, FrameId, ObservationId, Score, Source,
        Timestamp,
    };

    use super::*;

    #[test]
    fn outlines_elements_at_their_pixel_position() {
        // A 2x frame of the region (100, 50)–(110, 60) in points.
        let bounds = Bounds::new(100.0, 50.0, 10.0, 10.0).unwrap();
        let pixels = PixelBuffer::new(20, 20, vec![0; 20 * 20 * 4]).unwrap();
        let frame = Frame::new(FrameId(1), Timestamp(0), bounds, 2.0, pixels).unwrap();

        let mut observation = Observation::new(ObservationId::new("obs").unwrap(), Timestamp(0));
        observation.elements.push(Element {
            id: ElementId::new("e_1").unwrap(),
            role: Role::Button,
            name: None,
            value: None,
            description: None,
            bounds: Bounds::new(102.0, 53.0, 4.0, 3.0).unwrap(),
            visible_bounds: None,
            state: ElementState::default(),
            confidence: Confidence::new(Score::CERTAIN),
            sources: vec![Source::Accessibility],
            parent: None,
            children: Vec::new(),
        });

        let drawn = draw(&frame, &observation);
        let at = |x: usize, y: usize| &drawn[(y * 20 + x) * 4..(y * 20 + x) * 4 + 4];
        // Box spans pixels x 4..12, y 6..12 with a 2-pixel border.
        assert_eq!(at(4, 6), [255, 59, 48, 255]);
        assert_eq!(at(11, 11), [255, 59, 48, 255]);
        assert_eq!(at(7, 9), [0, 0, 0, 0], "interior stays untouched");
        assert_eq!(at(3, 6), [0, 0, 0, 0], "nothing left of the box");
    }
}
