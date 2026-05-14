# Roadmap Notes

This file tracks known follow-up work that is still useful but should not live
as loose task files in the repository root.

## Library Test Migration

- Move remaining library-behavior regression cases from
  `compiler/kernc_cli/tests/stdlib` into Kern test packages under `library/`.
- Keep toolchain-only coverage in `kern`: package resolution, bundle
  injection, runtime and linker wiring, diagnostics, expected compile
  failures, stdout and stderr capture, and release packaging.
- Keep new library behavior coverage as first-class `#[test]` functions in
  `library/kernlib-test`.

## Test Dogfooding Gaps

- Let `craft test` express expected compile failures, expected process aborts,
  and stdout or stderr assertions declaratively from Kern test roots.
- Improve `base.test` equality helpers or solver lowering so exact mutable
  slice types such as `&mut [T]` can use the ordinary fluent assertions.
- Let `base.test` result and option assertions compose into a single fluent
  chain for values such as `Result[Option[T], E]`.
