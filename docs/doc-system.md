# Kern Native Documentation Draft

## Goals

Kern documentation should follow the same language philosophy as the rest of the toolchain:

- High abstraction, low policy.
- Clarity over novelty.
- Explicit over implicit.
- Elegance through a small native surface and strong semantics behind it.

The source syntax should stay minimal. The compiler should own the structure. Renderers such as `craft`, `kern-lsp`, and future site tooling should consume the same semantic document model instead of reparsing ad hoc Markdown.

## Native Source Syntax

Kern supports two native documentation comment forms:

- `///` for outer item and member documentation.
- `//!` for inner module or file documentation.

Examples:

```kern
//! UART primitives for freestanding serial output.
//!
//! Design:
//! Keep the hardware boundary explicit and typed.

/// A typed view over the UART register block.
type Uart = struct {
    /// Base MMIO address.
    ///
    /// Safety:
    /// - Must point to a mapped UART register block.
    base: ^mut u8,
};
```

The syntax is intentionally plain:

- No `#[doc(...)]`.
- No command sigils such as `@param`.
- No Markdown-only control syntax as the semantic source of truth.

## Built-In Sections

The first paragraph is the summary. After that, Kern recognizes a small set of section headers written as plain `Title:` lines.

Initial built-in sections:

- `Args:`
- `Returns:`
- `Errors:`
- `Safety:`
- `Effects:`
- `Requires:`
- `Ensures:`
- `State:`
- `Boundary:`
- `Design:`
- `Rationale:`
- `Example:`
- `See:`
- `Note:`
- `Warning:`

These are chosen to fit Kern's systems-level model. The most important first-class sections are:

- `Safety`
- `Effects`
- `Boundary`
- `Design`

`Design:` is the main place to express the "elegance" philosophy in API form: why this surface is minimal, explicit, and policy-free.

## Semantic Model

The compiler captures doc comments as source `DocBlock`s in the AST and normalizes them into a semantic `KernDoc` model for tooling.

Conceptual model:

```text
DocBlock
  raw lines + span

KernDoc
  summary
  details
  sections[]
  raw_text

KernDocSection
  kind
  title
  body
  entries[]

KernDocEntry
  name?
  body
```

Normalization rules:

- The first prose paragraph becomes `summary`.
- Remaining prose before the first recognized section becomes `details`.
- Recognized `Title:` lines start semantic sections.
- `Args:` and similar list-oriented sections may contain `- name: body` entries.
- Unknown section titles are preserved as custom sections instead of rejected.

## Rendering Model

Source comments remain simple. Rendering is where docs become visually rich.

Expected presentation:

- Show the item signature first.
- Show the summary next.
- Render key sections like `Safety`, `Effects`, `Boundary`, and `Design` with distinct styling.
- Render `Example` as highlighted Kern code when possible.

This keeps source code elegant while still allowing polished output in Markdown, IDE hover, or generated sites.

## Toolchain Integration

### Compiler

The front-end parses `///` and `//!` directly into AST-attached doc blocks for:

- modules
- declarations
- struct fields
- enum variants
- trait methods

The semantic pipeline preserves docs on collected definitions so later compiler stages and tools can query them without reparsing source.

### `kern-lsp`

Hover output should be:

1. signature
2. summary
3. selected structured sections

This gives hover text more semantic value than the current signature-only presentation while preserving a stable Markdown contract for editors.

### `craft`

Documentation metadata should be emitted as part of the native package metadata root rather than embedded into `.craft/analysis.toml`, which is already reserved for build and analysis context.

First-stage metadata format:

- `Kmeta.toml` remains the package manifest.
- `Kmeta.docs.toml` carries structured documentation items derived from the compiler-owned `KernDoc` model.

That gives `craft` two downstream options:

- render Markdown directly from `Kmeta.docs.toml`
- or use the TOML as an intermediate form for richer presentation and indexing

## Phase Plan

Phase 1:

- native `///` and `//!` parsing
- AST and sema doc attachment
- semantic doc normalization
- hover rendering with docs
- `Kmeta.docs.toml` export

Phase 2:

- doc linting
- completion/documentation integration
- dedicated `craft doc` rendering command
- package import doc reuse for external library hovers

## Non-Goals For Phase 1

- block doc comments
- a full Markdown dialect as the semantic source of truth
- exhaustive style linting
- HTML/site generation in the compiler itself
