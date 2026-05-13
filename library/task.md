# kernlib Tasks

## Test Migration

- Move library API behavior coverage from the compiler/tooling repository into
  `kernlib-test` as Kern code.
- Keep toolchain-only coverage in `kern`: package resolution, bundle injection,
  runtime/linker wiring, diagnostics, expected compile failures, stdout/stderr
  capture, and release packaging.
- `kernlib-test` now dogfoods first-class `#[test]` cases. Keep new library
  behavior coverage as ordinary test functions instead of hand-written `main`
  adapters.
- Dogfood gap: `craft test` still does not express expected compile failures,
  expected process aborts, or stdout/stderr assertions declaratively from Kern
  test roots.
- Dogfood gap: `base.test` equality helpers can force trait-method lowering for
  exact mutable slice types such as `&mut [T]`. Tests currently prefer boolean
  comparisons for mutable slices until the helper API or solver path handles
  that shape cleanly.
- Dogfood gap: `base.test` result and option assertions do not compose into a
  single fluent chain. Tests must call `.should_ok().sum(...).should_some()`
  instead of chaining directly from `Result[Option[T], E]`.
