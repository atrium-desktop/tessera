# Tessera repository guidance

## Testing

- Run tests with `cargo nextest`. Do not add or use an xtask test command;
  xtask owns architecture and packaging checks only.
- Keep the multi-crate workspace. Tests follow crate boundaries.
- Small and medium modules keep an inline `#[cfg(test)] mod tests` next to the
  code.
- Large test blocks that have become unit-test bloat move to a sibling
  `<module>/tests.rs` (or `<module>/<test-module>.rs`) and the parent keeps
  `#[cfg(test)] mod tests;`.
- Shared test infrastructure lives in `crates/tessera-test-support` and is referenced
  only from `[dev-dependencies]`.
- External tests live in the root `tests/` package:
  - `tests/common/mod.rs` for shared helpers;
  - `tests/integration/main.rs` plus `tests/integration/` for integration
    modules;
  - `tests/e2e/main.rs` plus `tests/e2e/` for end-to-end modules;
  - one entry per suite so each suite links once;
  - `tests/fixtures/` for probe sources and data.
