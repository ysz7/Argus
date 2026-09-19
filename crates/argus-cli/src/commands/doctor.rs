//! `argus doctor`: diagnoses the installation.
//!
//! Every check exercises the real path it names: permissions are read from
//! the system, the capture backend captures the main display (the frame is
//! discarded), OCR and visual detection run on built-in sample images, and
//! the local service is asked for its health.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::Command;
use std::time::{Duration, Instant};

use argus_core::accessibility::{AccessibilityBackend, AppTarget, AxNode};
use argus_core::capture::{CaptureBackend, CaptureTarget};
use argus_core::perception::{HeuristicDetector, OcrBackend, VisualPerceptionBackend};
use argus_protocol::{Bounds, Frame, FrameId, PixelBuffer, Role, Timestamp};
use serde::Serialize;

use crate::output::{print_json, print_text};

/// A rendered dialog with known text (`tests/fixtures/ocr`).
const TEXT_SAMPLE: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/ocr/dialog.png"));
/// Lines of [`TEXT_SAMPLE`] that recognition must find.
const TEXT_SAMPLE_LINES: &[&str] = &["Delete file?", "This action cannot be undone."];
/// Real AppKit controls (`tests/fixtures/vision`).
const CONTROLS_SAMPLE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/vision/controls_light.png"
));
/// Oldest supported macOS (ScreenCaptureKit screenshots).
const MIN_MACOS: u32 = 14;
/// A window tree smaller than this is a skeleton.
const SPARSE_TREE: usize = 20;
/// Recognition slower than this is reported as a model preparation.
const SLOW_OCR: Duration = Duration::from_secs(5);

#[derive(Debug, clap::Args)]
pub(crate) struct DoctorArgs {
    /// Print the results as JSON.
    #[arg(long)]
    json: bool,

    /// Port of the local service to check.
    #[arg(long, env = "ARGUS_PORT", default_value_t = argus_server::DEFAULT_PORT)]
    port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Ok,
    /// Not a problem, worth knowing.
    Info,
    /// Works, but something will likely get in the way.
    Warning,
    Failed,
    /// Not checked because an earlier check failed.
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Check {
    name: &'static str,
    status: Status,
    detail: String,
    /// What to do about it.
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<String>,
}

impl Check {
    fn new(name: &'static str, status: Status, detail: impl Into<String>) -> Self {
        Self { name, status, detail: detail.into(), hint: None }
    }

    fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// What the checks need to know about the machine.
#[derive(Debug, Clone, Default)]
pub(crate) struct Environment {
    /// macOS product version, e.g. `15.1`.
    pub(crate) macos: Option<String>,
    /// The application macOS grants permissions to (the one that started
    /// `argus`), e.g. `Terminal`.
    pub(crate) host: Option<String>,
    /// Other running `argus` processes.
    pub(crate) other_processes: Vec<u32>,
    /// The local service port.
    pub(crate) port: u16,
    pub(crate) service: Service,
}

/// What answers on the service port.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum Service {
    #[default]
    NotRunning,
    Argus {
        version: String,
    },
    /// Another program uses the port.
    Other,
}

/// The backends to check; `None` if the platform has none.
pub(crate) struct Backends<'a> {
    pub(crate) capture: Option<&'a dyn CaptureBackend>,
    pub(crate) accessibility: Option<&'a dyn AccessibilityBackend>,
    pub(crate) ocr: Option<&'a dyn OcrBackend>,
    pub(crate) vision: &'a dyn VisualPerceptionBackend,
}

