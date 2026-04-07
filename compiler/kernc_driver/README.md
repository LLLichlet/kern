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

The intended split is:

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
- what concrete control/data constructs should LLVM lowering see?

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

## Why `Flow` Still Lives In `kernc_driver`

`Flow` is intentionally treated as a first-class layer, but it is not split into
`kernc_flow` yet.

That is deliberate.

Right now `Flow` is tightly coupled to:

- semantic ownership and definition lookup
- driver-level incremental staging
- warning emission and analysis artifact shaping
- reachability-driven lowering decisions

Splitting it too early would mostly move files across crates while preserving the
same dependencies, which adds ceremony without improving the design.

The current threshold for extracting a separate crate is higher:

- `Flow` has a stable public contract independent of driver staging
- multiple crates need to build or consume `Flow` directly
- the split removes real dependency pressure instead of renaming it

Until then, keeping `Flow` inside `kernc_driver` keeps the architecture honest:
analysis stays explicit, close to the incremental engine, and easy to evolve.

## Current Direction

The long-term expectation is:

- keep strengthening `Flow` as Kern's analysis IR
- keep MAST focused on lowering and code generation
- let lowering consume explicit `Flow`-derived optimization hints instead of
  rebuilding dataflow logic locally
- add more dataflow queries on top of `Flow` instead of introducing MIR only to
  imitate another compiler architecture
