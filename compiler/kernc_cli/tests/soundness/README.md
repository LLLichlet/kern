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

Each `.kn` file is a minimized regression seed. The harness lives in
[`soundness.rs`](../soundness.rs) and currently recognizes:

- `reject/`: program must be rejected by the compiler.
- `tree-reject/`: a case directory with `main.kn` plus helper package trees.
- `interface-reject/`: a case directory with `main.kn` plus helper interface packages.
- `known-bug-compile/`: currently compiles, but should not. The test fails once the bug is fixed.
- `known-bug-reject/`: currently rejects or ICEs, but should not. The test fails once the bug is fixed.
- `known-bug-timeout/`: currently hangs or times out. The test fails once the compiler stops timing out.
- `build-pass/`: program must compile successfully.
- `run-pass/`: program must compile and run successfully.
- `known-bug-run/`: currently runs with buggy output/exit status. The test fails once the behavior changes.

Expected reject/build/run buckets also reject internal compiler failure output by
default. A case must not print `panicked at`, `Kern Compiler Internal Error`, or
`LLVM IR Verification Failed` unless it is explicitly quarantined in a
`known-bug-*` bucket.

Leading comment directives:

```kern
// compile-args: --library-bundle std
// module-path: dep=dep
// module-interface-path: dep=iface
// stderr: overlapping trait impls are not allowed
// stderr: global proofs
// stderr-not: LLVM IR Verification Failed
// exit: 0
// timeout-ms: 2000
```

Use `stderr-not:` when a case must fail in a specific compiler layer rather than
fall through to a later generic failure mode.

`tree-reject/` cases read directives from `main.kn`, copy the whole case directory
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

## Fake-Reflection / Projection Invariants

Fake-reflection bugs happen when one query path treats an open associated-type
projection as if a blanket impl had globally proved an equality such as
`T.Trait.Arg.Assoc = Arg`, even though a more-specific impl can shadow that
blanket result after substitution. These are soundness bugs because they can
turn one concrete representation into another through trait/projection proof
search.

Maintain these invariants when changing trait solving, projection
normalization, method lookup, trait-object construction, or supertrait upcasts:

- Direct proof search, projection normalization, bound-method lookup,
  trait-object construction, default-method dispatch, and supertrait upcast
  must use the same specificity frontier. A shadowed generic impl must not be
  consulted after a more-specific impl matches.
- Open projections may use explicit in-scope associated-type equalities, but
  they must not reverse-solve missing generic arguments through global impl
  selection. Global impl projection is only sound once the projection target,
  trait arguments, and associated arguments are fully concrete.
- Trait objects only carry associated types that are written on the object or
  inherited through a validated upcast path. A bare trait object must not infer
  missing associated bindings from the concrete receiver's impl.
- `Self.Trait.Assoc` in trait methods and default methods is ordinary
  projection syntax. It must not bypass the checks used by `T.Trait.Assoc`.
- Generic associated type arguments are substitutions into the selected
  associated body. They are not a second opportunity to pick a shadowed blanket
  impl.

When a fake-reflection issue is reported, add at least one minimized
`reject/specialization` case for the exact query path involved. Prefer covering
the matrix of direct proof, projected return type, bound method, trait object,
supertrait upcast, default method, and const-generic variants instead of only
the originally reported surface syntax.

Useful upstream sources:

- Rust compiler test harness guide:
  <https://rustc-dev-guide.rust-lang.org/tests/compiletest.html>
- Rust `I-unsound` issue label:
  <https://github.com/rust-lang/rust/issues?q=is%3Aissue+label%3AI-unsound>
- Rust `tests/ui` corpus:
  <https://github.com/rust-lang/rust/tree/master/tests/ui>