pub(crate) fn run(args: &DoctorArgs) -> anyhow::Result<()> {
    let environment = Environment {
        macos: macos_version(),
        host: host_application(),
        other_processes: other_argus_processes(),
        port: args.port,
        service: probe_service(args.port),
    };
    let capture = argus_core::capture::default_backend().ok();
    let accessibility = argus_core::accessibility::default_backend().ok();
    let ocr = argus_core::perception::default_ocr_backend().ok();
    let vision = HeuristicDetector::default();
    let checks = diagnose(
        &environment,
        &Backends {
            capture: capture.as_deref(),
            accessibility: accessibility.as_deref(),
            ocr: ocr.as_deref(),
            vision: &vision,
        },
    );
    let failed = checks.iter().filter(|check| check.status == Status::Failed).count();
    if args.json {
        print_json(&serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "checks": checks,
            "ok": failed == 0,
        }))?;
    } else {
        print_text(&render(&checks))?;
    }
    if failed > 0 {
        anyhow::bail!("{failed} check(s) failed");
    }
    Ok(())
}

/// Runs every check in order.
pub(crate) fn diagnose(environment: &Environment, backends: &Backends<'_>) -> Vec<Check> {
    let host = environment.host.as_deref().unwrap_or("the application that runs argus");
    let mut checks = vec![check_macos(environment.macos.as_deref())];

    let settings = |pane: &str| {
        format!(
            "allow \"{host}\" in System Settings → Privacy & Security → {pane}, then quit and \
             reopen \"{host}\""
        )
    };
    let screen = backends.capture.map(|capture| capture.has_permission());
    checks.push(match screen {
        None => Check::new("Screen recording", Status::Failed, "no capture backend"),
        Some(true) => Check::new("Screen recording", Status::Ok, format!("granted to {host}")),
        Some(false) => Check::new("Screen recording", Status::Failed, "not granted")
            .hint(settings("Screen & System Audio Recording")),
    });
    let reading = backends.accessibility.map(|backend| backend.has_permission());
    checks.push(match reading {
        None => Check::new("Accessibility", Status::Failed, "no accessibility backend"),
        Some(true) => Check::new("Accessibility", Status::Ok, format!("granted to {host}")),
        Some(false) => Check::new("Accessibility", Status::Failed, "not granted")
            .hint(settings("Accessibility")),
    });

    checks.push(match backends.capture {
        Some(capture) if screen == Some(true) => check_capture(capture, environment),
        _ => Check::new("Capture backend", Status::Skipped, "needs screen recording"),
    });
    checks.push(match backends.accessibility {
        Some(backend) if reading == Some(true) => check_tree(backend),
        _ => Check::new("Accessibility tree", Status::Skipped, "needs accessibility"),
    });
    checks.push(match backends.ocr {
        Some(ocr) => check_ocr(ocr),
        None => Check::new("OCR backend", Status::Failed, "not available on this platform"),
    });
    checks.push(check_vision(backends.vision));
    checks.push(check_service(environment));
    checks
}

fn check_macos(version: Option<&str>) -> Check {
    const NAME: &str = "macOS";
    let Some(version) = version else {
        return Check::new(NAME, Status::Failed, "not macOS").hint("Argus runs on macOS only");
    };
    match version.split('.').next().and_then(|major| major.parse::<u32>().ok()) {
        Some(major) if major >= MIN_MACOS => Check::new(NAME, Status::Ok, version),
        Some(_) => Check::new(NAME, Status::Failed, version)
            .hint(format!("Argus needs macOS {MIN_MACOS} or later")),
        None => Check::new(NAME, Status::Warning, format!("unrecognized version `{version}`")),
    }
}

fn check_capture(capture: &dyn CaptureBackend, environment: &Environment) -> Check {
    const NAME: &str = "Capture backend";
    let started = Instant::now();
    match capture.capture(CaptureTarget::MainDisplay) {
        Ok(frame) => {
            let bounds = frame.bounds();
            Check::new(
                NAME,
                Status::Ok,
                format!(
                    "main display {}×{} pt @{}x in {} ms (not saved)",
                    bounds.width(),
                    bounds.height(),
                    frame.scale_factor(),
                    started.elapsed().as_millis()
                ),
            )
        }
        // The running service holds the capture: expected, not broken.
        Err(argus_core::capture::Error::Timeout(_))
            if matches!(environment.service, Service::Argus { .. }) =>
        {
            Check::new(NAME, Status::Warning, "blocked by the running argus service").hint(
                "the service captures for all clients; stop it to capture from other argus \
                 commands",
            )
        }
        Err(argus_core::capture::Error::Timeout(_)) => {
            let check = Check::new(NAME, Status::Failed, "the screenshot did not arrive");
            match environment.other_processes.as_slice() {
                [] => check.hint("retry; if it persists, restart the computer"),
                pids => check.hint(format!(
                    "another argus process is running (pid {}); macOS lets only one running \
                     process of a program capture — stop it, or use `argus serve` for all \
                     clients",
                    pids.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
                )),
            }
        }
        Err(error) => Check::new(NAME, Status::Failed, error.to_string()),
    }
}

