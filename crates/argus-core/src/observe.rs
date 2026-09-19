//! The observation pipeline.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use argus_accessibility::{AccessibilityBackend, AppTarget};
use argus_capture::{CaptureBackend, CaptureTarget, WindowInfo, main_window};
use argus_fusion::ElementEvidence;
use argus_perception::{OcrBackend, VisualPerceptionBackend};
use argus_protocol::{Bounds, Element, Observation, ObservationId, Source, Timestamp, Window};
use argus_tracking::Tracker;

use crate::assemble::assemble;
use crate::fuse::fuse;
use crate::normalize::normalize;
use crate::{Error, Result};

/// Source of process-unique observation numbers.
static NEXT_OBSERVATION: AtomicU64 = AtomicU64::new(1);

/// Produces observations of user interfaces.
///
/// Each source needs its backend; observing through a source whose backend
/// is missing fails with [`Error::Unsupported`].
///
/// An observer tracks elements across its observations: successive
/// observations of one application form a session in which an element keeps
/// its ID (see [`argus_tracking`]).
#[derive(Default)]
pub struct Observer {
    accessibility: Option<Box<dyn AccessibilityBackend>>,
    capture: Option<Box<dyn CaptureBackend>>,
    ocr: Option<Box<dyn OcrBackend>>,
    vision: Option<Box<dyn VisualPerceptionBackend>>,
    tracker: Mutex<Tracker>,
}

impl std::fmt::Debug for Observer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Observer")
            .field("accessibility", &self.accessibility.is_some())
            .field("capture", &self.capture.is_some())
            .field("ocr", &self.ocr.is_some())
            .field("vision", &self.vision.is_some())
            .finish()
    }
}

impl Observer {
    /// Creates an observer with the platform's default backends.
    pub fn new() -> Result<Self> {
        Ok(Self::default()
            .with_accessibility(argus_accessibility::default_backend()?)
            .with_capture(argus_capture::default_backend()?)
            .with_ocr(argus_perception::default_ocr_backend()?)
            .with_vision(Box::new(argus_perception::HeuristicDetector::default())))
    }

    /// Uses `backend` for accessibility trees.
    pub fn with_accessibility(mut self, backend: Box<dyn AccessibilityBackend>) -> Self {
        self.accessibility = Some(backend);
        self
    }

    /// Uses `backend` for frames and window layout.
    pub fn with_capture(mut self, backend: Box<dyn CaptureBackend>) -> Self {
        self.capture = Some(backend);
        self
    }

    /// Uses `backend` for text recognition.
    pub fn with_ocr(mut self, backend: Box<dyn OcrBackend>) -> Self {
        self.ocr = Some(backend);
        self
    }

    /// Uses `backend` for visual UI detection.
    pub fn with_vision(mut self, backend: Box<dyn VisualPerceptionBackend>) -> Self {
        self.vision = Some(backend);
        self
    }

