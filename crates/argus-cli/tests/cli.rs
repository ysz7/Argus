//! End-to-end tests of the `argus` binary.

use assert_cmd::Command;
use predicates::str::contains;

fn argus() -> Command {
    let mut cmd = Command::cargo_bin("argus").unwrap();
    cmd.env_remove("ARGUS_LOG");
    cmd
}

#[test]
fn prints_version() {
    argus()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains(concat!("argus ", env!("CARGO_PKG_VERSION"))));
}

#[test]
fn prints_help_without_arguments() {
    argus().assert().failure().stderr(contains("Usage: argus"));
}

#[test]
fn rejects_invalid_log_filter() {
    argus().args(["--log-level", "=="]).assert().failure().stderr(contains("invalid log filter"));
}

#[test]
fn json_logs_go_to_stderr_only() {
    let output = argus().args(["--log-level", "debug", "--log-format", "json"]).output().unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty(), "stdout must stay clean");

    let stderr = String::from_utf8(output.stderr).unwrap();
    let line = stderr.lines().next().expect("expected a log line");
    assert!(line.starts_with('{') && line.contains("\"argus started\""), "{line}");
}

#[test]
fn capture_rejects_conflicting_targets() {
    argus()
        .args(["capture", "--display", "1", "--window", "2"])
        .assert()
        .failure()
        .stderr(contains("cannot be used with"));
}

#[test]
fn capture_list_cannot_write_output() {
    argus()
        .args(["capture", "--list", "--output", "frame.png"])
        .assert()
        .failure()
        .stderr(contains("cannot be used with"));
}

#[test]
fn observe_rejects_conflicting_targets() {
    argus()
        .args(["observe", "--app", "Calculator", "--pid", "1"])
        .assert()
        .failure()
        .stderr(contains("cannot be used with"));
}

#[test]
fn observe_rejects_unknown_sources() {
    argus().args(["observe", "--source", "telepathy"]).assert().failure();
}

#[test]
fn capture_overlay_requires_output() {
    argus().args(["capture", "--overlay"]).assert().failure().stderr(contains("--output"));
}

#[test]
fn observe_accepts_source_lists() {
    // Parsing only: an unknown member of the list is rejected before observing.
    argus()
        .args(["observe", "--sources", "accessibility,telepathy"])
        .assert()
        .failure()
        .stderr(contains("telepathy"));
}

#[test]
fn inspect_is_documented() {
    argus()
        .args(["inspect", "--help"])
        .assert()
        .success()
        .stdout(contains("ELEMENT"))
        .stdout(contains("--sources"));
}

#[test]
fn observe_rejects_zero_observations() {
    argus().args(["observe", "--count", "0"]).assert().failure().stderr(contains("--count"));
}

#[test]
fn watch_rejects_zero_observations() {
    argus().args(["watch", "--count", "0"]).assert().failure().stderr(contains("--count"));
}

/// A running `argus serve`, stopped when dropped.
struct Service(std::process::Child);

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts `argus serve` on a free port and returns its address.
fn serve(extra: &[&str]) -> (Service, String) {
    use std::io::BufRead;

    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("argus"))
        .args(["serve", "--port", "0"])
        .args(extra)
        .env_remove("ARGUS_LOG")
        .env_remove("ARGUS_PORT")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let service = Service(child);
    let mut line = String::new();
    std::io::BufReader::new(stdout).read_line(&mut line).unwrap();
    let address = line.trim().strip_prefix("listening on http://").expect(&line).to_owned();
    (service, address)
}

fn http_get(address: &str, path: &str) -> String {
    use std::io::{Read, Write};

    let mut stream = std::net::TcpStream::connect(address).unwrap();
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {address}\r\n\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn serve_answers_health_checks() {
    let (_service, address) = serve(&[]);
    assert!(address.starts_with("127.0.0.1:"), "{address}");
    let response = http_get(&address, "/v1/health");
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains(r#""status":"ok""#), "{response}");
    let response = http_get(&address, "/v1/observation/obs_unknown");
    assert!(response.starts_with("HTTP/1.1 404"), "{response}");
    assert!(response.contains("observation_not_found"), "{response}");
}

#[test]
fn serve_reports_a_taken_port() {
    let (_service, address) = serve(&[]);
    let port = address.rsplit_once(':').unwrap().1;
    argus()
        .args(["serve", "--port", port])
        .assert()
        .failure()
        .stderr(contains(format!("cannot listen on port {port}")));
}

#[test]
fn serve_rejects_an_empty_history() {
    argus().args(["serve", "--history", "0"]).assert().failure().stderr(contains("--history"));
}
