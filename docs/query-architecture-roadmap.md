# Kern Query Architecture Roadmap

## Why This Exists

`kernc` currently has a solid batch compiler shape, but `kern-lsp` is still forced to
re-run coarse whole-root analysis and reconstruct editor semantics from a mix of
compiler output and token heuristics.

That gap is now the main reason for:

- visible latency on larger files
- false or stale editor diagnostics when semantic input shifts
- incomplete semantic coloring for local bindings and pattern payloads
- weak navigation coverage for more complex access forms

Kern's philosophy is to prefer a healthy rebuild over local patch stacks. This
document is the reference plan for moving the compiler and language server
toward a query-driven incremental architecture.

## Current Diagnosis

The current architecture is strong at:

- explicit pass boundaries
- stable internal identity through `NodeId`
- centralized registries for types and semantic state
- keeping the frontend in Rust and shared with tooling

The current architecture is weak at:

- query-level reuse
- dependency-driven invalidation
- compiler-authoritative IDE semantic output
- cancellation and stale-request dropping
- persistent semantic indexing across requests

Today, the LSP cache is still an `AnalysisArtifact` cache, not a semantic query
cache. That helps repeated read-only requests, but it does not change the
fundamental unit of invalidation.

## End State

The target architecture is:

1. A compiler-owned semantic database, not an LSP-owned reconstruction layer.
2. Fine-grained queries with explicit dependencies and revision-aware invalidation.
3. One semantic truth source for diagnostics, navigation, highlighting, tokens,
   completion, and rename.
4. Request scheduling that can cancel or drop stale work before it harms editor
   responsiveness.

## Target Query Layers

### Layer 1: Source and Syntax

- file text
- file parse tree
- syntax errors
- lightweight syntax facts used by completion and token fallback

This layer must be file-granular and cheap to invalidate.

### Layer 2: Module and Item Structure

- module root resolution
- module membership
- import graph
- item tree
- top-level declarations and signatures

This layer must avoid re-checking function bodies when only module shape matters.

### Layer 3: Semantic Bodies

- name resolution per body
- type-of-expression
- member resolution per access site
- pattern binding semantics
- trait/impl lookup

This is where current whole-pipeline invalidation is too coarse. Bodies need
their own cache and dependency edges.

### Layer 4: IDE Semantic Index

- symbol definitions
- references
- hover payloads
- semantic token classes
- rename targets
- per-file semantic slices

This layer must be compiler-authoritative. The LSP should consume it, not
reconstruct it from ad hoc pieces.

## Migration Stages

### Stage 0: Stop Digging the Hole Deeper

- Keep tactical fixes minimal.
- Prefer changes that create reusable semantic infrastructure.
- Avoid adding new lexer-only heuristics unless they are explicit fallback.

### Stage 1: Unified Semantic Index

Goal: create one compiler-side semantic record model that can represent:

- definitions
- references
- symbol kind
- modifiers such as mutability and static-ness

The first implementation may still be produced by the existing pass pipeline,
but it must become the interface boundary that the LSP reads from.

This stage is intentionally transitional: it does not solve incremental
compilation yet, but it removes the current split between symbols, hovers,
reference lists, and token heuristics.

### Stage 2: Split Global Structure from Body Checking

Goal: stop redoing full semantic work when only a subset of bodies changed.

Refactor the existing pipeline so that:

- module loading and item collection produce reusable structure artifacts
- import resolution produces reusable module facts
- item signatures and type headers are cached separately from body checking
- body checking runs per function or per body-like owner

This is the first major performance step.

### Stage 3: Introduce Explicit Query APIs

Goal: replace "run the pipeline and then inspect context" with named queries.

Examples:

- `parse(file)`
- `module_root(file)`
- `item_tree(module)`
- `imports(module)`
- `item_signature(def)`
- `type_of_expr(expr)`
- `resolve_name(use_site)`
- `resolve_member(access_site)`
- `semantic_index(file)`

The implementation can remain internal at first. The key is that every result
must have a stable identity, an input boundary, and invalidation rules.

### Stage 4: Revisioned Incremental Engine

Goal: attach dependency tracking and invalidation to the query layer.

Requirements:

- stable revision counter
- file content hashing
- dependency graph between queries
- partial invalidation on file edits
- reuse of unaffected query results

This is the stage where Kern reaches the class of architecture used by the best
interactive compilers and IDE backends.

### Stage 5: LSP Scheduler and Cancellation

Goal: make responsiveness match the new compiler core.

Requirements:

- request generations
- stale result dropping
- actual `$/cancelRequest` support
- background analysis scheduling
- separation between urgent queries and full diagnostics refresh

Without this stage, a better semantic core will still feel slower than it is.

## Concrete Work Breakdown

### Track A: Compiler Semantic Model

1. Introduce a unified semantic index type.
2. Record local bindings, parameters, type parameters, items, and references in
   that index.
3. Port semantic tokens to consume that index first.
4. Port definition/reference/rename to consume the same index.

### Track B: Compiler Refactor

1. Extract module and item collection artifacts from the current pass chain.
2. Make body checking callable per owner instead of only `check_all()`.
3. Introduce explicit query entry points over those artifacts.
4. Add revision-aware invalidation.

### Track C: LSP Runtime

1. Separate document storage from semantic scheduling.
2. Add generation IDs to analysis requests.
3. Drop stale work and stale publish results.
4. Add cancellation support.

## Non-Goals During Migration

- Do not rewrite everything into a framework.
- Do not move semantic logic into the extension host.
- Do not build a second frontend just for tooling.
- Do not keep expanding heuristic-only semantic token logic.

## Immediate Next Steps

The first construction step starts in Stage 1:

1. add a compiler-owned unified semantic index model
2. record local binding and parameter definitions explicitly
3. expose semantic entries from `AnalysisArtifact`
4. switch LSP semantic token classification to prefer that compiler-owned index

That gives Kern a clean architectural seam to build on, while still shipping
real editor improvements during the migration.
