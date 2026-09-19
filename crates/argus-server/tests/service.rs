//! The service end to end, over real TCP connections, with a scripted
//! accessibility backend (no screen access needed).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};

use argus_core::Observer;
use argus_core::accessibility::{
    AccessibilityBackend, AppTarget, AxFrame, AxNode, AxSnapshot, Error as AxError,
};
use argus_protocol::{Application, Observation, ObservationDelta};
use argus_server::{Config, Server};
use serde_json::Value;

/// What the scripted application shows: its name and the button's title.
#[derive(Debug, Clone)]
struct Screen {
    application: String,
    button: String,
}

struct Scripted(Arc<Mutex<Screen>>);

impl AccessibilityBackend for Scripted {
    fn has_permission(&self) -> bool {
        true
    }

    fn snapshot(&self, target: &AppTarget) -> argus_core::accessibility::Result<AxSnapshot> {
        let screen = self.0.lock().unwrap().clone();
        let (application, button) = match target {
            AppTarget::Name(name) if name == "Missing" => {
                return Err(AxError::ApplicationNotFound(name.clone()));
            }
            AppTarget::Name(name) => (name.clone(), "Other".to_owned()),
            _ => (screen.application, screen.button),
        };
        let frame = |x, y, width, height| Some(AxFrame { x, y, width, height });
        Ok(AxSnapshot {
            application: Application { name: Some(application), ..Application::default() },
            window: AxNode {
                role: "AXWindow".to_owned(),
                title: Some("Demo".to_owned()),
                frame: frame(0.0, 0.0, 300.0, 200.0),
                children: vec![AxNode {
                    role: "AXButton".to_owned(),
                    title: Some(button),
                    frame: frame(10.0, 10.0, 80.0, 24.0),
                    ..AxNode::default()
                }],
                ..AxNode::default()
            },
            truncated: false,
        })
    }
}

fn start() -> (SocketAddr, Arc<Mutex<Screen>>) {
    let screen =
        Arc::new(Mutex::new(Screen { application: "Demo".to_owned(), button: "Save".to_owned() }));
    let shared = Arc::clone(&screen);
    let server = Server::bind(
        &Config { port: 0, history: 8 },
        Box::new(move || {
            Ok(Observer::default().with_accessibility(Box::new(Scripted(Arc::clone(&shared)))))
        }),
    )
    .unwrap();
    let address = server.local_addr();
    std::thread::spawn(move || server.run());
    (address, screen)
}

struct Answer {
    status: u16,
    head: String,
    body: Value,
}

fn request(address: SocketAddr, head: &str) -> Answer {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.write_all(head.as_bytes()).unwrap();
    let mut text = String::new();
    stream.read_to_string(&mut text).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    Answer { status, head: head.to_owned(), body: serde_json::from_str(body).unwrap() }
}