fn check_tree(backend: &dyn AccessibilityBackend) -> Check {
    const NAME: &str = "Accessibility tree";
    let started = Instant::now();
    match backend.snapshot(&AppTarget::Frontmost) {
        Ok(snapshot) => {
            let application = snapshot.application.name.as_deref().unwrap_or("frontmost app");
            let detail = format!(
                "{application}: {} nodes in {} ms",
                count(&snapshot.window),
                started.elapsed().as_millis()
            );
            match (snapshot.truncated, count(&snapshot.window)) {
                (true, _) => Check::new(NAME, Status::Warning, format!("{detail} (truncated)"))
                    .hint("very large windows are observed partially"),
                (false, nodes) if nodes < SPARSE_TREE => Check::new(NAME, Status::Info, detail)
                    .hint(format!(
                        "{application} exposes little to accessibility (Electron and \
                         Chromium apps do); Argus relies on its pixels"
                    )),
                (false, _) => Check::new(NAME, Status::Ok, detail),
            }
        }
        // Permission is granted: the frontmost application is the problem,
        // not the installation.
        Err(error) => Check::new(NAME, Status::Warning, error.to_string())
            .hint("the frontmost application could not be read; try `argus observe --app Finder`"),
    }
}

fn count(node: &AxNode) -> usize {
    1 + node.children.iter().map(count).sum::<usize>()
}

fn check_ocr(ocr: &dyn OcrBackend) -> Check {
    const NAME: &str = "OCR backend";
    let frame = match sample(TEXT_SAMPLE) {
        Ok(frame) => frame,
        Err(error) => return Check::new(NAME, Status::Failed, error.to_string()),
    };
    let started = Instant::now();
    let lines = match ocr.detect(&frame) {
        Ok(lines) => lines,
        Err(error) => return Check::new(NAME, Status::Failed, error.to_string()),
    };
    let elapsed = started.elapsed();
    let found = TEXT_SAMPLE_LINES
        .iter()
        .filter(|expected| lines.iter().any(|line| line.text.trim() == **expected))
        .count();
    let detail = format!(
        "{found}/{} sample lines recognized in {} ms",
        TEXT_SAMPLE_LINES.len(),
        elapsed.as_millis()
    );
    if found < TEXT_SAMPLE_LINES.len() {
        return Check::new(NAME, Status::Failed, detail)
            .hint("text recognition does not work; check the macOS installation");
    }
    match elapsed > SLOW_OCR {
        true => Check::new(NAME, Status::Info, detail).hint(
            "macOS prepared its text recognition models (once per new argus binary); later \
             runs are fast",
        ),
        false => Check::new(NAME, Status::Ok, detail),
    }
}

fn check_vision(vision: &dyn VisualPerceptionBackend) -> Check {
    const NAME: &str = "Vision backend";
    let result = sample(CONTROLS_SAMPLE).map_err(|error| error.to_string()).and_then(|frame| {
        let started = Instant::now();
        let found = vision.detect(&frame).map_err(|error| error.to_string())?;
        Ok((found, started.elapsed()))
    });
    match result {
        Ok((found, elapsed)) => {
            let of = |role| found.iter().filter(|candidate| candidate.role == role).count();
            let detail = format!(
                "{} buttons, {} checkboxes in the sample in {} ms",
                of(Role::Button),
                of(Role::Checkbox),
                elapsed.as_millis()
            );
            match of(Role::Button) > 0 && of(Role::Checkbox) > 0 {
                true => Check::new(NAME, Status::Ok, detail),
                false => Check::new(NAME, Status::Failed, detail),
            }
        }
        Err(error) => Check::new(NAME, Status::Failed, error),
    }
}

