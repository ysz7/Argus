//! Partial visual detection must find what full detection finds, on real
//! AppKit controls (`tests/fixtures/vision`) edited the way interfaces
//! change: a control disappears, another one appears. Pure Rust.

use std::fs::File;
use std::path::PathBuf;

use argus_perception::{
    HeuristicDetector, VisualCandidate, VisualPerceptionBackend, frame_changes, update_detections,
};
use argus_protocol::{Bounds, Frame, FrameId, PixelBuffer, PixelRect, Role, Timestamp};

fn load(name: &str) -> (u32, u32, Vec<u8>) {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/vision").join(name);
    let mut reader =
        png::Decoder::new(std::io::BufReader::new(File::open(path).unwrap())).read_info().unwrap();
    let mut data = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut data).unwrap();
    data.truncate(info.buffer_size());
    (info.width, info.height, data)
}

fn frame(width: u32, height: u32, data: Vec<u8>) -> Frame {
    let bounds = Bounds::new(0.0, 0.0, width as f32 / 2.0, height as f32 / 2.0).unwrap();
    Frame::new(
        FrameId(1),
        Timestamp(0),
        bounds,
        2.0,
        PixelBuffer::new(width, height, data).unwrap(),
    )
    .unwrap()
}

/// Pixel rectangle `(x, y, width, height)` of a detection, grown by `grow`.
fn area(rect: &PixelRect, grow: u32) -> (u32, u32, u32, u32) {
    (
        rect.x as u32 - grow,
        rect.y as u32 - grow,
        rect.width as u32 + 2 * grow,
        rect.height as u32 + 2 * grow,
    )
}

fn fill(data: &mut [u8], width: u32, (x0, y0, w, h): (u32, u32, u32, u32), color: [u8; 4]) {
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            let offset = ((y * width + x) * 4) as usize;
            data[offset..offset + 4].copy_from_slice(&color);
        }
    }
}

fn copy(data: &mut [u8], width: u32, (x0, y0, w, h): (u32, u32, u32, u32), (tx, ty): (u32, u32)) {
    let source = data.to_vec();
    for y in 0..h {
        for x in 0..w {
            let from = (((y0 + y) * width + x0 + x) * 4) as usize;
            let to = (((ty + y) * width + tx + x) * 4) as usize;
            data[to..to + 4].copy_from_slice(&source[from..from + 4]);
        }
    }
}

/// A uniform area of `w × h` pixels, if the image has one.
fn blank(data: &[u8], width: u32, height: u32, w: u32, h: u32) -> Option<(u32, u32)> {
    let pixel =
        |x: u32, y: u32| &data[((y * width + x) * 4) as usize..((y * width + x) * 4 + 4) as usize];
    (0..height.saturating_sub(h)).step_by(8).find_map(|y| {
        (0..width.saturating_sub(w))
            .step_by(8)
            .find(|&x| {
                let color = pixel(x, y);
                (y..y + h).all(|py| (x..x + w).all(|px| pixel(px, py) == color))
            })
            .map(|x| (x, y))
    })
}

fn same(a: &VisualCandidate, b: &VisualCandidate) -> bool {
    let close = |p: f32, q: f32| (p - q).abs() <= 2.0;
    a.role == b.role
        && close(a.rect.x, b.rect.x)
        && close(a.rect.y, b.rect.y)
        && close(a.rect.width, b.rect.width)
        && close(a.rect.height, b.rect.height)
        && a.state == b.state
}

/// Detections of `left` that have no counterpart in `right`.
fn missing<'a>(left: &'a [VisualCandidate], right: &[VisualCandidate]) -> Vec<&'a VisualCandidate> {
    left.iter().filter(|a| !right.iter().any(|b| same(a, b))).collect()
}

fn check(image: &str) {
    let detector = HeuristicDetector::default();
    let (width, height, data) = load(image);
    let before = frame(width, height, data.clone());
    let previous = detector.detect(&before).unwrap();

    // A button appears in an empty area; a checkbox disappears.
    let button = previous.iter().find(|d| d.role == Role::Button).expect("a button");
    let checkbox = previous.iter().find(|d| d.role == Role::Checkbox).expect("a checkbox");
    let source = area(&button.rect, 6);
    let target = blank(&data, width, height, source.2 + 40, source.3 + 40).expect("an empty area");
    let mut edited = data.clone();
    copy(&mut edited, width, source, (target.0 + 20, target.1 + 20));
    let erase = area(&checkbox.rect, 2);
    let background = {
        let offset = (((erase.1) * width + erase.0 - 4) * 4) as usize;
        [edited[offset], edited[offset + 1], edited[offset + 2], edited[offset + 3]]
    };
    fill(&mut edited, width, erase, background);
    let after = frame(width, height, edited);

    let change = frame_changes(&before, &after).unwrap();
    let update = update_detections(&detector, &after, &previous, &change).unwrap();
    let full = detector.detect(&after).unwrap();

    // The edit is real: full detection sees a new button and misses the
    // checkbox.
    assert!(missing(&full, &previous).iter().any(|d| d.role == Role::Button), "{image}");
    assert!(missing(&previous, &full).iter().any(|d| d.role == Role::Checkbox), "{image}");
    assert!(update.reused > previous.len() / 2, "{image}: reused {}", update.reused);
    assert!(update.recomputed_share(&after) < 0.25, "{image}: {:?}", update.regions);
    let (lost, extra) = (missing(&full, &update.items), missing(&update.items, &full));
    assert!(lost.is_empty() && extra.is_empty(), "{image}: lost {lost:#?}\nextra {extra:#?}");
}

#[test]
fn partial_detection_matches_full_detection_light() {
    check("controls_light.png");
}

#[test]
fn partial_detection_matches_full_detection_dark() {
    check("controls_dark.png");
}
