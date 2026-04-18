# kernc_lower

`kernc_lower` lowers resolved Kern programs into MAST.

This crate is where the compiler crosses from semantic analysis into the
lowering/codegen side of the pipeline.

## Role In The Architecture

The current split across crates is:

- `kernc_driver`: orchestration, incremental analysis, `Flow`
- `kernc_lower`: lowering from semantic world into MAST
- `kernc_mast`: the lowered monomorphized IR itself

`kernc_lower` is the bridge between the analysis side and the lowering side.

## Why Lower After `Flow`

Kern deliberately does not force all analysis through a Rust-style MIR layer.

Instead:

- `Flow` handles control-flow and dataflow analysis
- `kernc_lower` consumes semantically checked program structure
- MAST becomes the monomorphized lowering IR that feeds MIR construction and later backend work

That means lowering does not need to carry every analysis concern with it.
It can stay focused on:

- monomorphization
- reachable item emission
- closure lowering
- vtable/materialization details
- backend-friendly body shapes

## Practical Consequence

This architecture allows the compiler to:

- answer high-level analysis questions before lowering
- keep incremental behavior centered in the driver
- prune unreachable module-owned items before expensive backend work
- keep MAST compact and purpose-built

## Relationship To Reachability

`kernc_lower` already consumes reachability information from the driver to avoid
lowering dead private module-owned items. This is the current contract:

- analysis computes facts in `Flow`
- lowering consumes those facts

instead of recomputing the same logic inside MAST.

The same contract now also covers early optimization:

- `Flow` can mark dead pure initializers and dead pure assignments
- `Flow` can identify identifier uses that safely collapse onto immutable source bindings
- `Flow` can identify immutable local bindings that are safe to forward as pure values
- `Flow` can identify unused immutable pure bindings that can be omitted entirely
- the driver translates those facts into explicit lowering hints
- `kernc_lower` consumes the hints conservatively while keeping analysis logic
  out of MAST

In practice, lowering now applies those hints at explicit effect-only evaluation
sites:

- ordinary expression statements
- `for` init and post/latch clauses
- ignored `let` initializers such as `let _ = expr`

This keeps the optimization boundary understandable: `Flow` decides what is
dead or safely forwardable, and lowering only decides where those facts can be
materialized or omitted.
