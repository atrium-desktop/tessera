# Technical Style, Syntax, and Link Contracts

This document defines normative rules for technical writing, syntax, formatting, and link contracts.

---

## 1. Technical Voice & Grammar

- **Audience Calibration**: Write for competent engineers new to this codebase. Be direct, factual, and concise.
- **Active Voice**: Prefer active voice over passive constructions.
  - *Correct*: "The engine initializes the buffer pool before accepting connections."
  - *Avoid*: "The buffer pool is initialized by the engine before connections are accepted."
- **Imperative Mood for Procedures**: In how-to guides and task procedures, start steps with imperative verbs.
  - *Correct*: "Clone the repository and run the test suite."
  - *Avoid*: "You should clone the repository and then running the tests is required."
- **Second Person for Tutorials**: Use second person ("you") in tutorials to guide the learner through a journey.
- **Austere Reference Prose**: In reference documentation, eliminate narrative pleasantries. Provide precise tables, parameter types, default values, and return signatures.

---

## 2. Heading & Document Hierarchy

- **Single H1 per File**: Every Markdown file must start with exactly one top-level `# Title`.
- **Hierarchical Nesting**: Never skip heading levels (do not jump from `# H1` to `### H3`). Use `## H2` for primary sections and `### H3` for subsections.
- **Actionable Headings**: In how-to guides, headings must state the specific task: `## Configure TLS Certificates` rather than `## Configuration`.

---

## 3. Relative Link Contracts

- **Direct File References**: Always link directly to explicit Markdown files, including the `.md` extension:
  - *Correct*: `[Taxonomy](taxonomy.md)` or `[ADRs](../adr/0001-setup.md)`
  - *Avoid*: `[Taxonomy](taxonomy)` or `[ADRs](../adr/)`
- **Case Sensitivity**: File names in links must match filesystem casing exactly (POSIX-strict).
- **No Floating Links**: Inline links must describe the destination clearly. Never use generic labels like `[here](link.md)` or `[link](link.md)`.
- **Firewall Compliance (`[INV-LINT-02]`)**: Never author a relative link that crosses from a public documentation surface into `docs/dev/**`.

---

## 4. Code Blocks & Formatting

- **Explicit Language Identifiers**: All fenced code blocks must specify an explicit syntax highlighting identifier (e.g., `bash`, `json`, `markdown`, `rust`, `yaml`).
- **Executable Commands vs. Output**:
  - Distinguish input commands from expected command output.
  - In shell examples, omit shell prompts (`$ `) when the block is intended for direct copy-pasting.

---

## 5. Standard Root Document Patterns

### `README.md`
The entry point must remain lean (1–2 pages maximum):
1. **Title & Value Pitch**: Clear description of what the project does and why it exists.
2. **Key Capabilities**: 3–5 bullet points highlighting core differentiators.
3. **Quick Start**: Minimal shell commands to build, run, or test from a clean clone.
4. **Documentation Map**: Direct links into `docs/` for deeper exploration.

### `CHANGELOG.md`
Follow Keep-a-Changelog conventions:
- Sections grouped by version: `## [X.Y.Z] - YYYY-MM-DD`.
- Standard subsection tags: `Added`, `Changed`, `Deprecated`, `Removed`, `Fixed`, `Security`.
