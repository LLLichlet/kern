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

`base.sync` exposes memory-order constants and reusable synchronization
primitives without depending on hosted `std`.

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

- `Atomic[T]` and `atomic[T](value)` are available in `base.sync`.
- `SpinLock[T]` is available through `spin_lock[T](value)`. It intentionally
  uses closure-based access instead of returning a copyable guard value.
- `Once` is available through `once()`.

### 2. Generic IO Adapters

`base.io` has `Read`, `Write`, in-memory readers, in-memory writers, and the
common glue needed to assemble pipelines.

Planned additions:

- `copy`
- `copy_n`
- `LimitReader`
- `CountingWriter`
- `NullWriter`

Status:

- Copying and bounded copying are available as `Read` methods:
  `reader.copy_to(writer)` and `reader.copy_n_to(writer, limit)`.
- `LimitReader`, `CountingWriter`, and `NullWriter` are available in `base.io`.

### 3. Filesystem Safety Helpers

`std.fs` covers basic file, directory, metadata, and path operations. Programs
still need safer replacement-style writes.

Planned additions:

- `path.path().write_all_atomic_tmp(alloc, tmp_path, buf)`, using a
  caller-provided temporary path
- `path.path().write_all_atomic(alloc, buf)`, using an automatically generated
  same-directory temporary path

Status:

- Atomic replacement writes are available as `fs.Path` methods.
- `write_all_atomic` uses process identifiers and bounded collision retries for
  its generated temporary path.
- Windows rename now uses replace-existing semantics so replacement-style writes
  have the same public contract across supported hosted targets.
- `std.proc.process_id()` is available as the process-level primitive used by
  the generated temporary-path policy.

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

`base.test` has core freestanding assertions. The default test target style is
postfixed on the checked value and ends by reporting to a local test report:

```kern
let t = test.report(io.stderr())..&;

"42".parse[i32]().should_ok().eq(42).sum(@loc(), t);
value.is_valid().should().sum(@loc(), t);
```

The report owns its writer value, so `test.report(io.stderr())` is safe and does
not require exposing trait-object construction at the call site. `sum(@loc(), t)`
keeps source locations explicit without a caller-location attribute.

Planned additions:

- richer equality helpers once formatting can render more value categories
- optional runner-level test aggregation layered above `base.test`

Status:

- `test.report(writer)` builds a local reporter with no global or static test
  state.
- `bool.should()`, `?T.should_some/should_none`, and
  `T!E.should_ok/should_err` provide the ordinary assertion vocabulary.
- `parse[T]()` and `parse_radix[T](radix)` are the preferred integer parsing
  surface; the older concrete parse helpers remain internal implementation
  details during the transition.

## Current Execution Order

1. Land low-risk `std.time` and `std.term` convenience helpers.
2. Add generic `std.io` adapters with hosted runtime coverage.
3. Add atomic wrappers in `base.sync` with compile and IR coverage.
4. Add filesystem atomic-write helpers after the IO layer has enough shared
   copying primitives.
5. Continue with test ergonomics and higher-level synchronization once the new
   low-level APIs have enough real usage.
