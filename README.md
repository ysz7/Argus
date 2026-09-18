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

> **Status:** early development. The [Observation Protocol v0.1](spec/ARGUS_PROTOCOL.md) is defined; perception is not implemented yet.

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

Requirements: macOS, stable Rust (see `rust-toolchain.toml`).

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

Logging goes to stderr and is controlled by `--log-level` (or `ARGUS_LOG`)
and `--log-format text|json`.

See [CONTRIBUTING.md](CONTRIBUTING.md) for conventions.

## License

[MIT](LICENSE) © 2026 Denys Zhodik
