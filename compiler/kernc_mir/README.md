# kernc_mir

`kernc_mir` defines Kern MIR, the mid-level IR layer that sits between
monomorphized lowering and backend-oriented code generation.

## Current Scope

This first implementation is intentionally incremental.

Today it provides:

- a dedicated MIR crate and public IR data structures
- explicit basic blocks and terminators
- module-level metadata needed by backend-oriented lowering
- explicit locals, places, operands, structured assignments, and structured
  rvalues for local copies, direct/indirect calls, aggregates, projections,
  scalar ops, casts, and basic address/load forms
- explicit memory-effect instructions for `memcpy`/`memmove`/`memset`
- a default MIR pass pipeline with compiler-owned local copy propagation,
  jump threading, branch folding, and unreachable-block pruning
- a built-in verifier that rejects invalid block/local/place references
- workload statistics suitable for compiler reporting and future optimization work

Lowering from MAST into MIR lives in `kernc_mir_lower`.
MIR itself does not carry opaque source-level fallback expressions.
If `kernc_mir_lower` cannot lift a MAST form into first-class MIR, lowering
fails immediately instead of smuggling source IR across the boundary.

The important first step is to make control-flow ownership explicit in a stable
mid-level layer instead of continuing to push every optimization directly into
MAST or LLVM.

## Position In The Pipeline

The intended long-term shape is:

1. AST
2. semantic analysis
3. Flow facts
4. monomorphization
5. MIR construction and optimization
6. backend-oriented lowering/codegen

MIR is still derived from MAST today, but the boundary is explicit:
`kernc_mir` owns MIR itself, and `kernc_mir_lower` owns the source-to-MIR
translation policy.

## Design Principle

MIR should own:

- CFG structure
- transform-friendly function bodies
- mid-level inlining and simplification
- future cross-CGU summaries

MAST should remain focused on:

- monomorphized emitted items
- backend-facing lowering
- predictable code generation

That split keeps the optimization story healthy and gives Kern a proper home for
whole-program and ThinLTO-oriented work later.
