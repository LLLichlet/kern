# Kern CLI Integration Tests

The integration suite for `kernc` lives in this directory and is organized by behavior area:

- `anonymous_aggregates.rs`: anonymous struct/union/enum coverage.
- `atomics.rs`: atomic intrinsics and ordering validation.
- `collections.rs`: collection-related compile diagnostics that require the
  compiler harness.
- `regressions.rs`: targeted compiler/runtime regressions.
- `soundness.rs`: file-driven type-system and proof-soundness corpus.
- `stdlib.rs`: kernlib bundle consumption, driver, hosted runtime, and platform
  harness coverage.
- `traits.rs`: trait bounds, supertraits, and trait-object behavior.

Common harness code lives in [`compiler/kernc_cli/src/test_support.rs`](../src/test_support.rs). Add new helpers there instead of copying temporary-file, compiler-invocation, or hosted-run boilerplate into individual test files.

Typical maintenance commands:

```bash
cargo test -p kernc_cli --tests
```

```bash
cargo test -p kernc_cli --test regressions
```

```bash
cargo run -p kernworker -- ci kernc-tests --mode smoke
```

Available test layers:

- `smoke`: fast compiler, runtime, and platform-critical coverage.
- `hosted`: slower hosted runtime and platform harness coverage.
- `all`: `smoke` plus `hosted`.

Prefer adding a test to the narrowest existing suite that matches the behavior
under change. Pure library behavior belongs in the Kern test packages under
`library/`, not in this Rust compiler harness. If a new compiler or runtime
area becomes large enough to deserve its own suite, create another file in this
directory and keep the support logic shared through the `kernc_cli::test_support`
module.
