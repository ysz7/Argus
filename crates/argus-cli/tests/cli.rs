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
