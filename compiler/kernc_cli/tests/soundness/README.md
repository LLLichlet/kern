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
- `build-pass/`: program must compile successfully.
- `run-pass/`: program must compile and run successfully.

Leading comment directives:

```kern
// compile-args: --library-bundle std
// stderr: overlapping trait impls are not allowed
// stderr: global proofs
// exit: 0
```

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
