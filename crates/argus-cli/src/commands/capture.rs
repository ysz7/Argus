//! `argus capture`: acquire a single frame for debugging.
//!
//! Prints frame metadata as JSON. Pixels are discarded unless `--output` is
//! given explicitly.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use argus_core::Observer;
use argus_core::accessibility::AppTarget;
use argus_core::capture::{self, CaptureBackend, CaptureTarget};
use serde_json::json;

use crate::commands::observe::Source;
use crate::output::print_json;
use crate::overlay;

#[derive(Debug, clap::Args)]
pub(crate) struct CaptureArgs {
    /// List displays, the frontmost application and its windows instead of
    /// capturing.
    #[arg(long, conflicts_with_all = ["display", "window", "output", "delay"])]
    list: bool,

    /// Capture a display instead of the frontmost window. Without an ID, the
    /// main display.
    #[arg(long, value_name = "ID", num_args = 0..=1, conflicts_with = "window")]
    display: Option<Option<u32>>,

    /// Capture a specific window (IDs are shown by `--list`).
    #[arg(long, value_name = "ID")]
    window: Option<u32>,

    /// Seconds to wait before capturing, e.g. to switch to another app.
    #[arg(long, value_name = "SECONDS")]
    delay: Option<f64>,

    /// Save the frame as a PNG file for debugging. Frames are otherwise never
    /// written to disk.
    #[arg(long, short, value_name = "PATH")]
    output: Option<PathBuf>,

    /// Draw an observation of the window on the saved PNG (red: buttons,
    /// blue: text, green: text boxes, orange: toggles, gray: other).
    #[arg(long, requires = "output", conflicts_with = "display")]
    overlay: bool,

    /// Sources of the observation drawn by `--overlay`, comma-separated.
    #[arg(
        long = "overlay-sources",
        visible_alias = "overlay-source",
        value_enum,
        value_delimiter = ',',
        default_values_t = [Source::Accessibility],
        requires = "overlay"
    )]
    overlay_sources: Vec<Source>,
}

pub(crate) fn run(args: &CaptureArgs) -> anyhow::Result<()> {
    let backend = capture::default_backend()?;
    if args.list {
        return print_json(&layout(backend.as_ref())?);
    }

    if let Some(seconds) = args.delay {
        let delay = Duration::try_from_secs_f64(seconds)
            .with_context(|| format!("invalid delay `{seconds}`"))?;
        std::thread::sleep(delay);
    }

    // Resolve the frontmost window up front so the output can say which
    // window was captured.
    let (target, window) = match (args.display, args.window) {
        (Some(Some(id)), _) => (CaptureTarget::Display(id), None),
        (Some(None), _) => (CaptureTarget::MainDisplay, None),
        (None, Some(id)) => {
            let window = backend.windows()?.into_iter().find(|window| window.id == id);
            (CaptureTarget::Window(id), window)
        }
        (None, None) => {
            let window = backend.frontmost_window()?;
            (CaptureTarget::Window(window.id), Some(window))
        }
    };

    let frame = backend.capture(target)?;
    if let Some(path) = &args.output {
        let pixels = if args.overlay {
            let pid = window
                .as_ref()
                .and_then(|window| window.application.pid)
                .context("cannot tell which application owns the captured window")?;
            let sources: Vec<_> = args.overlay_sources.iter().map(|s| s.to_protocol()).collect();
            let observation = Observer::new()?.observe(&AppTarget::Pid(pid), &sources)?;
            overlay::draw(&frame, &observation)
        } else {
            frame.pixels().as_bytes().to_vec()
        };
        write_png(frame.width(), frame.height(), &pixels, path)?;
        tracing::info!(path = %path.display(), "saved debug frame");
    }

    print_json(&json!({
        "target": describe(target),
        "window": window,
        "frame": {
            "id": frame.id().to_string(),
            "timestamp": frame.timestamp(),
            "bounds": frame.bounds(),
            "scale_factor": frame.scale_factor(),
            "width": frame.width(),
            "height": frame.height(),
        },
        "output": args.output,
    }))
}

fn layout(backend: &dyn CaptureBackend) -> anyhow::Result<serde_json::Value> {
    let application = backend.frontmost_application()?;
    let windows: Vec<_> =
        backend.windows()?.into_iter().filter(|window| window.layer == 0).collect();
    Ok(json!({
        "screen_recording_permission": backend.has_permission(),
        "displays": backend.displays()?,
        "frontmost_application": application,
        "windows": windows,
    }))
}

fn describe(target: CaptureTarget) -> serde_json::Value {
    match target {
        CaptureTarget::FrontmostWindow => json!("frontmost_window"),
        CaptureTarget::Window(id) => json!({ "window": id }),
        CaptureTarget::MainDisplay => json!("main_display"),
        CaptureTarget::Display(id) => json!({ "display": id }),
    }
}

pub(crate) fn write_png(width: u32, height: u32, rgba: &[u8], path: &Path) -> anyhow::Result<()> {
    let file = File::create(path).with_context(|| format!("cannot create {}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    // Pixels are premultiplied; for opaque screen content this is identical
    // to straight alpha, which is all a debug image needs.
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba)?;
    writer.finish()?;
    Ok(())
}
