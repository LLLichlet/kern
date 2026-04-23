# kernc_driver

`kernc_driver` is the orchestration crate for the Kern compiler frontend pipeline.

It owns:

- incremental frontend caching
- structure / import / type-resolution staging
- analysis artifacts used by CLI and LSP
- the `Flow` analysis layer
- warning emission for unused items, unused bindings, and dead stores

## Why `Flow` Exists

Kern does not treat Rust-style MIR as a default target shape.

Instead, the driver builds a dedicated `Flow` layer for analysis after semantic
resolution but before lowering to MAST. The goal is not to model a low-level
execution IR. The goal is to provide a stable, query-friendly representation for
control-flow and dataflow questions.

`Flow` currently provides:

- explicit `FlowNodeId` and `FlowBindingId`
- per-owner CFGs
- CFG nodes that can point back to concrete AST node ids
- explicit node facts and node transfer summaries
- explicit node effect summaries
- explicit definition facts, including copy-source candidates
- explicit use-def relationships for node-local uses
- explicit def-use relationships for concrete definitions
- explicit resolved-use classification (`missing` / `unique` / `ambiguous`)
- explicit single-source-use summaries for uniquely resolved local values
- control-region summaries
- liveness
- reaching definitions
- dead-store detection
- conservative lowering hints for dead pure stores, immutable local copy
  propagation, and pure value forwarding
- reachability information for pruning and warnings

Those lowering hints are now intentionally grouped by role:

- `elision`: dead pure initializers, dead pure assignments, removable bindings
- `forwarding`: immutable copy sources, pure value forwarding, identifier collapse

This makes `Flow` a Kern-specific analysis IR.

## `Flow` vs MAST

The current split is:

- `Flow`: analysis-oriented, query-driven, incremental-friendly
- `MAST`: lowering-oriented, monomorphized, codegen-friendly

`Flow` answers questions like:

- which binding is live here?
- what does this node read, write, kill, or generate?
- what kind of definition is this, and is it a pure local copy?
- which definitions reach this concrete use?
- which concrete uses belong to this definition?
- is this use unresolved, uniquely sourced, or ambiguous?
- if this use is unique, what exact local definition and copy-source does it come from?
- is this definition node pure enough to let lowering prune it safely?
- which assignment is dead?
- which private item is reachable?
- which body-only edit invalidates which facts?

`MAST` answers different questions:

- what exact monomorphized function or global should be emitted?
- what concrete monomorphized structure should MIR construction and later backend lowering see?

This keeps the analysis layer and the lowering layer separate instead of forcing
one IR to serve both jobs poorly.

## Incremental Design

The driver is intentionally query-driven without hiding too much behavior behind
implicit invalidation magic.

The main stages are:

1. Parse and collect structure.
2. Resolve imports.
3. Resolve types.
4. Build analysis artifacts.
5. Lower only reachable module-owned items.

For body-only edits, the driver can reuse earlier stages and recompute only the
parts that actually depend on changed function bodies. `Flow` is designed to fit
that model directly.

That reuse is now modeled around an explicit incremental driver family key.
Invocation-only settings such as output paths can vary per compile, while
frontend and staged semantic caches stay shared as long as the analysis-shaping
options remain the same. This is the contract used by both `craft` and
`kern-lsp`.

## `kernc_driver` vs `kernc_flow`

`Flow` now lives in the dedicated [`kernc_flow`](../kernc_flow/README.md)
crate, while `kernc_driver` still owns the orchestration around it.

The split is:

- `kernc_flow`: shared `Flow` data structures and lowering-hint containers
- `kernc_driver`: `Flow` construction, staged analysis, warning emission,
  reachability, and reuse/incremental policy

This keeps the reusable `Flow` surface in its own crate without pretending that
analysis orchestration became backend-agnostic. The driver still decides when
`Flow` is built, which facts are surfaced to CLI/LSP, and how those facts feed
lowering and warnings.

## Current Direction

The repository currently uses this division of labor:

- `Flow` is Kern's source-near analysis IR
- MIR is Kern's transform-oriented mid-level IR
- MAST stays focused on backend-oriented lowering and code generation
- lowering and MIR construction consume explicit `Flow`-derived facts instead of
  rebuilding dataflow logic locally
- mid-level optimization work belongs in MIR rather than collapsing `Flow` and
  `MAST` into one compromise layer
