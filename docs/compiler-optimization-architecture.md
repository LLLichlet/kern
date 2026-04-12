# Compiler Optimization Architecture

This document defines the optimization direction that grows out of recent Kern
compiler work on `#[inline]`, codegen-unit partitioning, SIMD-heavy JSON
parsing, and runtime/linkage cleanup.

The goal is not to add "more optimization" in the abstract. The goal is to make
Kern's optimization pipeline healthy:

- explicit about boundaries
- owned by the compiler rather than by accidental LLVM behavior
- compatible with no-std, kernels, embedded, and hosted builds
- scalable from single-object freestanding programs to multi-CGU optimized builds

## Why This Needs A Formal Architecture

Two recent experiments exposed the current boundary problems clearly.

### 1. LLVM-side `alwaysinline` is not a healthy primary strategy

Trying to force `alwaysinline` through LLVM's C API module/function pass entry
points is not a stable compiler architecture.

Observed failures:

- running function passes such as `instcombine` on declarations can ICE inside LLVM
- module-level `AlwaysInliner` can delete `alwaysinline` functions in a single CGU
  even when other CGUs still need that symbol
- the behavior is owned by LLVM's pass pipeline shape, not by Kern's semantics

Conclusion:

- LLVM remains the backend optimizer
- Kern must own semantic inlining policy before LLVM IR emission
- LLVM-side inlining should be a later backend optimization layer, not the first
  or only implementation of Kern's inline contract

### 2. "Anything non-internal is a CGU root" is too coarse

The previous CGU partition rule used:

- `linkage != Internal` => partition root

This is too broad.

It confuses:

- ABI-facing exported symbols
- generic/link-once instantiations
- package-internal implementation details that happened to be emitted as external

That makes cross-CGU planning noisy and undermines later WPO design.

Conclusion:

- only true externally visible ABI items should anchor CGU roots
- link-once/generic instantiations are shareable implementation artifacts, not
  optimization roots
- private top-level items should not default to external linkage

## Current Pipeline

Today the important stages are roughly:

1. AST + semantic analysis
2. flow analysis / reachability / source-level optimization hints
3. lowering into monomorphized MAST
4. LLVM IR codegen
5. LLVM optimization and final object emission

This split already has value:

- semantic checks remain source-aware
- flow analysis already computes CFG/dataflow facts
- MAST is concrete, monomorphized, and easy to emit

But it also has a hard limit:

- flow is an analysis/facts layer, not a durable transform IR
- MAST is already too close to codegen concerns to become the long-term home of
  serious mid-level optimization

## What Each Layer Should Own

Kern should keep three distinct layers rather than forcing one structure to do
every job.

### Flow

Flow should remain the source-near control/dataflow analysis layer.

It should own:

- reachability
- dead-store and forwarding facts
- diagnostics-oriented control facts
- source-driven body pruning hints

It should not become the long-term mutation surface for whole-program or heavy
mid-level optimization.

### MAST

MAST should remain the low-level monomorphized lowering target used to drive
backend emission.

It should own:

- concrete monomorphized items
- ABI-visible item metadata
- low-level expression forms already close to codegen
- predictable emission into LLVM IR

It should not be stretched into the primary optimization IR for cross-function
and cross-CGU work. Once that happens, every optimization starts fighting
codegen concerns too early.

### A New Mid-Level IR

Kern should gain a real mid-level IR between flow and final MAST/codegen.

This IR does not need to be maximally academic. It does need to satisfy the real
requirements that the current pipeline lacks:

- explicit CFG
- explicit basic blocks
- explicit terminators
- typed locals/temporaries
- per-function mutation as a first-class operation
- stable function summaries for later CGU/LTO planning

This is the right place for:

- inlining
- CFG simplification
- scalar simplification
- branch cleanup
- constant propagation beyond source-local facts
- devirtualization and call-graph reasoning later
- cross-CGU import/export summaries later

## Do We Need MIR?

Yes.

More precisely:

- `flow + MAST` is enough for early language bring-up and some local cleanup
- it is not enough for a serious optimization architecture
- the recent `inline` work is useful, but it is also proof that we are
  currently doing mid-level work in the wrong layer because there is no proper
  layer yet

Kern should therefore add a MIR-like stage. It does not need Rust's exact MIR.
It does need a CFG-based, typed, transform-friendly IR with clear ownership.

## Proposed Mid-Level IR Shape

The initial MIR should be pragmatic.

Recommended properties:

- monomorphized
- function-local
- block-based
- typed
- explicit `call`, `branch`, `switch`, `return`, `unreachable`
- locals identified separately from frontend symbols
- side-effecting operations explicit
- no hidden source block result semantics

Recommended non-goals for v1:

- no full SSA requirement on day one
- no need to model every backend detail
- no need to replace MAST immediately

A healthy first design is:

- build MIR after monomorphization
- run MIR optimization passes
- lower optimized MIR into MAST or directly into codegen-oriented structures

Whether MAST survives long-term as a separate layer can be decided later. It is
not necessary to answer that before introducing MIR.

