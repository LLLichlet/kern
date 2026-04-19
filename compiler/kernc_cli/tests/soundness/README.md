# Kern Soundness Corpus

This directory is the beginning of a file-driven soundness suite for `kernc`.

The goal is not to copy Rust's test suite wholesale. Kern and Rust differ in
ownership, lifetime tracking, trait object design, closure representation, and
mutability semantics. But Rust's issue tracker and `tests/ui` corpus are still a
very strong source of *bug shapes*:

- coherence / overlapping impls
- orphan-rule escapes
- associated type projection ambiguity
- recursive or non-terminating proof search
- escaping stack closures / fat-pointer mismatches
- illegal mutability upgrades through pointer-like values

Each `.rn` file is a minimized regression seed. The harness lives in
[`soundness.rs`](../soundness.rs) and currently recognizes:

- `reject/`: program must be rejected by the compiler.
- `tree-reject/`: a case directory with `main.rn` plus helper package trees.
- `interface-reject/`: a case directory with `main.rn` plus helper interface packages.
- `known-bug-compile/`: currently compiles, but should not. The test fails once the bug is fixed.
- `known-bug-reject/`: currently rejects or ICEs, but should not. The test fails once the bug is fixed.
- `known-bug-timeout/`: currently hangs or times out. The test fails once the compiler stops timing out.
- `build-pass/`: program must compile successfully.
- `run-pass/`: program must compile and run successfully.
- `known-bug-run/`: currently runs with buggy output/exit status. The test fails once the behavior changes.

Leading comment directives:

```kern
// compile-args: --library-bundle std
// module-path: dep=dep
// module-interface-path: dep=iface
// stderr: overlapping trait impls are not allowed
// stderr: global proofs
// exit: 0
// timeout-ms: 2000
```

`tree-reject/` cases read directives from `main.rn`, copy the whole case directory
to a temporary workspace, and currently support relative `module-path` mappings
such as `dep=dep`.

`interface-reject/` cases do the same, but first compile helper packages into
temporary `kmeta` outputs and then mount them through `module-interface-path`.

Known-bug buckets are intentionally inverted:

- they pass while the bug still reproduces
- they fail once the compiler stops reproducing it

That makes them useful as a quarantine lane while a bug is still open. When one
starts failing, move it into `reject/`, `build-pass/`, or `run-pass/` with the
new expected behavior.

Recommended maintenance workflow:

1. Mine Rust's official `I-unsound` issues and `tests/ui` / `known-bug` corpus
   for *structural patterns*, not syntax.
2. Adapt only the parts that correspond to Kern semantics.
3. Reduce the example to the smallest Kern program that hits the same proof or
   representation failure mode.
4. Preserve a short comment at the top describing the soundness class and source
   issue or inspiration.
5. If Kern fixes the bug, move the case into the appropriate expected bucket and
   keep it in CI permanently.

The long-term target is two layers:

- curated regression seeds from real issues
- reducer/fuzzer-generated cases that shrink back into this corpus

Useful upstream sources:

- Rust compiler test harness guide:
  <https://rustc-dev-guide.rust-lang.org/tests/compiletest.html>
- Rust `I-unsound` issue label:
  <https://github.com/rust-lang/rust/issues?q=is%3Aissue+label%3AI-unsound>
- Rust `tests/ui` corpus:
  <https://github.com/rust-lang/rust/tree/master/tests/ui>
