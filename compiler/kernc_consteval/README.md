# kernc_consteval

Shared compile-time evaluation vocabulary for Kern.

This crate owns the compile-time value model, local place representation, pure
value projection helpers, and the host callback surface for explicitly hosted
compile-time execution.

This crate deliberately does not own a private AST-like const IR. Compile-time
evaluation must eventually consume Kern's shared typed body / middle IR so the
compiler does not maintain two independent semantics for calls, control flow,
places, aggregates, and pointer behavior.

The current evaluator driver still lives in `kernc_sema`; this crate is the
shared, AST-free core state and value layer that can be reused when that driver
moves behind a proper middle-IR boundary.
