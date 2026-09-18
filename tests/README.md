# Tests

- `integration/` — workspace-level integration tests (crate
  `argus-integration-tests`), e.g. enforcement of architectural boundaries.
- `fixtures/` — input data for tests (UI trees, frames). Never commit private
  screen content. `fixtures/fusion/<name>/` holds recordings of one window by
  several sources; how to record one is described in
  `crates/argus-core/tests/fusion_golden.rs`.
- `golden/` — expected outputs (e.g. Observation JSON) compared against by
  tests.

Crate-specific tests live next to the code (`#[cfg(test)]` modules) or in the
crate's own `tests/` directory.
