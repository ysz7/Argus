//! The observation pipeline.

use std::sync::atomic::{AtomicU64, Ordering};

use argus_accessibility::{AccessibilityBackend, AppTarget};
use argus_protocol::{Bounds, Observation, ObservationId, Timestamp, Window};

use crate::Result;
use crate::assemble::assemble;
use crate::normalize::normalize;

/// Source of process-unique observation numbers.
static NEXT_OBSERVATION: AtomicU64 = AtomicU64::new(1);

/// Produces observations of user interfaces.
pub struct Observer {
    accessibility: Box<dyn AccessibilityBackend>,
}

impl std::fmt::Debug for Observer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Observer").finish_non_exhaustive()
    }
}

impl Observer {
    /// Creates an observer with the platform's default backends.
    pub fn new() -> Result<Self> {
        Ok(Self::with_backends(argus_accessibility::default_backend()?))
    }

    /// Creates an observer with explicit backends.
    pub fn with_backends(accessibility: Box<dyn AccessibilityBackend>) -> Self {
        Self { accessibility }
    }

    /// Observes the focused window of `target` using the accessibility tree.
    pub fn observe_accessibility(&self, target: &AppTarget) -> Result<Observation> {
        let timestamp = Timestamp::now();
        let snapshot = self.accessibility.snapshot(target)?;
        if snapshot.truncated {
            tracing::warn!("accessibility tree was truncated; the observation is incomplete");
        }

        let candidates = normalize(argus_accessibility::candidates(&snapshot));
        let window = Window {
            title: snapshot.window.title.clone().filter(|title| !title.is_empty()),
            bounds: snapshot.window.frame.and_then(|frame| {
                Bounds::new(frame.x as f32, frame.y as f32, frame.width as f32, frame.height as f32)
                    .ok()
            }),
        };
        let observation = assemble(
            next_observation_id(),
            timestamp,
            Some(snapshot.application),
            Some(window),
            &candidates,
        );
        tracing::debug!(elements = observation.elements.len(), "assembled observation");
        Ok(observation)
    }
}

fn next_observation_id() -> ObservationId {
    let number = NEXT_OBSERVATION.fetch_add(1, Ordering::Relaxed);
    ObservationId::new(format!("obs_{number:06}")).expect("generated ids are non-empty")
}

#[cfg(test)]
mod tests {
    use argus_accessibility::{AxFrame, AxNode, AxSnapshot};
    use argus_protocol::{Application, Role};

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
        let observer = Observer::with_backends(Box::new(FakeAccessibility(snapshot)));

        let first = observer.observe_accessibility(&AppTarget::Frontmost).unwrap();
        first.validate().unwrap();
        assert_eq!(first.window.as_ref().unwrap().title.as_deref(), Some("Demo"));
        assert_eq!(first.elements[1].role, Role::Button);
        assert_eq!(first.elements[1].name.as_deref(), Some("OK"));

        let second = observer.observe_accessibility(&AppTarget::Frontmost).unwrap();
        assert_ne!(first.id, second.id);
    }
}
