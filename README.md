# Argus

**Argus** is a local perception layer for computer-use systems. It turns what
is on the screen into a structured, grounded, confidence-aware observation that
an external AI agent can consume, and connects to AI clients as an MCP server
that also executes the agent's actions, with guards.

```text
screen / native UI
        ↓
      Argus  ──── text where it is sure, images where it is not
        ↓
 external AI agent (Claude Desktop, Claude Code, …)
        ↓
 action ("click e_12, expecting "Save"") → Argus checks and executes
```

Argus answers one question:

> What is in the interface right now, where is it, what state is it in, and
> how confident is Argus about each claim?

> **Status:** early development, macOS only. The [Observation Protocol v0.2](spec/ARGUS_PROTOCOL.md)
> is defined; macOS screen capture, Accessibility, OCR and visual UI
> detection work and are fused into one observation; elements keep stable IDs
> across successive observations, and `argus watch` reports what changed.
> `argus mcp` connects Argus to AI clients ([MCP](spec/ARGUS_MCP.md)).

Argus is open source under the [MIT license](LICENSE). It runs entirely on
your Mac: perception needs no model, no API key and no network.

## Highlights

- **Structure first, pixels where needed.** Native accessibility trees,
  OCR and a visual detector are fused into one observation: roles, names,
  values, states (including text selection and styles), bounds in screen
  points and a confidence for every claim.
- **Stable element IDs and deltas.** The same element keeps its ID across
  observations; after every action the agent receives only what changed.
- **Hybrid view for agents.** Compact text where Argus is sure; images of
  just the regions it cannot describe (windows without accessibility, drawn
  content, pop-ups).
- **MCP connector with guarded actions.** `argus mcp` plugs into Claude
  Desktop and Claude Code. The agent acts by element ID with the name it
  expects; Argus refuses mismatches, stays inside the observed application
  and blocks system-wide shortcuts and sensitive applications.
- **Fast.** An observation takes 80–400 ms; an action with its resulting
  delta about 0.7 s.

## Results so far

- **Agent evaluation** (Claude Sonnet 5, 18 tasks, 176 runs): on
  applications with accessibility, an agent using Argus succeeded as often as
  with screenshots (89% vs 93%) with 37% fewer tokens and 32% less time. On a
  drawn canvas without accessibility, the fused view solved every task that
  accessibility alone could not.
- **Live MCP tests** in Claude Code: Calculator, TextEdit (write, format
  and save a document) and Chess (a 19-move game, every move by element) all
  completed without a wrong click or a false "done".

## Quick start

```bash
cargo install --path crates/argus-cli          # → ~/.cargo/bin/argus
argus doctor                                   # permissions and backends
argus observe --app Calculator                 # one observation as JSON
claude mcp add --scope user argus -- ~/.cargo/bin/argus mcp   # Claude Code
```