    /// Ends the tracking session: the next observation gets fresh element
    /// IDs and no [`Observation::previous`].
    pub fn reset_tracking(&self) {
        self.tracker.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).reset();
    }

    /// Observes `target` using the given evidence sources and fuses their
    /// evidence into one observation.
    ///
    /// With accessibility, the focused window of the application is observed
    /// and pixel sources read the same window. Without it, pixel sources read
    /// the application's main window.
    pub fn observe(&self, target: &AppTarget, sources: &[Source]) -> Result<Observation> {
        Ok(self.inspect(target, sources)?.observation)
    }

    /// Like [`Observer::observe`], but also returns the evidence behind every
    /// element.
    pub fn inspect(&self, target: &AppTarget, sources: &[Source]) -> Result<Inspection> {
        if sources.is_empty() {
            return Err(Error::NoSources);
        }
        for source in sources {
            if !matches!(source, Source::Accessibility | Source::Ocr | Source::Vision) {
                return Err(Error::Unsupported { feature: "the requested evidence source" });
            }
        }
        let wants = |source: Source| sources.contains(&source);
        let started = Timestamp::now();
        let mut timestamp = started;
        let mut evidence = Vec::new();
        let mut application = None;
        let mut window = None;

        let pixels = wants(Source::Ocr) || wants(Source::Vision);
        let snapshot = if wants(Source::Accessibility) {
            let accessibility = self
                .accessibility
                .as_deref()
                .ok_or(Error::Unsupported { feature: "accessibility" })?;
            match accessibility.snapshot(target) {
                Ok(snapshot) => Some(snapshot),
                // Applications that expose no accessible window (custom
                // toolkits, games) are exactly where pixels are needed.
                Err(
                    error @ (argus_accessibility::Error::NoWindow
                    | argus_accessibility::Error::Platform(_)),
                ) if pixels => {
                    tracing::warn!(
                        %error,
                        "accessibility is unavailable for this window; observing its pixels only"
                    );
                    None
                }
                Err(error) => return Err(error.into()),
            }
        } else {
            None
        };
        if let Some(snapshot) = &snapshot {
            if snapshot.truncated {
                tracing::warn!("accessibility tree was truncated; the observation is incomplete");
            }
            evidence.push(normalize(argus_accessibility::candidates(snapshot)));
            application = Some(snapshot.application.clone());
            window = Some(Window {
                title: snapshot.window.title.clone().filter(|title| !title.is_empty()),
                bounds: snapshot.window.frame.and_then(|frame| {
                    Bounds::new(
                        frame.x as f32,
                        frame.y as f32,
                        frame.width as f32,
                        frame.height as f32,
                    )
                    .ok()
                }),
            });
        }

        let capture = if pixels {
            let capture =
                self.capture.as_deref().ok_or(Error::Unsupported { feature: "capture" })?;
            match (&snapshot, window.as_ref().and_then(|window| window.bounds)) {
                (Some(snapshot), Some(bounds)) => {
                    match window_like(capture, snapshot.application.pid, &bounds) {
                        Ok(captured) => Some((capture, captured)),
                        // Off screen or on another Space: the tree is still
                        // worth reporting.
                        Err(Error::WindowMismatch) => {
                            tracing::warn!(
                                "the window is not capturable; observing its accessibility tree only"
                            );
                            None
                        }
                        Err(error) => return Err(error),
                    }
                }
                _ => Some((capture, target_window(capture, target)?)),
            }
        } else {
            None
        };

        if let Some((capture, captured)) = capture {
            let frame = capture.capture(CaptureTarget::Window(captured.id))?;
            timestamp = frame.timestamp();
            if wants(Source::Ocr) {
                let ocr = self.ocr.as_deref().ok_or(Error::Unsupported { feature: "OCR" })?;
                let regions = ocr.detect(&frame)?;
                evidence.push(normalize(argus_perception::ocr_candidates(&frame, &regions)));
            }
            if wants(Source::Vision) {
                let vision = self
                    .vision
                    .as_deref()
                    .ok_or(Error::Unsupported { feature: "visual detection" })?;
                let detections = vision.detect(&frame)?;
                evidence.push(normalize(argus_perception::vision_candidates(&frame, &detections)));
            }
            if snapshot.is_none() {
                application = Some(captured.application);
                window = Some(Window { title: captured.title, bounds: Some(captured.bounds) });
            }
        }

        let fused = fuse(&evidence);
        let mut observation =
            assemble(next_observation_id(), timestamp, application, window, &fused);
        let tracking_started = std::time::Instant::now();
        let report = self
            .tracker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .track(&mut observation, &fused.native_ids());
        tracing::debug!(
            elements = observation.elements.len(),
            continued = report.continued,
            kept = report.kept,
            restored = report.restored,
            new = report.new,
            lost = report.lost,
            uncertain = report.uncertain,
            tracking_ms = tracking_started.elapsed().as_millis() as u64,
            elapsed_ms = Timestamp::now().0.saturating_sub(started.0),
            "assembled observation"
        );
        Ok(Inspection { observation, evidence: fused.into_evidence() })
    }
}

/// An observation together with the evidence behind its elements.
#[derive(Debug, Clone, PartialEq)]
pub struct Inspection {
    /// The observation.
    pub observation: Observation,
    /// Evidence for each element: `evidence[i]` belongs to
    /// `observation.elements[i]`.
    pub evidence: Vec<ElementEvidence>,
}

