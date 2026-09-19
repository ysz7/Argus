//! The observation worker and the history of observations.
//!
//! Observations are made one at a time on a single thread that owns the
//! observers: platform backends need not be thread-safe, and two
//! observations of the screen at once would only slow each other down.
//!
//! Every distinct request (application and sources) has its own observer and
//! thus its own tracking session, so programs watching different
//! applications do not end each other's sessions.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};

use argus_core::accessibility::AppTarget;
use argus_core::{Inspection, Observer};
use argus_protocol::{Frame, Observation, Source};

use crate::http::Response;

/// Creates an observer; called once per tracking session.
pub type ObserverFactory = Box<dyn Fn() -> argus_core::Result<Observer> + Send>;

/// Requests that may wait for the worker at once; more are refused.
const QUEUE: usize = 8;

/// What to observe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObserveRequest {
    pub(crate) target: AppTarget,
    pub(crate) sources: Vec<Source>,
}

impl ObserveRequest {
    /// Identifies the tracking session of the request.
    fn session(&self) -> String {
        let target = match &self.target {
            AppTarget::Frontmost => "frontmost".to_owned(),
            AppTarget::Pid(pid) => format!("pid:{pid}"),
            AppTarget::Name(name) => format!("name:{}", name.to_lowercase()),
        };
        let mut sources: Vec<String> =
            self.sources.iter().map(|source| format!("{source:?}")).collect();
        sources.sort();
        sources.dedup();
        format!("{target}|{}", sources.join(","))
    }
}

/// An observation made by the service.
#[derive(Debug)]
pub(crate) struct Entry {
    pub(crate) inspection: Inspection,
    /// The request that produced it.
    pub(crate) request: ObserveRequest,
    /// The frames its pixels were read from, kept only while it is the
    /// latest observation of its session (for `/frame` crops).
    pub(crate) frames: Mutex<Vec<Frame>>,
}

impl Entry {
    pub(crate) fn observation(&self) -> &Observation {
        &self.inspection.observation
    }
}

/// The most recent observations, oldest first.
#[derive(Debug)]
pub(crate) struct History {
    entries: VecDeque<Arc<Entry>>,
    capacity: usize,
}

impl History {
    pub(crate) fn new(capacity: usize) -> Self {
        Self { entries: VecDeque::new(), capacity: capacity.max(1) }
    }

    fn push(&mut self, entry: Arc<Entry>) {
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        // Only the latest frames of a session are kept.
        let session = entry.request.session();
        for older in &self.entries {
            if older.request.session() == session {
                lock(&older.frames).clear();
            }
        }
        self.entries.push_back(entry);
    }

    pub(crate) fn get(&self, id: &str) -> Option<Arc<Entry>> {
        self.entries.iter().rev().find(|entry| entry.observation().id.as_str() == id).cloned()
    }

    pub(crate) fn latest(&self) -> Option<Arc<Entry>> {
        self.entries.back().cloned()
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether `to` continues the tracking session of observation `from`,
    /// directly or through observations still in the history.
    pub(crate) fn continues(&self, from: &str, to: &Observation) -> bool {
        let mut previous = to.previous.clone();
        while let Some(id) = previous {
            if id.as_str() == from {
                return true;
            }
            previous = self.get(id.as_str()).and_then(|entry| entry.observation().previous.clone());
        }
        false
    }
}

/// Locks `mutex`, ignoring poisoning (the data stays consistent: every
/// update is a single push).
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct Job {
    request: ObserveRequest,
    reply: mpsc::Sender<Result<Arc<Entry>, Response>>,
}

/// Handle to the observation thread.
#[derive(Debug, Clone)]
pub(crate) struct Worker {
    jobs: SyncSender<Job>,
}

impl Worker {
    /// Starts the observation thread. Fails if `factory` cannot create an
    /// observer (e.g. an unsupported platform).
    pub(crate) fn start(
        factory: ObserverFactory,
        history: Arc<Mutex<History>>,
        sessions: usize,
    ) -> argus_core::Result<Self> {
        let (jobs, queue) = mpsc::sync_channel(QUEUE);
        let (ready, started) = mpsc::channel();
        std::thread::Builder::new()
            .name("argus-observer".to_owned())
            .spawn(move || {
                // The first observer proves the factory works.
                match factory() {
                    Ok(observer) => {
                        let _ = ready.send(Ok(()));
                        run(factory, observer, &queue, &history, sessions.max(1));
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                    }
                }
            })
            .expect("failed to spawn the observation thread");
        started.recv().expect("the observation thread reports its start")?;
        Ok(Self { jobs })
    }

    /// Observes on the worker thread and records the observation.
    pub(crate) fn observe(&self, request: ObserveRequest) -> Result<Arc<Entry>, Response> {
        let (reply, answer) = mpsc::channel();
        match self.jobs.try_send(Job { request, reply }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                return Err(Response::error(
                    503,
                    "busy",
                    "too many observations are waiting; retry later".to_owned(),
                ));
            }
            Err(TrySendError::Disconnected(_)) => return Err(internal()),
        }
        answer.recv().unwrap_or_else(|_| Err(internal()))
    }
}

