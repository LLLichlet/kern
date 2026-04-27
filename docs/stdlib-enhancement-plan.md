# Standard Library Enhancement Plan

This note tracks the current library gaps that materially affect everyday Kern
programming. It is a maintainer planning document, not a public compatibility
promise.

## Goals

- Keep `base` freestanding and allocation-explicit.
- Keep `std` practical for hosted programs without hiding provider boundaries.
- Add small, testable APIs before larger policy-heavy abstractions.
- Treat compile failures found while exercising promised syntax as compiler
  bugs, then fix them before moving on.

## Priority Batches

### 1. Synchronization

`std.sync` currently exposes memory-order constants, while users still have to
call atomic intrinsics directly.

Planned additions:

- `Atomic[T]` as the single long-term atomic storage shape
- `atomic[T](value)` construction
- const-generic ordering methods for load, store, exchange, compare-and-swap,
  and common integer read-modify-write operations
- `SpinLock[T]` with closure-based `with_lock` and `try_with_lock`
- `Once` with `call_once` and `try_call_once`
- later: payload-carrying `OnceCell[T]` once initialization storage has a
  stable library shape

Status:

- `Atomic[T]` and `atomic[T](value)` are available in `std.sync`.
- `SpinLock[T]` is available through `spin_lock[T](value)`. It intentionally
  uses closure-based access instead of returning a copyable guard value.
- `Once` is available through `once()`.

### 2. Generic IO Adapters

`std.io` has `Reader`, `Writer`, in-memory readers, and in-memory writers. It
still lacks the common glue needed to assemble pipelines.

Planned additions:

- `copy`
- `copy_n`
- `LimitReader`
- `CountingWriter`
- `NullWriter`

### 3. Filesystem Safety Helpers

`std.fs` covers basic file, directory, metadata, and path operations. Programs
still need safer replacement-style writes.

Planned additions:

- `write_all_atomic_tmp(alloc, path, tmp_path, buf)`, using a caller-provided
  temporary path
- later: automatic temporary-path generation once process identifiers or random
  bytes are available in `std`

Status:

- `write_all_atomic_tmp` is available in `std.fs`.
- Windows rename now uses replace-existing semantics so replacement-style writes
  have the same public contract across supported hosted targets.

### 4. Time Convenience

`std.time` has monotonic `Instant`, `Duration`, and millisecond sleep. It needs
the small helpers users expect in benchmark and CLI code.

Planned additions:

- `sleep_secs`
- `sleep_micros`
- `sleep_nanos`
- `Duration` equality, ordering, and hashing
- `Instant` equality and ordering

### 5. Test Ergonomics

`std.test` has core assertions, but diagnostics are still sparse.

Planned additions:

- message-bearing assertion variants
- option/result predicate assertions
- richer equality helpers once formatting can render more value categories

Status:

- Message-bearing variants are available for equality and option/result helpers.
- Option/result predicate assertions are available as `assert_some`,
  `assert_none`, `assert_ok`, and `assert_err`.

## Current Execution Order

1. Land low-risk `std.time` and `std.term` convenience helpers.
2. Add generic `std.io` adapters with hosted runtime coverage.
3. Add atomic wrappers in `std.sync` with compile and IR coverage.
4. Add filesystem atomic-write helpers after the IO layer has enough shared
   copying primitives.
5. Continue with test ergonomics and higher-level synchronization once the new
   low-level APIs have enough real usage.