fn get(address: SocketAddr, path: &str) -> Answer {
    request(address, &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n", address.port()))
}

fn code(answer: &Answer) -> &str {
    answer.body["error"]["code"].as_str().unwrap_or_default()
}

#[test]
fn serves_observations_and_changes() {
    let (address, screen) = start();

    let health = get(address, "/v1/health");
    assert_eq!(health.status, 200);
    assert_eq!(health.body["status"], "ok");
    assert_eq!(health.body["observations"], 0);

    let answer = get(address, "/v1/observation?sources=accessibility");
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert!(answer.head.contains("Server-Timing: accessibility;dur="), "{}", answer.head);
    assert!(answer.head.contains("Content-Type: application/json"));
    let first: Observation = serde_json::from_value(answer.body).unwrap();
    first.validate().unwrap();
    assert_eq!(first.elements[1].name.as_deref(), Some("Save"));

    // Stored observations are served as they were.
    let stored = get(address, &format!("/v1/observation/{}", first.id));
    assert_eq!(serde_json::from_value::<Observation>(stored.body).unwrap(), first);

    // Another application in between has its own session.
    let other = get(address, "/v1/observation?app=Other&sources=accessibility");
    assert_eq!(other.status, 200);

    screen.lock().unwrap().button = "Saved".to_owned();
    let middle = get(address, "/v1/observation?sources=accessibility");
    let middle: Observation = serde_json::from_value(middle.body).unwrap();
    assert_eq!(middle.previous.as_ref(), Some(&first.id));

    // Changes since the first observation, through the middle one.
    let answer = get(address, &format!("/v1/changes?since={}", first.id));
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert!(answer.head.contains("Server-Timing: "));
    let delta: ObservationDelta = serde_json::from_value(answer.body).unwrap();
    assert_eq!(delta.from, first.id);
    let renamed: Vec<_> = delta.changed.iter().filter(|c| c.property == "name").collect();
    assert_eq!(renamed.len(), 1, "{delta:?}");
    assert_eq!((renamed[0].id.as_str(), &renamed[0].to), ("e_2", &Value::from("Saved")));
    assert!(delta.added.is_empty() && delta.removed.is_empty(), "{delta:?}");
    let last = get(address, &format!("/v1/observation/{}", delta.to));
    let last: Observation = serde_json::from_value(last.body).unwrap();
    assert_eq!(last.previous.as_ref(), Some(&middle.id));
    assert_eq!(delta.apply(&first).unwrap().elements, last.elements);

    // Nothing changed since the last one.
    let answer = get(address, &format!("/v1/changes?since={}", last.id));
    let delta: ObservationDelta = serde_json::from_value(answer.body).unwrap();
    assert!(delta.is_empty(), "{delta:?}");

    // Elements of the latest observation of any session, or of a given one.
    let button = &last.elements[1].id;
    let answer = get(address, &format!("/v1/elements/{button}?observation={}", last.id));
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert_eq!(answer.body["observation"], last.id.as_str());
    assert_eq!(answer.body["element"]["name"], "Saved");
    assert_eq!(answer.body["evidence"]["contributions"][0]["source"], "accessibility");
    let missing = get(address, &format!("/v1/elements/e_999?observation={}", last.id));
    assert_eq!((missing.status, code(&missing)), (404, "element_not_found"));
    assert_eq!(get(address, "/v1/health").body["observations"], 5);
}

#[test]
fn a_new_session_is_not_a_change() {
    let (address, screen) = start();
    let first = get(address, "/v1/observation?sources=accessibility");
    let first: Observation = serde_json::from_value(first.body).unwrap();

    // Another application comes to the front.
    screen.lock().unwrap().application = "Else".to_owned();
    let answer = get(address, &format!("/v1/changes?since={}", first.id));
    assert_eq!((answer.status, code(&answer)), (409, "session_changed"));
    let id = answer.body["error"]["observation"].as_str().unwrap().to_owned();
    let fresh = get(address, &format!("/v1/observation/{id}"));
    let fresh: Observation = serde_json::from_value(fresh.body).unwrap();
    assert_eq!(fresh.previous, None);
    assert_eq!(fresh.application.unwrap().name.as_deref(), Some("Else"));
}

#[test]
fn refuses_what_it_cannot_serve() {
    let (address, _) = start();
    let cases = [
        ("/v1/nothing", 404, "not_found"),
        ("/v1/observation?application=Calculator", 400, "bad_request"),
        ("/v1/observation?app=A&pid=1", 400, "bad_request"),
        ("/v1/observation?pid=abc", 400, "bad_request"),
        ("/v1/observation?sources=telepathy", 400, "bad_request"),
        ("/v1/observation?app=Missing&sources=accessibility", 404, "not_found"),
        // No capture backend: pixel sources are not supported here.
        ("/v1/observation", 501, "unsupported"),
        ("/v1/observation/obs_nope", 404, "observation_not_found"),
        ("/v1/changes", 400, "bad_request"),
        ("/v1/changes?since=obs_nope", 404, "observation_not_found"),
        ("/v1/elements/e_1", 404, "observation_not_found"),
    ];
    for (path, status, expected) in cases {
        let answer = get(address, path);
        assert_eq!((answer.status, code(&answer)), (status, expected), "{path}");
        assert!(answer.body["error"]["message"].is_string(), "{path}");
    }

    let port = address.port();
    let post =
        request(address, &format!("POST /v1/health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"));
    assert_eq!(post.status, 405);
    assert!(post.head.contains("Allow: GET"));
    // DNS rebinding and web pages.
    let rebound = request(address, "GET /v1/health HTTP/1.1\r\nHost: evil.example:7412\r\n\r\n");
    assert_eq!((rebound.status, code(&rebound)), (403, "forbidden"));
    let page = request(
        address,
        &format!(
            "GET /v1/health HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: https://evil.example\r\n\r\n"
        ),
    );
    assert_eq!((page.status, code(&page)), (403, "forbidden"));
    let garbage = request(address, "HELLO\r\n\r\n");
    assert_eq!(garbage.status, 400);
}

#[test]
fn fails_to_start_without_an_observer() {
    let result = Server::bind(
        &Config { port: 0, history: 1 },
        Box::new(|| Err(argus_core::Error::Unsupported { feature: "testing" })),
    );
    assert_eq!(result.unwrap_err().code(), "unsupported");
}