fn internal() -> Response {
    Response::error(500, "internal", "the observation thread stopped".to_owned())
}

/// A tracking session: an observer and when it was last used.
struct Session {
    key: String,
    observer: Observer,
    used: u64,
}

fn run(
    factory: ObserverFactory,
    first: Observer,
    queue: &Receiver<Job>,
    history: &Mutex<History>,
    capacity: usize,
) {
    let mut spare = Some(first);
    let mut sessions: Vec<Session> = Vec::new();
    for (clock, job) in (1u64..).zip(queue.iter()) {
        let key = job.request.session();
        let index = match sessions.iter().position(|session| session.key == key) {
            Some(index) => index,
            None => {
                let observer = match spare.take().map_or_else(&factory, Ok) {
                    Ok(observer) => observer,
                    Err(error) => {
                        let _ = job.reply.send(Err(crate::api::failure(&error)));
                        continue;
                    }
                };
                if sessions.len() == capacity {
                    // The least recently used session ends.
                    let oldest = (0..sessions.len()).min_by_key(|&i| sessions[i].used).unwrap_or(0);
                    tracing::debug!(session = %sessions[oldest].key, "tracking session ended");
                    sessions.swap_remove(oldest);
                }
                tracing::debug!(session = %key, "tracking session started");
                sessions.push(Session { key, observer, used: clock });
                sessions.len() - 1
            }
        };
        let session = &mut sessions[index];
        session.used = clock;
        let request = job.request;
        let result = catch_unwind(AssertUnwindSafe(|| {
            session.observer.inspect(&request.target, &request.sources)
        }));
        let reply = match result {
            Ok(Ok(mut inspection)) => {
                let frames = Mutex::new(std::mem::take(&mut inspection.frames));
                let entry = Arc::new(Entry { inspection, request, frames });
                lock(history).push(Arc::clone(&entry));
                Ok(entry)
            }
            Ok(Err(error)) => Err(crate::api::failure(&error)),
            Err(_) => {
                // The observer may be inconsistent; the session ends.
                tracing::error!(session = %session.key, "an observation panicked");
                sessions.swap_remove(index);
                Err(Response::error(500, "internal", "the observation failed".to_owned()))
            }
        };
        let _ = job.reply.send(reply);
    }
}

#[cfg(test)]
mod tests {
    use argus_protocol::{ObservationId, Timestamp};

    use super::*;

    fn entry(id: &str, previous: Option<&str>) -> Arc<Entry> {
        let mut observation = Observation::new(ObservationId::new(id).unwrap(), Timestamp(0));
        observation.previous = previous.map(|id| ObservationId::new(id).unwrap());
        Arc::new(Entry {
            inspection: Inspection {
                observation,
                evidence: Vec::new(),
                perception: None,
                tracking: Default::default(),
                timings: Default::default(),
                frames: Vec::new(),
            },
            request: ObserveRequest { target: AppTarget::Frontmost, sources: Vec::new() },
            frames: Mutex::new(Vec::new()),
        })
    }

    #[test]
    fn history_keeps_the_latest_observations() {
        let mut history = History::new(2);
        history.push(entry("a", None));
        history.push(entry("b", Some("a")));
        history.push(entry("c", Some("b")));
        assert_eq!(history.len(), 2);
        assert!(history.get("a").is_none());
        assert_eq!(history.latest().unwrap().observation().id.as_str(), "c");
    }

    #[test]
    fn sessions_are_followed_through_the_history() {
        let mut history = History::new(8);
        for (id, previous) in [("a", None), ("x", None), ("b", Some("a")), ("c", Some("b"))] {
            history.push(entry(id, previous));
        }
        let next = entry("d", Some("c"));
        assert!(history.continues("c", next.observation()));
        assert!(history.continues("a", next.observation()));
        assert!(!history.continues("x", next.observation()), "another session");
        assert!(!history.continues("a", entry("e", None).observation()), "a new session");
    }

    #[test]
    fn requests_share_a_session_only_for_the_same_application_and_sources() {
        let request = |target, sources: &[Source]| {
            ObserveRequest { target, sources: sources.to_vec() }.session()
        };
        let name = |name: &str| AppTarget::Name(name.to_owned());
        assert_eq!(
            request(name("Calculator"), &[Source::Ocr, Source::Accessibility]),
            request(name("calculator"), &[Source::Accessibility, Source::Ocr]),
        );
        assert_ne!(
            request(name("Calculator"), &[Source::Accessibility]),
            request(name("Calculator"), &[Source::Accessibility, Source::Ocr]),
        );
        assert_ne!(
            request(AppTarget::Pid(1), &[Source::Ocr]),
            request(AppTarget::Frontmost, &[Source::Ocr])
        );
    }
}