## Linkage And CGU Policy

The compiler should use this boundary model.

### Linkage policy

For non-generic monomorphic items:

- `extern` => `External`
- explicit `export_name(...)` => `External`
- `pub` top-level items => `External`
- private top-level items => `Internal`

For generic instantiated bodies:

- use `LinkOnceOdr`

This keeps source visibility and ABI/export intent much closer to actual object
linkage behavior.

### CGU roots

CGU roots should be only:

- `External` functions/globals with bodies/initializers

CGU roots should not be:

- `Internal` helpers
- `LinkOnceOdr` generic instantiations

Internal or link-once items may still be promoted/imported as needed across
units, but they should not anchor the partition graph.

### Why this matters

This gives Kern a clean optimization boundary:

- roots represent outward-facing anchors
- everything else is implementation detail
- partitioning becomes quieter and more predictable
- later WPO import/export summaries have a clean semantic base

## Inlining Strategy

Inlining should become layered.

### Current step

The current lowering-owned `#[inline]` handling is the correct short-term
direction because:

- it is compiler-owned
- it preserves semantics explicitly
- it does not depend on LLVM pass quirks

This pass can continue to grow carefully as a stopgap.

### Future step

Once MIR exists, the semantic inlining contract should move there.

That gives us:

- proper CFG rewriting
- better handling of multi-block control flow
- better summary-driven heuristics
- one place for call graph analysis

`#[inline]` should be treated as:

- a strong semantic request owned by Kern
- validated and applied in Kern IR first
- then optionally reinforced in LLVM IR as a backend hint, not as the source of truth

## ThinLTO / Full WPO Direction

Kern should support optimization at three different scales.

### 1. Intra-function and intra-package optimization

Owned by the Kern frontend/mid-level pipeline:

- MIR scalar/CFG passes
- MIR inlining
- better linkage and CGU planning

This is the first milestone and should land before depending on LLVM ThinLTO.

### 2. Cross-CGU optimization inside one compile invocation

This should be driven by MIR summaries and explicit import/export planning.

Needed pieces:

- per-function size/cost summaries
- call graph
- side-effect / inline eligibility summaries
- CGU-local ownership plus import candidates

This is Kern-owned WPO before LLVM sees the final modules.

### 3. LLVM ThinLTO / full-module LTO

After Kern-owned summaries and CGU policy are healthy, add backend LTO modes.

Recommended direction:

- support ordinary multi-object non-LTO builds first
- add optional full LTO mode as the first explicit whole-program baseline
- add ThinLTO only after Kern-owned MIR summary/import-export machinery exists

LLVM should then optimize on top of a healthy Kern partition, not compensate for
an unhealthy one.

## Cross-Package Story

Kern should separate:

- cross-CGU optimization inside one package build
- cross-package optimization across package boundaries

The second problem is harder and should be explicit.

Reason:

- Kern packages are not just LLVM modules
- source/package structure matters
- `craft` should own policy and caching

A healthy staged plan is:

1. solve MIR + CGU + WPO inside one package build
2. add package-produced optimization metadata sidecars later
3. optionally add LLVM bitcode sidecars for ThinLTO-capable release pipelines

This avoids prematurely hard-coding the package system around LLVM details.

## Recommended Near-Term Plan

### Phase A: stabilize current architecture

Done or in progress:

- compiler-owned `#[inline]` lowering pass
- naked functions force `noinline`
- private items stop defaulting to external linkage
- only `External` items anchor CGU roots

### Phase B: introduce MIR

Next:

- design MIR data structures
- build MIR from monomorphized functions
- keep flow as a fact producer
- keep MAST/codegen as the backend-facing layer

### Phase C: migrate optimizations into MIR

Move or reimplement:

- `inline`
- CFG simplification
- canonical return/branch normalization
- local constant propagation
- branch folding

### Phase D: summary-driven CGU planning

Then:

- derive CGU planning from MIR call graph and summary data
- support import/promote decisions intentionally
- stop using purely local workload heuristics as the only planner signal

### Phase E: ThinLTO / WPO integration

Finally:

- add ThinLTO-capable backend mode
- add full-LTO mode where appropriate
- keep non-LTO multi-object builds as the baseline, not as an afterthought

## Non-Goals

Kern should explicitly avoid these unhealthy directions:

- letting LLVM pass-pipeline accidents define language-level optimization policy
- turning MAST into a half-codegen, half-mid-level compromise blob
- making `craft` or package metadata responsible for compiler semantic decisions
- hiding CGU/LTO semantics behind fuzzy runtime/library flags
- chasing performance only through backend attributes without owning the IR story

## Summary

The architectural direction is:

- keep flow
- keep MAST for backend-oriented lowering/emission
- add a real MIR
- move serious mid-level optimization ownership into MIR
- define CGU roots by true external visibility
- treat LLVM ThinLTO/full LTO as backend layers on top of a healthy Kern-owned optimization model

That is the path from today's targeted fixes to a compiler that can compete on
code quality without losing Kern's explicitness or freestanding-first design.