fn check_service(environment: &Environment) -> Check {
    const NAME: &str = "Local server";
    let port = environment.port;
    match &environment.service {
        Service::Argus { version } => Check::new(
            NAME,
            Status::Ok,
            format!("argus {version} running on http://127.0.0.1:{port}"),
        )
        .hint(
            "while it runs, other argus commands cannot capture the screen (macOS allows one \
             capturing process of a program); ask the service instead",
        ),
        Service::NotRunning => {
            Check::new(NAME, Status::Info, format!("not running on port {port}"))
                .hint("start it with `argus serve`")
        }
        Service::Other => {
            Check::new(NAME, Status::Warning, format!("port {port} is used by another program"))
                .hint("run the service on another port: `argus serve --port <N>`")
        }
    }
}

/// Decodes a built-in RGBA sample rendered at 2x.
fn sample(png: &[u8]) -> anyhow::Result<Frame> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png)).read_info()?;
    let mut data = vec![0; reader.output_buffer_size().unwrap_or_default()];
    let info = reader.next_frame(&mut data)?;
    data.truncate(info.buffer_size());
    let bounds = Bounds::new(0.0, 0.0, info.width as f32 / 2.0, info.height as f32 / 2.0)?;
    let pixels = PixelBuffer::new(info.width, info.height, data)?;
    Ok(Frame::new(FrameId(0), Timestamp::now(), bounds, 2.0, pixels)?)
}

pub(crate) fn render(checks: &[Check]) -> String {
    let mut out = format!("Argus Doctor (argus {})\n\n", env!("CARGO_PKG_VERSION"));
    for check in checks {
        let status = match check.status {
            Status::Ok => "OK",
            Status::Info => "INFO",
            Status::Warning => "WARNING",
            Status::Failed => "FAILED",
            Status::Skipped => "SKIPPED",
        };
        out.push_str(&format!("{:<20} {:<8} {}\n", check.name, status, check.detail));
        if let Some(hint) = &check.hint {
            if check.status != Status::Ok || check.name == "Local server" {
                out.push_str(&format!("{:<20} → {hint}\n", ""));
            }
        }
    }
    let failed = checks.iter().filter(|check| check.status == Status::Failed).count();
    let warnings = checks.iter().filter(|check| check.status == Status::Warning).count();
    out.push('\n');
    out.push_str(&match (failed, warnings) {
        (0, 0) => "Everything is ready.\n".to_owned(),
        (0, warnings) => format!("Ready, with {warnings} warning(s).\n"),
        (failed, _) => format!("{failed} problem(s) found.\n"),
    });
    out
}

/// The macOS product version, if this is macOS.
fn macos_version() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let output = Command::new("/usr/bin/sw_vers").arg("-productVersion").output().ok()?;
    let version = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!version.is_empty()).then_some(version)
}

