# Kern Ecosystem Tooling Tasks

## P0 Docstrings As Markdown-First Documentation

- [x] Treat doc comments as Markdown-first text and avoid warning on ordinary
  prose lines that happen to end in `:`.
- [x] Keep structured sections opt-in through a recognized title set
  (`Args:`, `Returns:`, `Errors:`, `Safety:`, etc.) instead of treating every
  alphabetic `Title:` as a section.
- [x] Preserve raw doc text in metadata and generated docs without losing
  Markdown constructs such as headings, lists, fenced code blocks, links, and
  prose labels.
- [x] Add parser/doc tests covering Markdown labels, headings, lists, code
  fences, and section parsing.
- [x] Add public API doc quality metrics to generated docs or lint output:
  documented public items, undocumented public items, and warning counts.

## P1 Match Over Comparable Values

- [x] Specify the semantics for value-pattern match arms over non-scalar values:
  arm patterns are evaluated in order and compare with the scrutinee through
  the appropriate equality operator.
- [x] Decide whether the capability is `Eq[T]` directly or a dedicated pattern
  trait. Start conservatively with `Eq[T]` for literal/const-like patterns.
- [x] Extend parsing/AST/sema/lowering so string and slice-like values can be
  matched without `if name == "...";` chains.
- [x] Add positive tests for custom `Eq` match value patterns, plus
  ambiguity/type-error tests.
- [x] Document the feature in design.md and tutorials with guidance on when
  `match` improves dispatch readability.

## P2 Style, Formatting, And Lint Tooling

- [x] Add `craft fmt` as a deterministic formatter entry point.
- [x] Add `craft lint` or `craft style` as a non-mutating analyzer for project
  health and Kern idioms.
- [x] Start with source metrics: source files, code lines, blank lines, inline
  comments (`//`, `/* */`), doc comments (`///`, `//!`), comment ratio, doc
  ratio, and doc-line totals.
- [x] Add source-level public-doc coverage to `craft style` without forcing a
  full build.
- [x] Add semantic public-doc coverage from compiler metadata for release-grade
  documentation policy.
- [x] Add first advisory source-level style suggestions that have already
  appeared in real packages: prefer `for`/iterators over simple index-only
  `while`; use handle-style temporaries for repeated borrowed receivers; split
  long postfix chains.
- [ ] Add constructor-convention style suggestions where the signal is strong:
  prefer helpers such as `string()`, `list()`, `map()`, or domain-specific
  constructors such as `page()` only when they express a real constructor,
  allocator, builder, or capability boundary.
- [x] Keep style rules configurable by severity and scope so incubators,
  low-level runtime code, and mature packages can choose different strictness.
- [x] Add tests for lint metrics and CLI output before enabling stricter rules.

## P3 Adoption And Policy

- [x] Update docs/style.md with docstring, match, formatter, lint, comment
  ratio, and test-coverage expectations.
- [ ] Define package maturity gates: minimum public-doc coverage, comment ratio
  bands, smoke tests, and optional benchmark coverage.
- [ ] Wire new checks into `craft publish` or release-policy validation only
  after the standalone commands have stabilized.