impl Inspection {
    /// The element with the given ID and its evidence.
    pub fn element(&self, id: &str) -> Option<(&Element, &ElementEvidence)> {
        let index =
            self.observation.elements.iter().position(|element| element.id.as_str() == id)?;
        Some((&self.observation.elements[index], &self.evidence[index]))
    }
}

/// The main window of the target application.
fn target_window(capture: &dyn CaptureBackend, target: &AppTarget) -> Result<WindowInfo> {
    let belongs = |window: &WindowInfo| match target {
        AppTarget::Frontmost => true,
        AppTarget::Pid(pid) => window.application.pid == Some(*pid),
        AppTarget::Name(name) => [&window.application.name, &window.application.bundle_id]
            .into_iter()
            .flatten()
            .any(|text| text.eq_ignore_ascii_case(name)),
    };
    let window = match target {
        AppTarget::Frontmost => capture.frontmost_window()?,
        _ => main_window(capture.windows()?.into_iter().filter(belongs)).ok_or_else(|| {
            argus_accessibility::Error::ApplicationNotFound(format!("{target:?} with a window"))
        })?,
    };
    Ok(window)
}

/// The capturable window of process `pid` that best overlaps `bounds` (the
/// window whose accessibility tree was read).
fn window_like(
    capture: &dyn CaptureBackend,
    pid: Option<u32>,
    bounds: &Bounds,
) -> Result<WindowInfo> {
    let overlap = |window: &WindowInfo| {
        let common = window.bounds.intersection(bounds).map_or(0.0, |c| c.width() * c.height());
        let union = window.bounds.width() * window.bounds.height()
            + bounds.width() * bounds.height()
            - common;
        if union > 0.0 { common / union } else { 0.0 }
    };
    capture
        .windows()?
        .into_iter()
        .filter(|window| window.layer == 0 && (pid.is_none() || window.application.pid == pid))
        .map(|window| (overlap(&window), window))
        .filter(|(overlap, _)| *overlap >= 0.5)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, window)| window)
        .ok_or(Error::WindowMismatch)
}

/// A new observation ID, unique across runs: the time the process made its
/// first observation (milliseconds, base 36) and a counter.
fn next_observation_id() -> ObservationId {
    static RUN: OnceLock<String> = OnceLock::new();
    let run = RUN.get_or_init(|| base36(Timestamp::now().0));
    let number = NEXT_OBSERVATION.fetch_add(1, Ordering::Relaxed);
    ObservationId::new(format!("obs_{run}_{number:06}")).expect("generated ids are non-empty")
}

fn base36(mut value: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut digits = Vec::new();
    loop {
        digits.push(DIGITS[(value % 36) as usize]);
        value /= 36;
        if value == 0 {
            break;
        }
    }
    digits.reverse();
    String::from_utf8(digits).expect("ASCII digits")
}

#[cfg(test)]
mod tests {
    use argus_accessibility::{AxFrame, AxNode, AxSnapshot};
    use argus_capture::DisplayInfo;
    use argus_perception::OcrRegion;
    use argus_protocol::{Application, Frame, FrameId, PixelBuffer, PixelRect, Role, Score};

    use super::*;

    struct FakeAccessibility(AxSnapshot);

    impl AccessibilityBackend for FakeAccessibility {
        fn has_permission(&self) -> bool {
            true
        }

