# kernc_mast

`kernc_mast` defines MAST, the monomorphized abstract syntax tree used by Kern
after semantic analysis and lowering.

MAST is one of the distinctive parts of the compiler architecture.

## What MAST Is

MAST is a lowering IR for code generation.

It is:

- monomorphized
- explicit about emitted items
- close to the needs of later MIR/backend lowering
- simpler than the original typed AST for backend work

It is not meant to be the primary home for every high-level analysis.

## Why Not Use MAST For Everything

Kern separates two concerns:

- analysis
- lowering

High-level analyses such as liveness, dead-store detection, reachability, and
incremental body-only recomputation fit better on `Flow`, which keeps stable
binding and node identities.

MAST comes later and is optimized for different concerns:

- concrete emitted item sets
- monomorphized function bodies
- backend-friendly expression and statement shapes

That separation keeps MAST focused and avoids turning it into a catch-all IR.

## Position In The Pipeline

The current pipeline shape is:

1. AST
2. semantic resolution
3. `Flow` analysis
4. MAST lowering
5. MIR construction and optimization
6. backend lowering/codegen

In that model, MAST is not competing with `Flow`.
It follows `Flow`.

## Design Principle

If a feature is mainly about:

- warnings
- reachability
- dataflow
- incremental queries

it probably belongs in `Flow`.

If a feature is mainly about:

- monomorphized emitted items
- backend lowering
- concrete runtime layout of lowered bodies

it probably belongs in MAST.