/// Executable path and parent of process `pid`.
fn process(pid: u32) -> Option<(u32, String)> {
    let output = Command::new("/bin/ps")
        .args(["-o", "ppid=,comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let line = String::from_utf8(output.stdout).ok()?;
    let (parent, path) = line.trim().split_once(' ')?;
    Some((parent.trim().parse().ok()?, path.trim().to_owned()))
}

/// The application bundle among the ancestors of this process: macOS
/// attributes privacy permissions to it.
fn host_application() -> Option<String> {
    let mut pid = std::process::id();
    for _ in 0..32 {
        let (parent, path) = process(pid)?;
        if let Some(name) = app_name(&path) {
            return Some(name);
        }
        if parent <= 1 {
            break;
        }
        pid = parent;
    }
    None
}

/// The outermost application bundle in an executable path:
/// `/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper.app/...`
/// → `Visual Studio Code`.
fn app_name(path: &str) -> Option<String> {
    let end = path.find(".app/")?;
    let start = path[..end].rfind('/').map_or(0, |slash| slash + 1);
    Some(path[start..end].to_owned())
}

/// Other running processes of an `argus` executable.
pub(crate) fn other_argus_processes() -> Vec<u32> {
    let Ok(output) = Command::new("/bin/ps").args(["-axo", "pid=,comm="]).output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (pid, path) = line.trim().split_once(' ')?;
            let pid: u32 = pid.parse().ok()?;
            let executable = path.trim().rsplit('/').next()?;
            (executable == "argus" && pid != std::process::id()).then_some(pid)
        })
        .collect()
}

/// Asks the local service on `port` for its health.
pub(crate) fn probe_service(port: u16) -> Service {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(300)) else {
        return Service::NotRunning;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let request = format!("GET /v1/health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    let mut response = String::new();
    if stream.write_all(request.as_bytes()).is_err()
        || stream.read_to_string(&mut response).is_err()
    {
        return Service::Other;
    }
    let body = response.split_once("\r\n\r\n").map_or("", |(_, body)| body);
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(health) if health["status"] == "ok" => {
            Service::Argus { version: health["version"].as_str().unwrap_or("?").to_owned() }
        }
        _ => Service::Other,
    }
}

#[cfg(test)]
mod tests {
    use argus_core::accessibility::{AxSnapshot, Error as AxError};
    use argus_core::capture::{DisplayInfo, Error as CaptureError, WindowInfo};
    use argus_core::perception::OcrRegion;
    use argus_protocol::{Application, PixelRect, Score};

    use super::*;

    struct FakeCapture {
        permission: bool,
        timeout: bool,
    }

    impl CaptureBackend for FakeCapture {
        fn has_permission(&self) -> bool {
            self.permission
        }
        fn displays(&self) -> argus_core::capture::Result<Vec<DisplayInfo>> {
            Ok(Vec::new())
        }
        fn windows(&self) -> argus_core::capture::Result<Vec<WindowInfo>> {
            Ok(Vec::new())
        }
        fn frontmost_application(&self) -> argus_core::capture::Result<Option<Application>> {
            Ok(None)
        }
        fn capture(&self, _target: CaptureTarget) -> argus_core::capture::Result<Frame> {
            if self.timeout {
                return Err(CaptureError::Timeout("screenshot"));
            }
            let bounds = Bounds::new(0.0, 0.0, 10.0, 5.0).unwrap();
            let pixels = PixelBuffer::new(20, 10, vec![0; 800]).unwrap();
            Ok(Frame::new(FrameId(1), Timestamp(0), bounds, 2.0, pixels).unwrap())
        }
    }

    struct FakeAccessibility(bool);

    impl AccessibilityBackend for FakeAccessibility {
        fn has_permission(&self) -> bool {
            self.0
        }
        fn snapshot(&self, _target: &AppTarget) -> argus_core::accessibility::Result<AxSnapshot> {
            Err(AxError::NoWindow)
        }
    }

    /// Recognizes the sample's lines.
    struct FakeOcr;

