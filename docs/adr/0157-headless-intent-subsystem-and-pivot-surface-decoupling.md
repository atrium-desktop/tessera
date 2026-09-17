# ADR-0157: Headless intent subsystem and Pivot surface decoupling

- Status: Proposed
- Date: 2026-09-15
- Scope: `crates/chrome/tessera-pivot`, `crates/services/tessera-intent`, `crates/shell/tessera-pivot`, `docs/explanation/architecture.md`;
  amends and refines [ADR-0151](0151-pivot-unified-intent-and-action-surface.md)

## Context

[ADR-0151](0151-pivot-unified-intent-and-action-surface.md) successfully unified application discovery and quick search into **Pivot**, replacing the disparate full-screen launcher and Prism search box.

However, in implementing ADR-0151 as an initial MVP, **domain engine logic was inlined directly into the presentation crate** (`crates/chrome/tessera-pivot`):
1. **Inlined Math Evaluator (`src/calc.rs`)**: A complete arithmetic expression tokenizer and recursive-descent/precedence parser.
2. **Inlined Command Matcher (`src/commands.rs`)**: Hard-coded system action keywords, multilingual phrase matching (English and Chinese), and iconography mappings.
3. **Monolithic Query Pipeline (`build_search_items()`)**: The aggregation of open window titles, desktop application entries, calculation outputs, and agent queries was embedded directly inside the UI rendering lifecycle.

### The Architectural Failure Mode of Inlined Domain Logic

Because `tessera-pivot` was treated purely as a UI chrome component, its domain intelligence was held captive behind graphical rendering dependencies (`lens::Frame`, `lens::Input`, `tessera_design::materials`).

This coupling creates severe architectural blockades:
- **Zero Headless Reusability**: The system's central intent-matching and calculator capabilities cannot be consumed by the CLI (`tessera-ctl`), IPC clients, external accessibility tools, or HUD shortcuts without linking the entire immediate-mode UI rendering stack.
- **Impaired Testability**: Testing whether `"1920 * 1080"` computes correctly or whether `"锁屏"` properly resolves to `SystemCmd::Lock` required either running tests inside a crate burdened by UI types or manually mocking rendering contexts.
- **Concept-Identity Mismatch**: Pivot is hailed as the "System Intent & Action Cockpit" (ADR-0151 §2), yet its code was packaged as if it were merely a visual overlay.

---

## Decision

### 1. Decouple Engine from Presentation: `tessera-intent` vs `tessera-pivot`

We factor Pivot into two strictly separated components across architectural tiers:

```text
crates/
├── services/
│   └── tessera-intent/       # ★ The Headless Intent & Action Engine (100% Non-GUI)
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs        # IntentEngine, query pipeline, ranking
│           ├── calc.rs       # Pure arithmetic evaluation & unit conversion
│           ├── commands.rs   # System command palette definitions & fuzzy matching
│           ├── providers/    # Extensible Intent Providers (Apps, Windows, Commands, Math, Agent)
│           └── items.rs      # Data-only SearchItem, SearchAction, ActionOutcome
│
└── panels/
    └── tessera-pivot/        # ★ Pure Human-Interface Panel Surface
        ├── Cargo.toml        # Depends on tessera-intent, tessera-ui, lens
        └── src/
            ├── lib.rs        # Implements panel lifecycle & Chrome contract
            ├── layout.rs     # Search bar, category pill bar, result cards, liquid glass
            ├── input.rs      # Keystrokes (Tab, Return, Esc), caret navigation
            └── animation.rs  # Spring dynamics for progressive disclosure
```

### 2. The Headless `IntentEngine` Architecture

`tessera-intent` owns all query routing, ranking, and execution logic. It has zero rendering dependencies and exposes a clean asynchronous/synchronous query API:

```rust
pub struct IntentEngine {
    providers: Vec<Box<dyn IntentProvider>>,
}

pub trait IntentProvider: Send + Sync {
    fn query(&self, ctx: &QueryContext) -> Vec<IntentItem>;
    fn execute(&self, action: &IntentAction) -> Result<ActionOutcome, IntentError>;
}
```

Built-in headless providers include:
- **`MathProvider`**: Instant evaluation for arithmetic and unit expressions (formerly `calc.rs`).
- **`CommandProvider`**: Instant matching for desktop control actions (formerly `commands.rs`).
- **`AppProvider`**: Queries the application catalog provided by `tessera-desktop-entries`.
- **`WindowProvider`**: Matches live toplevel windows and workspaces from `tessera-state`.
- **`AgentProvider`**: Natural-language routing to `tessera-agent-broker` and `tessera-mcp`.

### 3. `tessera-pivot` Becomes a Thin View Consumer

`crates/panels/tessera-pivot` retains only:
- Visual layout: floating liquid-glass geometry, search bar typography, category filters, and result lists.
- Interactive animation: spring-driven expansion between compact search mode and expanded category browse mode.
- Input capture: forwarding keychars and selection events into `IntentEngine::query()` and mapping `IntentAction` into compositor runtime events.

---

## Invariants & Behavioral Boundaries

- **`[INV-INTENT-01]` Zero Graphics Dependency in `tessera-intent`**:
  `crates/services/tessera-intent` must never depend on `lens`, `flux`, `prism`, or any graphical rendering library. It operates exclusively on plain Rust strings, enums, numbers, and `protocol/tessera-state` models.
- **`[INV-INTENT-02]` Dual-Interface Equivalence**:
  Every action achievable through the Pivot visual interface must be executable headlessly via `IntentEngine::query` and `IntentEngine::execute` with identical outcomes.
- **`[INV-INTENT-03]` Sub-Millisecond Search Latency**:
  Local providers (Math, Command, Apps, Windows) in `IntentEngine` must complete queries in under 1 millisecond on modern hardware, guaranteeing immediate, stutter-free keystroke feedback for the UI surface.

---

## Alternatives (Negative Knowledge)

- **Keep calculation and commands in `tessera-pivot` but export them as public modules**: Rejected. Exporting non-GUI functions from a crate that links GPU rendering libraries still forces any non-GUI consumer (like a CLI or daemon) to transitively compile and link the entire graphics stack.
- **Create individual micro-crates for calculator, commands, and search**: Rejected as excessive fragmentation. The unifying concept is **Intent Resolution**. Grouping them as internal providers of `tessera-intent` maintains high cohesion without crate bloat.

---

## Consequences

### Positive
- Enables instantaneous headless testing of all calculation, fuzzy matching, and command resolution logic.
- Unlocks the future creation of `tessera-ctl` (command-line query and launch utility) without touching UI code.
- Purifies `tessera-pivot` into a focused, easily maintainable UI surface that excels at motion and presentation.

### Negative / Follow-up Work
- Requires extracting `calc.rs` and `commands.rs` into `crates/services/tessera-intent`.
- Requires refactoring `tessera-pivot/src/lib.rs` to consume `tessera_intent::IntentEngine`.
