# Contributing to Argus

## Requirements

- macOS (the first supported platform), Apple Silicon recommended.
- Stable Rust (pinned via `rust-toolchain.toml`, includes `rustfmt` and
  `clippy`).

## Checks

Every change must pass the same checks as CI:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace
```

Live tests that need a macOS GUI session and the Screen Recording permission
are `#[ignore]`d in CI. Run them locally when touching platform code:

```bash
cargo test --workspace -- --ignored
```

## Conventions

- **Boundaries.** Respect the crate dependency graph described in the README.
  It is enforced by `tests/integration/tests/architecture.rs`; a new crate must
  be declared there.
- **Errors.** Library crates define a typed `Error` enum with `thiserror` and a
  `Result<T, E = Error>` alias. `argus-core` aggregates them. Binaries use
  `anyhow`. Every core error has a stable `code()`.
- **Logging.** Use `tracing`. Logs go to stderr; stdout is reserved for
  machine-readable output. Never log screen content (recognized text, element
  names, pixels) at any level.
- **Unsafe.** Only platform crates (`argus-capture`, `argus-accessibility`,
  `argus-perception`) may use `unsafe`, and every `unsafe` block needs a
  `// SAFETY:` comment. All other crates use `#![forbid(unsafe_code)]`.
- **Privacy.** No cloud upload, no persistent screenshots, no screen-content
  telemetry. Debug captures are written only when explicitly requested.
- **Tests.** Each change comes with tests; new protocol behavior comes with
  golden fixtures.