    impl OcrBackend for FakeOcr {
        fn detect(&self, _frame: &Frame) -> argus_core::perception::Result<Vec<OcrRegion>> {
            Ok(TEXT_SAMPLE_LINES
                .iter()
                .map(|text| OcrRegion {
                    text: (*text).to_owned(),
                    rect: PixelRect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 },
                    confidence: Score::new(1.0).unwrap(),
                })
                .collect())
        }
    }

    fn statuses(checks: &[Check]) -> Vec<(&str, Status)> {
        checks.iter().map(|check| (check.name, check.status)).collect()
    }

    #[test]
    fn a_ready_machine_passes() {
        let environment = Environment {
            macos: Some("15.1".to_owned()),
            host: Some("Terminal".to_owned()),
            port: 7412,
            ..Environment::default()
        };
        let capture = FakeCapture { permission: true, timeout: false };
        let vision = HeuristicDetector::default();
        let checks = diagnose(
            &environment,
            &Backends {
                capture: Some(&capture),
                accessibility: Some(&FakeAccessibility(true)),
                ocr: Some(&FakeOcr),
                vision: &vision,
            },
        );
        assert_eq!(
            statuses(&checks),
            [
                ("macOS", Status::Ok),
                ("Screen recording", Status::Ok),
                ("Accessibility", Status::Ok),
                ("Capture backend", Status::Ok),
                // No window in front: not an installation problem.
                ("Accessibility tree", Status::Warning),
                ("OCR backend", Status::Ok),
                ("Vision backend", Status::Ok),
                ("Local server", Status::Info),
            ]
        );
        assert_eq!(checks[1].detail, "granted to Terminal");
        let text = render(&checks);
        assert!(text.contains("Ready, with 1 warning(s)."), "{text}");
        assert!(text.contains("start it with `argus serve`"), "{text}");
    }

    #[test]
    fn missing_permissions_say_where_to_grant_them() {
        let environment = Environment {
            macos: Some("13.6".to_owned()),
            host: Some("Visual Studio Code".to_owned()),
            other_processes: vec![42],
            port: 7412,
            service: Service::Other,
        };
        let capture = FakeCapture { permission: false, timeout: false };
        let vision = HeuristicDetector::default();
        let checks = diagnose(
            &environment,
            &Backends {
                capture: Some(&capture),
                accessibility: Some(&FakeAccessibility(false)),
                ocr: None,
                vision: &vision,
            },
        );
        assert_eq!(
            statuses(&checks),
            [
                ("macOS", Status::Failed),
                ("Screen recording", Status::Failed),
                ("Accessibility", Status::Failed),
                ("Capture backend", Status::Skipped),
                ("Accessibility tree", Status::Skipped),
                ("OCR backend", Status::Failed),
                ("Vision backend", Status::Ok),
                ("Local server", Status::Warning),
            ]
        );
        let hint = checks[1].hint.as_deref().unwrap();
        assert!(
            hint.contains("\"Visual Studio Code\"")
                && hint.contains("Screen & System Audio Recording")
        );
        assert!(render(&checks).contains("4 problem(s) found."));
    }

    #[test]
    fn a_blocked_capture_names_the_other_process() {
        let environment = Environment { other_processes: vec![4242], ..Environment::default() };
        let check = check_capture(&FakeCapture { permission: true, timeout: true }, &environment);
        assert_eq!(check.status, Status::Failed);
        assert!(check.hint.unwrap().contains("pid 4242"));
    }

    #[test]
    fn a_capture_held_by_the_service_is_expected() {
        let environment = Environment {
            other_processes: vec![4242],
            service: Service::Argus { version: "0.0.1".to_owned() },
            ..Environment::default()
        };
        let check = check_capture(&FakeCapture { permission: true, timeout: true }, &environment);
        assert_eq!(check.status, Status::Warning);
    }

    #[test]
    fn host_applications_are_the_outermost_bundle() {
        assert_eq!(
            app_name(
                "/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper.app/Contents/MacOS/Code Helper"
            )
            .as_deref(),
            Some("Visual Studio Code")
        );
        assert_eq!(
            app_name("/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal")
                .as_deref(),
            Some("Terminal")
        );
        assert_eq!(app_name("/bin/zsh"), None);
    }

    #[test]
    fn samples_decode() {
        let text = sample(TEXT_SAMPLE).unwrap();
        assert_eq!((text.width(), text.height()), (960, 480));
        let controls = sample(CONTROLS_SAMPLE).unwrap();
        assert_eq!((controls.width(), controls.height()), (1040, 720));
    }

    #[test]
    fn an_unused_port_is_not_a_service() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        assert_eq!(probe_service(port), Service::NotRunning);
    }
}
