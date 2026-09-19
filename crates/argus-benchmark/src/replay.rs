//! Replaying recorded steps through the real observation pipeline.
//!
//! The recorded accessibility tree and frame stand in for the live
//! platform backends; text recognition and visual detection run for real.
//! The observer cannot tell a replay from a live window: fusion, the scene
//! graph, tracking and incremental perception behave as they would.

use std::sync::{Arc, Mutex};

use argus_core::Observer;
use argus_core::accessibility::{AccessibilityBackend, AppTarget, AxSnapshot};
use argus_core::capture::{CaptureBackend, CaptureTarget, DisplayInfo, WindowInfo};
use argus_protocol::{Application, Frame, FrameId, Timestamp};

use crate::dataset::Step;

/// The window id every replayed frame has.
const WINDOW: u32 = 1;
/// The process every replayed application has.
const PID: u32 = 1;

/// The step being replayed, shared by the backends.
#[derive(Debug, Clone, Default)]
pub(crate) struct Stage(Arc<Mutex<Option<Staged>>>);

#[derive(Debug, Clone)]
struct Staged {
    frame: Frame,
    window: WindowInfo,
    accessibility: Option<AxSnapshot>,
}

impl Stage {
    /// Makes `step` the current state of the replayed window. `number`
    /// orders the frames in time.
    pub(crate) fn set(&self, step: &Step, number: u64) {
        let application = Application { pid: Some(PID), ..step.info.application.clone() };
        let frame = Frame::new(
            FrameId(number),
            Timestamp(number * 1000),
            step.frame.bounds(),
            step.frame.scale_factor(),
            step.frame.pixels().clone(),
        )
        .expect("a loaded frame is valid");
        let accessibility = step.accessibility.clone().map(|mut snapshot| {
            snapshot.application = application.clone();
            snapshot
        });
        let window = WindowInfo {
            id: WINDOW,
            title: step.info.title.clone(),
            application,
            bounds: step.info.bounds,
            layer: 0,
        };
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(Staged { frame, window, accessibility });
    }

    fn get(&self) -> Option<Staged> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

/// The target that observes the replayed window.
pub(crate) const TARGET: AppTarget = AppTarget::Pid(PID);

/// An observer of whatever `stage` holds.
pub(crate) fn observer(
    stage: &Stage,
    ocr: Option<Box<dyn argus_core::perception::OcrBackend>>,
) -> Observer {
    let mut observer = Observer::default()
        .with_accessibility(Box::new(ReplayAccessibility(stage.clone())))
        .with_capture(Box::new(ReplayCapture(stage.clone())))
        .with_vision(Box::new(argus_core::perception::HeuristicDetector::default()));
    if let Some(ocr) = ocr {
        observer = observer.with_ocr(ocr);
    }
    observer
}

struct ReplayAccessibility(Stage);

impl AccessibilityBackend for ReplayAccessibility {
    fn has_permission(&self) -> bool {
        true
    }

    fn snapshot(&self, _target: &AppTarget) -> argus_core::accessibility::Result<AxSnapshot> {
        self.0.get().and_then(|staged| staged.accessibility).ok_or(
            // A step recorded without a tree: an application that exposes
            // no accessible window.
            argus_core::accessibility::Error::NoWindow,
        )
    }
}

struct ReplayCapture(Stage);

impl CaptureBackend for ReplayCapture {
    fn has_permission(&self) -> bool {
        true
    }

    fn displays(&self) -> argus_core::capture::Result<Vec<DisplayInfo>> {
        Ok(Vec::new())
    }

    fn windows(&self) -> argus_core::capture::Result<Vec<WindowInfo>> {
        Ok(self.0.get().map(|staged| vec![staged.window]).unwrap_or_default())
    }

    fn frontmost_application(&self) -> argus_core::capture::Result<Option<Application>> {
        Ok(self.0.get().map(|staged| staged.window.application))
    }

    fn capture(&self, target: CaptureTarget) -> argus_core::capture::Result<Frame> {
        match (target, self.0.get()) {
            (CaptureTarget::Window(WINDOW) | CaptureTarget::FrontmostWindow, Some(staged)) => {
                Ok(staged.frame)
            }
            (CaptureTarget::Window(id), _) => Err(argus_core::capture::Error::WindowNotFound(id)),
            _ => Err(argus_core::capture::Error::NoFrontmostWindow),
        }
    }
}
