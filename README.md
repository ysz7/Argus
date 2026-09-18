# Argus

**Argus** is a local perception layer for computer-use systems. It turns what
is on the screen into a structured, grounded, confidence-aware observation that
an external AI agent can consume.

```text
screen / native UI
        ↓
      Argus
        ↓
structured observation
        ↓
 external AI agent
```

Argus answers one question:

> What is in the interface right now, where is it, what state is it in, and
> how confident is Argus about each claim?

> **Status:** early development. The [Observation Protocol v0.1](spec/ARGUS_PROTOCOL.md)
> is defined; macOS screen capture and Accessibility-based observations work.
> Pixel-based perception (OCR, vision) is not implemented yet.

## Architectural boundaries

Argus is **not** an AI agent and not a single AI model.

```text
ARGUS             pixels / native structure → meaning
AGENT             meaning → intention
EXECUTION LAYER   intention → physical action
```

Argus never:

- plans tasks or decides what to click;
- controls the mouse or keyboard;
- makes decisions on behalf of an agent or evaluates user permissions;
- performs irreversible actions;
- replaces application APIs;
- becomes an LLM agent.

Screen content is untrusted input: text on screen is reported as UI content,
never interpreted as instructions to Argus.

### Privacy by default

- all processing is local; frames are never uploaded anywhere;
- screenshots are not persisted (debug captures only on explicit request);
- no telemetry containing screen content;
- the local service listens on localhost only.

## Workspace layout

| Crate                 | Responsibility                                          |
| --------------------- | ------------------------------------------------------- |
| `argus-protocol`      | Public data structures and serialization contract.      |
| `argus-capture`       | Frame acquisition (screens, windows).                   |
| `argus-accessibility` | Native accessibility adapters (macOS first).            |
| `argus-perception`    | OCR and visual perception backends.                     |
| `argus-fusion`        | Merging evidence from different sources.                |
| `argus-tracking`      | Element identity and changes over time.                 |
| `argus-core`          | Orchestration pipeline; aggregated error type.          |
| `argus-server`        | Local service / API.                                    |
| `argus-cli`           | The `argus` binary: user and developer commands.        |

### Pipeline

```text
accessibility ─┐
OCR           ─┼─▶ SourceCandidate ─▶ normalize ─▶ Normalized ─▶ assemble ─▶ Observation
vision        ─┘
```

(OCR and vision are planned.)

Sources map their native vocabulary (e.g. `AXButton`) to protocol roles and
report evidence as `SourceCandidate`s. Normalization (`argus-core`) converts
coordinates to global points, derives visibility, cleans text and enforces
consistency of roles, states and confidence. Observations can only be
assembled from normalized candidates.

Allowed internal dependencies (enforced by
[`tests/integration/tests/architecture.rs`](tests/integration/tests/architecture.rs)):

```text
argus-cli ──→ argus-server ──→ argus-core ──→ capture, accessibility,
    │                              │          perception, fusion, tracking
    └──────────────────────────────┴─────────────────→ argus-protocol
```

- `argus-protocol` depends on no other Argus crate and contains no platform
  code.
- Source and processing crates (`capture`, `accessibility`, `perception`,
  `fusion`, `tracking`) depend only on `argus-protocol` and never on each
  other; `argus-core` wires them together.
- Nothing depends on `argus-server` or `argus-cli` except the CLI itself.

Other directories (some appear in later phases): `spec/` (protocol specification), `tests/` (fixtures,
golden files, integration tests), `benchmarks/`, `examples/`, `models/`,
`research/` (offline, non-runtime).

## Building

Requirements: macOS 14 or later, stable Rust (see `rust-toolchain.toml`).

```bash
cargo build
cargo test
cargo fmt --check
cargo clippy
```

Run the CLI:

```bash
cargo run -p argus-cli -- --help
```

### Permissions

macOS grants permissions to the application that launches `argus` (Terminal,
iTerm, VS Code, ...). Allow that application in System Settings → Privacy &
Security, then restart it:

- **Accessibility** — needed by `argus observe` and `argus accessibility`;
- **Screen & System Audio Recording** — needed by `argus capture`.

The first attempt triggers the system prompt.

### Observe

```bash
argus observe                          # focused window of the frontmost app
argus observe --app Calculator         # by name or bundle id, works in background
argus observe --pid 4321
argus observe --source accessibility   # the only source so far (default)
```

Prints an [Argus Observation](spec/ARGUS_PROTOCOL.md) as JSON.
`argus accessibility` prints the raw native tree (`AXButton`, ...) for
debugging the platform adapter.

Electron/Chromium applications (VS Code, Slack, ...) expose only a skeleton
tree unless asked to enable accessibility; Argus never writes to other
applications, so such apps await pixel-based perception.

### Capture (developer tool)

```bash
argus capture --list               # displays, frontmost app, windows (JSON)
argus capture                      # frontmost window of the frontmost app
argus capture --window 482         # a specific window
argus capture --display            # the main display (or --display <ID>)
argus capture --delay 3 -o f.png   # wait, then also save a debug PNG
argus capture -o f.png --overlay   # draw the observation's element boxes on it
```

`argus capture` prints frame metadata (global bounds in points, pixel size,
scale factor). Pixels are kept in memory and discarded; a PNG is written only
with `--output`. `--overlay` outlines every element of the window's
accessibility observation on the PNG to verify grounding by eye.

Logging goes to stderr and is controlled by `--log-level` (or `ARGUS_LOG`)
and `--log-format text|json`.

See [CONTRIBUTING.md](CONTRIBUTING.md) for conventions.

## License

[MIT](LICENSE) © 2026 Denys Zhodik
