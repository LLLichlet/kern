# Kern LSP Roadmap

This note tracks the stabilization work needed to move `kern-lsp` from its
older 0.6-era architecture toward the same reliability standard as `kernc` and
`craft`.

The immediate rule is that editor responsiveness is correctness. A language
server request must prefer a partial, stale, lexical, or parse-only answer over
blocking the process on full analysis of broken in-memory source.

## Request Tiers

LSP analysis should stay split by cost and risk.

- lexical: semantic-token fallback and local keyword completion
- parse-only: syntax diagnostics, document structure, lightweight code actions
- surface: top-level symbols, signatures, imports, and type-position completion
- clean semantic: hover, navigation, full completion, and references from the
  last clean artifact
- dirty semantic: only allowed for bounded, proven body-only edits

Dirty source should not silently flow into full semantic analysis. When a dirty
request cannot prove that it is cheap, it should use lexical/parse/surface data
or the clean artifact.

## Hot-Path Rules

- Completion must return quickly for incomplete local syntax such as `let a`,
  `let mut n`, `const N`, or `static VALUE`.
- Semantic tokens must never require a fresh full artifact for dirty text.
- Code actions for dirty documents should use parse diagnostics and lightweight
  fixes.
- Workspace refresh may rebuild broad state, but interactive requests should be
  able to ignore stale refresh results.
- Tests for dirty behavior should assert cache effects, not only user-visible
  output.

## Current First Milestone

The first stabilization pass focuses on making dirty and half-written source
cheap:

- keep semantic tokens lexical when a document is dirty
- keep code actions parse-only when a document is dirty
- avoid full analysis for incomplete binding-name completion
- preserve clean-artifact completion for valid body positions
- record selected analysis tiers so hot-path tests and verbose trace can assert
  behavior directly
- trace dirty navigation and signature-help requests as clean-semantic fallbacks
- trace interactive and diagnostics request latency under `trace=verbose`
- drop stale diagnostics tasks before analysis when a newer document generation
  already exists
- yield remaining diagnostics tasks to the next scheduler drain after an
  exceeded diagnostics budget
- keep `analysis::tests::dirty_cache` as the main regression target

## Later Milestones

1. Extend explicit request budgets beyond diagnostics-lane yielding into
   interactive tier selection and long request degradation.
2. Coalesce diagnostics and workspace refresh work by document generation.
3. Add cancellation or stale-generation checks before expensive analysis stages.
4. Move heavy analysis off the main dispatch path.
5. Add real-project LSP smoke coverage for `incubator/limine-smoke` and
   CoolPotOS-kern-style freestanding packages.