Then ask Claude, e.g. *"With Argus, compute 389 + 456 in Calculator"*. Setup
for Claude Desktop and all options: [MCP](#mcp-claude-desktop-and-claude-code).

## Architectural boundaries

Argus is **not** an AI agent and not a single AI model.

```text
ARGUS (perception)   pixels / native structure → meaning
AGENT                meaning → intention
ARGUS (executor)     intention → checked physical action
```

Argus never:

- plans tasks or decides what to click;
- acts on its own: the mouse and keyboard move only on an agent's explicit
  command through the MCP adapter, in the observed application, after the
  guards of [`spec/ARGUS_MCP.md`](spec/ARGUS_MCP.md);
- makes decisions on behalf of an agent;
- replaces application APIs;
- becomes an LLM agent.

The perception pipeline (`argus-core`) and the HTTP service only observe;
the executor (`argus-input`) is used by the MCP adapter alone.

Screen content is untrusted input: text on screen is reported as UI content,
never interpreted as instructions to Argus.

### Privacy by default

- all processing is local; Argus uploads nothing. Over MCP, observations and
  region images go to the AI client that started the server (and from there
  to its model), as tool results, and nowhere else;
- screenshots are not persisted (debug captures only on explicit request);
- no telemetry containing screen content;
- the local service listens on localhost only and refuses requests from web
  pages.

## Workspace layout

| Crate                 | Responsibility                                          |
| --------------------- | ------------------------------------------------------- |
| `argus-protocol`      | Public data structures and serialization contract.      |
| `argus-capture`       | Frame acquisition (screens, windows).                   |
| `argus-accessibility` | Native accessibility adapters (macOS first).            |
| `argus-perception`    | OCR and visual perception backends.                     |
| `argus-fusion`        | Merging evidence from different sources; scene graph.   |
| `argus-tracking`      | Element identity and changes over time.                 |
| `argus-core`          | Orchestration pipeline; aggregated error type.          |
| `argus-server`        | Local service / API (observation only).                 |
| `argus-input`         | Mouse and keyboard execution (MCP adapter only).        |
| `argus-mcp`           | MCP server: agent view, region images, guarded actions. |
| `argus-benchmark`     | Benchmark dataset, replay and metrics.                  |
| `argus-cli`           | The `argus` binary: user and developer commands.        |

### Pipeline

```text
accessibility ─┐
OCR           ─┼─▶ SourceCandidate ─▶ normalize ─▶ Normalized ─▶ fuse + scene graph ─▶ Fused ─▶ assemble ─▶ Observation
vision        ─┘
```

(`vision` is a model-free heuristic detector.)

Sources map their native vocabulary (e.g. `AXButton`) to protocol roles and
report evidence as `SourceCandidate`s. Normalization (`argus-core`) converts
coordinates to global points, derives visibility, cleans text and enforces
consistency of roles, states and confidence. Fusion (`argus-fusion`) merges
the evidence of all sources so that one real object becomes one element,
listing every contributing source; it records conflicts and lowers the
confidence of disputed properties (policy in
[`crates/argus-fusion/src/lib.rs`](crates/argus-fusion/src/lib.rs)). The scene
graph step then infers structure: `label_for` relations (reported by the
platform, or a text next to an unlabelled checkbox, radio button or field),
`row` elements for repeated lines of pixel-derived elements, and `contains`
relations for elements drawn inside others outside the tree. Inferred
structure is attributed to the `derived` source.
Observations can only be assembled from fused, normalized evidence.

Allowed internal dependencies (enforced by
[`tests/integration/tests/architecture.rs`](tests/integration/tests/architecture.rs)):

```text
argus-cli ──→ argus-server ──────┐
    │     ├──→ argus-mcp ────────┼──→ argus-core ──→ capture, accessibility,
    │     │        └──→ argus-input                   perception, fusion, tracking
    │     └──→ argus-benchmark ──┘
    └──────────────────────────────────────────────→ argus-protocol
```

- `argus-protocol` depends on no other Argus crate and contains no platform
  code.
- Source and processing crates (`capture`, `accessibility`, `perception`,
  `fusion`, `tracking`) depend only on `argus-protocol` and never on each
  other; `argus-core` wires them together.
- Nothing depends on `argus-server`, `argus-mcp`, `argus-benchmark` or
  `argus-cli` except the CLI itself; only `argus-mcp` depends on
  `argus-input`.

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
- **Screen & System Audio Recording** — needed by `argus capture` and by the
  `ocr` and `vision` sources of `argus observe` (on by default).

The first attempt triggers the system prompt.

### Doctor

```bash
argus doctor            # permissions, backends, local service
argus doctor --json
```

```text
Argus Doctor (argus 0.0.1)

macOS                OK       27.0
Screen recording     OK       granted to Visual Studio Code
Accessibility        OK       granted to Visual Studio Code
Capture backend      OK       main display 1470×956 pt @2x in 115 ms (not saved)
Accessibility tree   OK       Finder: 212 nodes in 30 ms
OCR backend          OK       2/2 sample lines recognized in 113 ms
Vision backend       OK       4 buttons, 2 checkboxes in the sample in 1 ms
Local server         INFO     not running on port 7412
                     → start it with `argus serve`

Everything is ready.
```

Every check runs the real path: the main display is captured (and
discarded), OCR and visual detection run on built-in sample images, and the
accessibility tree of the frontmost application is read (only its size is
reported). Failed checks say what to do, e.g. which application to allow in
which System Settings pane. The exit status is non-zero if a check failed.

### Observe

```bash
argus observe                          # focused window of the frontmost app
argus observe --app Calculator         # by name or bundle id, works in background
argus observe --pid 4321
argus observe --sources accessibility  # only the native accessibility tree
argus observe --sources ocr,vision     # only the window's pixels
argus observe --count 10 --interval-ms 500   # a tracked series, as JSON Lines
argus watch --app Calculator           # what changes, as it changes
argus watch --json                     # first observation, then deltas (JSON Lines)
argus inspect                          # evidence and conflicts of every element
argus inspect e_42 --json              # ... of one element, as JSON
```

Prints an [Argus Observation](spec/ARGUS_PROTOCOL.md) as JSON. By default the
accessibility tree, OCR and visual detection of the same window are fused.
`argus inspect` shows, for each element, which source said what and which
conflicts were resolved how. It observes anew, so element IDs from an earlier
run match only while the interface is unchanged.

With `--count`, successive observations form a tracking session: an element
that is still there keeps its ID, a new one gets an ID never used before, and
`confidence.identity` says how sure Argus is that an element is the one that
had the ID before (see [§3.1 of the protocol](spec/ARGUS_PROTOCOL.md)). IDs
are stable within one run only.

`argus watch` observes repeatedly and prints the changes:

```text
obs_mu7z4911_000004  +1 -0 ~1
+ e_19 dialog "Saved"
~ e_17 enabled true → false
```

With `--json` it prints the first observation of a session and then one
[ObservationDelta](spec/ARGUS_PROTOCOL.md) per observation; applying the
deltas in order reproduces every observation.

Successive observations of a window are perceived **incrementally**: the new
frame is compared with the previous one, and OCR and visual detection run
again only on the regions that changed (grown to the text lines and controls
they touch); everything else is reused. Unchanged frames, including a moved
window, reuse all results; large changes are perceived in full.

```bash
argus watch --stats                    # per observation: mode, changed pixels, time per stage
argus watch --no-incremental           # perceive every frame in full
argus watch --verify-incremental       # compare each incremental result with a full one
```
`argus accessibility` prints the raw native tree (`AXButton`, ...) for
debugging the platform adapter.

OCR uses Apple Vision on-device. It reports text and its position only —
recognized text never implies a control role. The very first recognition
after a new `argus` binary is built can take ~20–30 s while macOS prepares
its models; later runs take a few hundred milliseconds per window.

Visual detection is a deterministic, model-free detector (outlines, shapes
and content of controls). It proposes buttons, text boxes, checkboxes, radio
buttons, tabs and icons with deliberately modest confidence; other sources
confirm or reject these hypotheses.

Electron/Chromium applications (VS Code, Slack, ...) expose only a skeleton
tree unless asked to enable accessibility; Argus never writes to other
applications, so for them the pixel sources supply most elements.

When an application exposes no accessible window or does not answer
accessibility requests (custom toolkits, games), `argus observe` warns and
continues with the pixel sources; with `--sources accessibility` alone the
error is reported. Conversely, when the accessibility window cannot be
captured (off screen, another Space), its tree is reported alone.

macOS answers accessibility requests unreliably while the screen is locked
(the application element instead of its window); Argus then reports that the
application has no accessible window.

### MCP: Claude Desktop and Claude Code

`argus mcp` is an MCP server on stdio: the AI client starts it and the model
uses its tools (`list_apps`, `observe`, `click`, `type_text`, `press_keys`,
`scroll`, `drag`, `screenshot`; [spec](spec/ARGUS_MCP.md)). Install the binary
once:

```bash
cargo install --path crates/argus-cli      # → ~/.cargo/bin/argus
```

**Claude Desktop:** Settings → Developer → Edit Config, then add to
`~/Library/Application Support/Claude/claude_desktop_config.json` (use your
home directory; restart Claude Desktop afterwards):

```json
{
  "mcpServers": {
    "argus": {
      "command": "/Users/YOU/.cargo/bin/argus",
      "args": ["mcp", "--log-file", "/Users/YOU/Library/Logs/argus-mcp.jsonl"]
    }
  }
}
```

**Claude Code:**

```bash
claude mcp add --scope user argus -- ~/.cargo/bin/argus mcp --log-file ~/Library/Logs/argus-mcp.jsonl
```

macOS attributes the permissions to the client: allow **Claude** (or the
terminal / VS Code running Claude Code) under Accessibility and Screen &
System Audio Recording, then restart it. Ask, for example, *"With Argus, open
Calculator and compute 389 + 456"*.

Guards: actions only in the observed application and only on its own
windows; clicks by id are refused when `expect` does not match the element;
terminals, password managers, System Settings, chat clients and the editors
hosting them (VS Code, Cursor) are blocked
(`--unblock-app NAME` lifts one); system-wide shortcuts (cmd+tab, cmd+space,
log out, screenshots) are refused; typing stops if you move the mouse.
Options: `--read-only` (observe only), `--allow-app NAME` (only these
applications), `--log-file PATH` (one JSON line per call: tool, duration,
text and image sizes, never screen content).

Several clients and sessions can use Argus at once: each `argus mcp` relays
to one shared background process (macOS lets only one process of the binary
capture the screen), which exits a minute after the last client. `argus
serve` cannot capture meanwhile.

### Local service

```bash
argus serve                            # http://127.0.0.1:7412
argus serve --port 0                   # a free port, printed on stdout
```

Any local program can then observe without linking with Rust
([API](spec/ARGUS_SERVICE.md)):

```bash
curl 'http://127.0.0.1:7412/v1/health'
curl 'http://127.0.0.1:7412/v1/observation?app=Calculator'          # observe now
curl 'http://127.0.0.1:7412/v1/changes?since=obs_mu81clxa_000002'    # observe again: the delta
curl 'http://127.0.0.1:7412/v1/elements/e_4'                         # an element and its evidence
```

Each application (and set of sources) has its own tracking session, shared
by all clients; observations are kept in memory in a bounded history
(`--history`, default 32) and nothing is written to disk. Requests from web
pages (an `Origin` header, or a `Host` other than localhost) are refused.
[`examples/python/watch.py`](examples/python/watch.py) is a client that uses
only the Python standard library:

```bash
python3 examples/python/watch.py --app Calculator --count 10
```

macOS lets only one running process of a program capture the screen: while
`argus serve` runs, the pixel sources of other `argus` commands time out. Ask
the service instead, or stop it.

### Benchmark

```bash
argus benchmark run                     # accuracy on benchmarks/dataset, by mode
argus benchmark run --mode pixels --explain
argus benchmark latency --app Calculator   # live latency and memory
```

Recorded windows (frame, accessibility tree, reviewed ground truth) are
replayed through the real pipeline with the accessibility tree alone, visual
detection alone, pixels alone (OCR + detection) and everything fused, and
scored for recall, precision, role, text, grounding, states, relations,
identity across steps and confidence calibration. See
[benchmarks/README.md](benchmarks/README.md) for the method, the dataset and
the baseline.

### Capture (developer tool)

```bash
argus capture --list               # displays, frontmost app, windows (JSON)
argus capture                      # frontmost window of the frontmost app
argus capture --window 482         # a specific window
argus capture --display            # the main display (or --display <ID>)
argus capture --delay 3 -o f.png   # wait, then also save a debug PNG
argus capture -o f.png --overlay   # draw the observation's element boxes on it
argus capture -o f.png --overlay --overlay-sources ocr,vision   # ... of other sources
```

`argus capture` prints frame metadata (global bounds in points, pixel size,
scale factor). Pixels are kept in memory and discarded; a PNG is written only
with `--output`. `--overlay` outlines every element of an observation of the
window on the PNG to verify grounding by eye.

Logging goes to stderr and is controlled by `--log-level` (or `ARGUS_LOG`)
and `--log-format text|json`.

See [CONTRIBUTING.md](CONTRIBUTING.md) for conventions.

## License

[MIT](LICENSE) © 2026 Denys Zhodik