        fn snapshot(&self, _target: &AppTarget) -> argus_accessibility::Result<AxSnapshot> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn observes_through_the_accessibility_pipeline() {
        let frame = |x, y, width, height| Some(AxFrame { x, y, width, height });
        let snapshot = AxSnapshot {
            application: Application { name: Some("Demo".to_owned()), ..Application::default() },
            window: AxNode {
                role: "AXWindow".to_owned(),
                title: Some("Demo".to_owned()),
                frame: frame(0.0, 0.0, 300.0, 200.0),
                children: vec![AxNode {
                    role: "AXButton".to_owned(),
                    title: Some("OK".to_owned()),
                    frame: frame(10.0, 10.0, 80.0, 24.0),
                    ..AxNode::default()
                }],
                ..AxNode::default()
            },
            truncated: false,
        };
        let observer =
            Observer::default().with_accessibility(Box::new(FakeAccessibility(snapshot)));

        let first = observer.observe(&AppTarget::Frontmost, &[Source::Accessibility]).unwrap();
        first.validate().unwrap();
        assert_eq!(first.window.as_ref().unwrap().title.as_deref(), Some("Demo"));
        assert_eq!(first.elements[1].role, Role::Button);
        assert_eq!(first.elements[1].name.as_deref(), Some("OK"));

        let second = observer.observe(&AppTarget::Frontmost, &[Source::Accessibility]).unwrap();
        assert_ne!(first.id, second.id);
        let run = |id: &ObservationId| id.as_str().rsplit_once('_').unwrap().0.to_owned();
        assert_eq!(run(&first.id), run(&second.id), "one run");
        assert_eq!(base36(0), "0");
        assert_eq!(base36(1_789_797_328_309), "mu7z4911");
        // The same window again: the same elements, tracked.
        assert_eq!(second.previous.as_ref(), Some(&first.id));
        for (a, b) in first.elements.iter().zip(&second.elements) {
            assert_eq!(a.id, b.id);
            assert_eq!(b.confidence.identity.map(|s| s.get()), Some(1.0));
        }

        observer.reset_tracking();
        let third = observer.observe(&AppTarget::Frontmost, &[Source::Accessibility]).unwrap();
        assert_eq!(third.previous, None);
        assert_eq!(third.elements[0].id.as_str(), "e_3", "IDs are never reused");
    }

    struct FakeCapture;

    fn fake_window() -> WindowInfo {
        WindowInfo {
            id: 42,
            title: Some("Editor".to_owned()),
            application: Application {
                name: Some("Demo".to_owned()),
                bundle_id: Some("com.example.demo".to_owned()),
                pid: Some(7),
            },
            bounds: Bounds::new(100.0, 50.0, 200.0, 100.0).unwrap(),
            layer: 0,
        }
    }

    impl CaptureBackend for FakeCapture {
        fn has_permission(&self) -> bool {
            true
        }

        fn displays(&self) -> argus_capture::Result<Vec<DisplayInfo>> {
            Ok(Vec::new())
        }

        fn windows(&self) -> argus_capture::Result<Vec<WindowInfo>> {
            Ok(vec![fake_window()])
        }

        fn frontmost_application(&self) -> argus_capture::Result<Option<Application>> {
            Ok(Some(fake_window().application))
        }

        fn capture(&self, target: CaptureTarget) -> argus_capture::Result<Frame> {
            assert_eq!(target, CaptureTarget::Window(42));
            let pixels = PixelBuffer::new(400, 200, vec![0; 400 * 200 * 4]).unwrap();
            Ok(Frame::new(FrameId(1), Timestamp(5), fake_window().bounds, 2.0, pixels).unwrap())
        }
    }

    struct FakeOcr;

    impl OcrBackend for FakeOcr {
        fn detect(&self, _frame: &Frame) -> argus_perception::Result<Vec<OcrRegion>> {
            Ok(vec![OcrRegion {
                text: " Save ".to_owned(),
                rect: PixelRect { x: 20.0, y: 40.0, width: 80.0, height: 30.0 },
                confidence: Score::new(0.5).unwrap(),
            }])
        }
    }

    #[test]
    fn observes_through_the_ocr_pipeline() {
        let observer =
            Observer::default().with_capture(Box::new(FakeCapture)).with_ocr(Box::new(FakeOcr));
        for target in [AppTarget::Frontmost, AppTarget::Name("com.example.demo".to_owned())] {
            let observation = observer.observe(&target, &[Source::Ocr]).unwrap();
            observation.validate().unwrap();
            assert_eq!(observation.timestamp, Timestamp(5));
            assert_eq!(observation.window.as_ref().unwrap().title.as_deref(), Some("Editor"));

            let [text] = observation.elements.as_slice() else { panic!("{observation:#?}") };
            assert_eq!(text.role, Role::Text);
            assert_eq!(text.name.as_deref(), Some("Save"), "cleaned by normalization");
            assert_eq!(text.sources, [Source::Ocr]);
            // Pixel (20, 40) of a 2x frame at (100, 50) is point (110, 70).
            assert_eq!(text.bounds, Bounds::new(110.0, 70.0, 40.0, 15.0).unwrap());
        }
    }

