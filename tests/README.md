# Root test suites

This package is the single external-test entry point for the workspace. It is a
workspace member but never a production dependency.

```text
tests/
  common/            shared helpers; re-exports tessera-test-support
  integration/       one entry, one link: main.rs + modules
  e2e/               one entry, one link: main.rs + modules
```

Run both suites:

```bash
cargo nextest run -p tessera-tests
```

The end-to-end modules spawn the production binaries as separate processes.
`.config/nextest.toml` builds `--workspace --bins` once, before the tests that
need them, because a test that runs `cargo build` itself blocks on the
target-directory lock the runner already holds. Running the suite outside
nextest means building those binaries first.

Run the native-free subset (native modules are compiled out):

```bash
cargo nextest run -p tessera-tests --no-default-features
```

The `native` feature gates tests that need the compositor binary, gfx/optics-ui, or
the real shell tray service. `tessera-test-support` supplies environment
guards, scratch directories, socket names, polling, and workspace/binary
lookup. Do not add normal dependencies from production crates to this package
or to `tessera-test-support`.
