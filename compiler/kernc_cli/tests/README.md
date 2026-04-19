# Kern CLI Integration Tests

The integration suite for `kernc` lives in this directory and is organized by behavior area:

- `anonymous_aggregates.rs`: anonymous struct/union/enum coverage.
- `atomics.rs`: atomic intrinsics and ordering validation.
- `collections.rs`: hosted `base.coll` execution coverage.
- `filesystem.rs`: hosted `std.fs` execution coverage.
- `regressions.rs`: targeted compiler/runtime regressions.
- `soundness.rs`: file-driven type-system and proof-soundness corpus.
- `stdlib.rs`: standard library, driver, and platform runtime coverage.
- `traits.rs`: trait bounds, supertraits, and trait-object behavior.

Common harness code lives in [`support/mod.rs`](./support/mod.rs). Add new helpers there instead of copying temporary-file, compiler-invocation, or hosted-run boilerplate into individual test files.

Typical maintenance commands:

```bash
cargo test -p kernc_cli --tests
```

```bash
cargo test -p kernc_cli --test regressions
```

```bash
python3 -m ops ci kernc-tests --mode smoke
```

Available test layers:

- `smoke`: fast compiler, runtime, and platform-critical coverage.
- `hosted`: slower hosted standard-library coverage (`collections` and `filesystem`).
- `all`: `smoke` plus `hosted`.

Prefer adding a test to the narrowest existing suite that matches the behavior under change. If a new area becomes large enough to deserve its own suite, create another file in this directory and keep the support logic shared through `support/mod.rs`.
