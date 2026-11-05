# Error Handling & Panic Discipline

How Tessera distinguishes recoverable errors from unrecoverable states, and the
two mechanically enforced rules that keep panics from becoming an availability
defect. The normative rules are checked by
`cargo run -p xtask -- check-panics` (ADR-0195, `INV-ARCH-51`).

---

## 1. Errors Are Values, Panics Are Reachability

A compositor is a long-lived, multi-threaded service: a panic on any render,
input, IPC, or worker thread is a visible session failure, and a panic while a
mutex is held poisons it, cascading that one failure into every later
acquisition. So the default is to return typed errors across every boundary, and
to reserve `panic!` for states the type system cannot rule out but the program
guarantees.

| Class | Where it belongs | Form | Example |
| :--- | :--- | :--- | :--- |
| **Recoverable** | Every API, protocol, and IO boundary | `Result<_, E>` with a crate-local `thiserror::Error` | `ServerError`, `ConnectError` |
| **Fail-fast startup** | Process entry, before the event loop | `expect("<what invariant>")` with a message naming the cause | a missing shader or an unbindable display socket |
| **Invariant tripwire** | State machines with a proven exhaustiveness | `panic!`/`unreachable!` on a branch a test can show unreachable | a render transaction that is provably not re-entered |
| **Lock poisoning** | Never | `unwrap_or_else(\|e\| e.into_inner())` | see rule 1 |

A panic site is acceptable only if it is one of the middle two classes. Any
other `unwrap`/`expect` is a latent defect and is what the ratchet counts.

---

## 2. Rule 1 — No Poisonable Lock May Panic (Hard Zero)

Acquiring a `Mutex`/`RwLock` must never `.unwrap()`/`.expect()` the poison
result. A single panicking writer turns the mutex into a panic generator for
every later reader; the compositor is more available if it recovers the guard
and logs.

```rust
// Forbidden — one panicking thread poisons this for all callers.
let state = self.state.lock().unwrap();

// Required — recover the guard; the data is structurally intact.
let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
```

Prefer `try_lock` with a defined fallback where blocking is unacceptable (the
render thread must never stall on a worker, see `tray::TrayProjection`).

## 3. Rule 2 — No New Panic Sites (Ratchet)

The production panic sites that remain are grandfathered in
[`tooling/xtask/panic-baseline.toml`](../../tooling/xtask/panic-baseline.toml).
The gate fails if any crate's count rises, or if a new crate appears with
sites and no baseline. Counts may fall freely; when they do, regenerate the
baseline in the same change so the ledger reflects reality:

```bash
cargo run -p xtask -- check-panics --update
```

Raising a baseline is a review decision: it must name the site and why the
state is unreachable without a type-level proof, the same bar as a new
invariant tag.

## 4. Scope and Exclusions

The gate measures **production** code only: `crates/*/src/**` minus any
`#[cfg(test)]` item (brace-aware, so a mid-file test helper does not hide the
production code below it) and minus `tests/` directories and `tests.rs` modules.
Tests are unrestricted — `unwrap` in a test is a failure signal, not a defect.

Crate `build.rs` files and `examples/` are outside `src/` and are not counted.

## 5. Verifying

```bash
cargo run -p xtask -- check-panics        # fail-closed in CI
cargo run -p xtask -- check-panics --update  # rewrite the baseline (review it)
```