    #[test]
    fn missing_backends_are_reported() {
        let error = Observer::default().observe(&AppTarget::Frontmost, &[Source::Ocr]).unwrap_err();
        assert_eq!(error.code(), "unsupported");
        let error = Observer::default().observe(&AppTarget::Frontmost, &[]).unwrap_err();
        assert_eq!(error.code(), "no_sources");
        let error =
            Observer::default().observe(&AppTarget::Frontmost, &[Source::Derived]).unwrap_err();
        assert_eq!(error.code(), "unsupported");
    }

    /// The fake window's accessibility tree: one unnamed button around the
    /// text the fake OCR finds.
    fn demo_snapshot(window_x: f64) -> AxSnapshot {
        let frame = |x, y, width, height| Some(AxFrame { x, y, width, height });
        AxSnapshot {
            application: Application { pid: Some(7), ..fake_window().application },
            window: AxNode {
                role: "AXWindow".to_owned(),
                title: Some("Editor".to_owned()),
                frame: frame(window_x, 50.0, 200.0, 100.0),
                children: vec![AxNode {
                    role: "AXButton".to_owned(),
                    frame: frame(105.0, 65.0, 60.0, 25.0),
                    ..AxNode::default()
                }],
                ..AxNode::default()
            },
            truncated: false,
        }
    }

    #[test]
    fn fuses_accessibility_with_pixels_of_the_same_window() {
        let observer = Observer::default()
            .with_accessibility(Box::new(FakeAccessibility(demo_snapshot(100.0))))
            .with_capture(Box::new(FakeCapture))
            .with_ocr(Box::new(FakeOcr));
        let inspection =
            observer.inspect(&AppTarget::Frontmost, &[Source::Accessibility, Source::Ocr]).unwrap();
        let observation = &inspection.observation;
        observation.validate().unwrap();
        assert_eq!(observation.timestamp, Timestamp(5), "the time of the frame");

        let [_, button] = observation.elements.as_slice() else { panic!("{observation:#?}") };
        assert_eq!((button.role, button.name.as_deref()), (Role::Button, Some("Save")));
        assert_eq!(button.sources, [Source::Accessibility, Source::Ocr]);

        let (element, evidence) = inspection.element(button.id.as_str()).unwrap();
        assert_eq!(element, button);
        assert_eq!(evidence.contributions.len(), 2);
        assert!(inspection.element("e_404").is_none());
    }

    struct WindowlessAccessibility;

    impl AccessibilityBackend for WindowlessAccessibility {
        fn has_permission(&self) -> bool {
            true
        }

        fn snapshot(&self, _target: &AppTarget) -> argus_accessibility::Result<AxSnapshot> {
            Err(argus_accessibility::Error::NoWindow)
        }
    }

    #[test]
    fn pixels_stand_in_for_missing_accessibility() {
        let observer = Observer::default()
            .with_accessibility(Box::new(WindowlessAccessibility))
            .with_capture(Box::new(FakeCapture))
            .with_ocr(Box::new(FakeOcr));
        let all = [Source::Accessibility, Source::Ocr];
        let observation = observer.observe(&AppTarget::Name("Demo".to_owned()), &all).unwrap();
        observation.validate().unwrap();
        assert_eq!(observation.window.as_ref().unwrap().title.as_deref(), Some("Editor"));
        assert_eq!(observation.elements.len(), 1);
        assert_eq!(observation.elements[0].sources, [Source::Ocr]);

        // Asked for accessibility alone, the failure is reported.
        let error = observer.observe(&AppTarget::Frontmost, &[Source::Accessibility]).unwrap_err();
        assert_eq!(error.code(), "not_found");
    }

    #[test]
    fn pixels_of_another_window_are_not_fused() {
        let observer = Observer::default()
            .with_accessibility(Box::new(FakeAccessibility(demo_snapshot(900.0))))
            .with_capture(Box::new(FakeCapture))
            .with_ocr(Box::new(FakeOcr));
        // The accessibility window is elsewhere: its tree is reported alone.
        let observation =
            observer.observe(&AppTarget::Frontmost, &[Source::Accessibility, Source::Ocr]).unwrap();
        assert!(observation.elements.iter().all(|e| e.sources == [Source::Accessibility]));
        assert_eq!(observation.elements.len(), 2);
    }
}
