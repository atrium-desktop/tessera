# Documentation Style & Hyperlink Standards

This document specifies the style guide, Markdown conventions, and hyperlink
standards for all documentation in Tessera.

---

## 1. Hyperlink Standards

1. **Relative Paths**: All cross-references between documentation files use
   explicit relative paths, e.g.
   `[ADR-0159](../../adr/0159-package-boundaries-follow-capabilities.md)`.
2. **Never Use Absolute File URLs**: Never write hardcoded local paths
   (`/home/...`, `file://...`) or absolute repository URLs that break across
   mirrors and forks. External links to `github.com/ming2k/optics` decision
   records are the sole permitted exception, and only when citing an Optics
   ADR.
3. **Target Existence**: Every relative link must resolve to a valid file.
   Dead links or broken anchors block publication.
4. **Anchor Hygiene**: When linking to a section header, use GitHub-flavored
   kebab-case anchors matching the heading text.
5. **Scope Firewall**: See [Taxonomy](taxonomy.md) for the rule that public
   documentation must never link into `docs/dev/**`.

---

## 2. Formatting & Markdown Conventions

1. **Header Hierarchy**:
   - Exactly one `# Level 1 Heading` per file.
   - Subsections descend logically: `## Level 2`, then `### Level 3`. Never skip
     heading levels.
2. **Code Blocks**:
   - Always declare an explicit language identifier on fenced code blocks
     (`rust`, `bash`, `yaml`, `text`). Never leave fences untyped.
3. **Lists and Tables**:
   - Use clean Markdown tables with aligned delimiters.
   - Prefer `-` for unordered bullets and `1.` for sequential procedures.
4. **Line Width**: Wrap prose at roughly 80 columns for readable diffs.

---

## 3. Tone and Vocabulary

1. **Audience Calibration**: Write for competent engineers new to this
   codebase. Be direct, factual, and concise.
2. **Active Voice and Imperative Procedures**: Prefer active voice; start
   procedural steps with imperative verbs.
3. **Austere Reference Prose**: In `docs/reference/`, eliminate narrative
   pleasantries and provide precise tables, parameter types, and defaults.
4. **Mechanisms Over Aspirations**: When describing a constraint, name the
   enforcing type, function, boundary rule, or test rather than vague prose.
5. **Spelling Gate**: The repository runs `crate-ci/typos` in CI, with
   project-specific allowances in `typos.toml`. Add a term there rather than
   leaving a typo. CJK glyphs appear in quoted user-reported symptom strings
   (e.g. `重影`, `闪烁`) and are intentional.
