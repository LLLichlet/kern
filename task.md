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
  The implementation currently normalizes trailing horizontal whitespace and
  final-newline consistency. AST-level layout and method-chain wrapping remain
  future formatter work.
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
- [x] Do not add constructor-convention style suggestions yet. Real code uses
  `T.{}` legitimately for field defaults, resets, runtime internals,
  freestanding support, tests, and plain aggregates. Keep the style guidance in
  docs/style.md, and revisit an automated rule only with semantic information
  about real constructor, allocator, builder, or capability-boundary APIs.
- [x] Keep style rules configurable by severity and scope so prototypes,
  low-level runtime code, and mature packages can choose different strictness.
- [x] Add tests for lint metrics and CLI output before enabling stricter rules.

## P3 Adoption And Policy

- [x] Update docs/style.md with docstring, match, formatter, lint, comment
  ratio, and test-coverage expectations.
- [x] Define package maturity gates: minimum public-doc coverage, comment ratio
  bands, smoke tests, and optional benchmark coverage.
- [x] Wire stabilized checks into `craft publish` without hard-coding maturity
  thresholds: deterministic `craft fmt --check` semantics are enforced, while
  style suggestions and public-doc metrics are reported as review signals until
  explicit release-policy thresholds exist.

## P4 Publish Safety

- [x] Treat `craft publish` as a strict local release gate instead of a mutating
  package-preparation command.
- [x] Require publishable packages to be inside a Git worktree with a resolvable
  `HEAD`.
- [x] Reject dirty Git worktrees before release checks run.
- [x] Require `Craft.lock` to exist and be committed before publish.
- [x] Check release graph lockfile freshness without rewriting `Craft.lock` so
  stale lockfiles cannot be silently repaired during publish.
- [x] Require each publishable package's `repository` metadata to match a
  configured Git remote, including normalized HTTPS/SSH GitHub forms.
- [x] Add publish tests for missing Git worktrees, dirty worktrees, missing or
  damaged committed lockfiles, remote mismatches, normalized SSH remotes, source
  policy, formatting, style, and workspace package metadata.

## P5 Distributed Publish Proofs

- [x] Add committed `Craft.lock` publish proofs instead of depending on a
  central registry or a separate publish artifact.
- [x] Record package identity, repository, and SHA-256 digests for
  `Craft.toml` and package source contents.
- [x] Make normal lockfile synchronization generate or update stale proofs and
  require the current lockfile to be committed before publish succeeds.
- [x] Verify Git dependencies automatically while fetching, without requiring a
  caller opt-in policy.
- [x] Reject Git dependencies with missing proofs, stale proofs, package/version
  mismatches, or repository/source mismatches.
- [x] Keep path dependencies as local/development sources outside the default
  ecosystem proof boundary.
