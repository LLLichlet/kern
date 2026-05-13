# Range, Pattern, Slice, and Match Unification Plan

This plan records the design direction for removing the current hard-coded
range/match/slice split and replacing it with one orthogonal model.

## Current State

- Done: match alternatives now have an internal `Pattern -> Bind` model.
- Done: one `match` arm with multiple alternatives must produce the same bind
  shape.
- Done: bind environments are represented internally as `void` or
  `struct { name: T, ... }`, with fields canonicalized by name.
- Not done: range syntax is not yet an expression-level value.
- Not done: range forms are not yet canonical builtin type families.
- Not done: slice bounds still use dedicated syntax paths.
- Not done: match range patterns are still lowered through a range-specific
  hard-coded path.
- Not done: library iterator ranges still use `range(start, end)` constructors.

## Core Principle

`..` constructs a range value. Language contexts consume that value through a
small set of protocols.

```text
a .. b       constructs a range value
match        consumes Pattern
slice        consumes Slicer
for          consumes Iterator / IntoIterator
```

`range(start, end)` and `start .. end` must not become two parallel concepts.
The symbolic syntax is the canonical language form; library helpers may remain
only as compatibility or readability wrappers while the language is still
pre-1.0.

## Range Type Families

Range expressions should produce canonical builtin type families, not named
builtin structs.

```kern
0 .. 10      // i32..i32
0 ..= 10     // i32..=i32
..10         // ..i32
..=10        // ..=i32
10..         // i32..
..           // ..
```

Canonical type spelling:

```kern
T..T         // half-open range [start, end)
T..=T        // inclusive range [start, end]
..T          // range-to
..=T         // range-to-inclusive
T..          // range-from
..           // full range
```

Open issue: `..T` currently collides with parent-anchored paths such as
`..foo.Bar`. Phase 2 must resolve this before implementation. Options:

- keep `..T` for range-to types and replace parent-anchored type paths with a
  different spelling before 1.0;
- keep parent-anchored paths and choose another canonical spelling for range-to
  types;
- disambiguate by grammar only if the result is simple, predictable, and does
  not make either feature context-dependent in a surprising way.

Do not silently choose one in the parser without documenting the language rule.

These are like `?T` and `T!E`: builtin type forms with ordinary semantics. They
are not named builtin structs. Their layout can be modeled structurally by the
compiler:

```kern
T..T         // struct { start: T, end: T }
T..=T        // struct { start: T, end: T }
..T          // struct { end: T }
..=T         // struct { end: T }
T..          // struct { start: T }
..           // void-like zero-field value
```

The constructor should be unique:

```kern
let r = a .. b;
```

Do not introduce `Range.{ start: a, end: b }` as another spelling for the same
thing.

## Pattern Model

Every match pattern is conceptually checked as:

```kern
trait Pattern[T] {
    type Bind;
    fn apply(value: T) ?Bind;
}
```

`Bind` is the binding environment:

```kern
1             // Bind = void
0 .. 10       // Bind = void
.{ Some: x }  // Bind = struct { x: T }
Point.{ x }   // Bind = struct { x: X }
```

`mut` is binding metadata, not part of the value type, but it is part of bind
shape equality for an arm because the arm body must see one coherent local
environment.

For:

```kern
.{ Int: n }, .{ Float: n } => n
```

both alternatives bind:

```kern
struct { n: T }
```

For:

```kern
.{ Int: n }, .{ Float: other } => n
```

the alternatives bind different shapes and must be rejected.

Compiler-known patterns still keep extra static power:

- exhaustiveness checking
- unreachable/shadowed pattern warnings
- scalar interval coverage
- enum/struct decomposition

Opaque user-provided `Pattern` values may participate in runtime matching but
cannot prove exhaustiveness unless the compiler has explicit static knowledge
for them.

## Value Matching

Kern already has value patterns. They should be folded into the Pattern model
instead of being treated as a separate ad-hoc match feature.

Exact value matching should not mean "all `Eq` values are patterns".

Possible internal model:

```kern
trait MatchValue[T] {
    fn match_value(value: T) bool;
}

Exact[T] : Pattern[T]
    where T: MatchValue[T],
{
    type Bind = void;
}
```

`Eq` is ordinary equality. `MatchValue` is pattern semantics. The two can be
related by library impls where appropriate, but they should remain distinct
concepts.

## Slice Consumption

Slice syntax should consume a range value through a protocol rather than parse
its own private range grammar.

```kern
trait Slicer[R] {
    type Out;
    fn slice(range: R) Out;
}
```

Examples:

```kern
buf[0 .. n]
buf[..]
buf[i ..]
buf[..=last]
```

These should parse as range expressions passed to indexing/slicing semantics.

## For Consumption

`for` should consume range values through the iterator protocol.

```kern
for (i: 0 .. n) {
    ...
}
```

Library range iterator constructors such as `range(0, n)` should become
ordinary wrappers around the canonical range expression model, or be removed
before 1.0 if they become redundant.

## Match Consumption

Range patterns should be range values consumed as `Pattern[T]`.

```kern
match (x) {
    0 .. 10 => ...,
    10 ..= 20 => ...,
    _ => ...,
}
```

For compiler-known scalar range values, the compiler may keep static coverage
analysis. For non-static or user-defined pattern values, the arm is a runtime
pattern and cannot close exhaustiveness by itself.

## Implementation Plan

### Phase 1: Pattern Bind Core

- [x] Add internal bind shape model for match alternatives.
- [x] Represent no-bind patterns as `void`.
- [x] Represent binding environments as anonymous structs.
- [x] Canonicalize bind field order by name.
- [x] Require all alternatives in one arm to produce the same bind shape.
- [x] Define arm-body bindings from the unified bind shape once.
- [x] Add regression coverage for matching and mismatching alternative binds.
- [x] Document `Pattern[T] -> ?Bind`.

### Phase 2: Range AST and Type Forms

- [ ] Add AST nodes for range expressions:
  - [ ] `start .. end`
  - [ ] `start ..= end`
  - [ ] `.. end`
  - [ ] `..= end`
  - [ ] `start ..`
  - [ ] `..`
- [ ] Add type AST forms for range type families:
  - [ ] `T..T`
  - [ ] `T..=T`
  - [ ] `..T`
  - [ ] `..=T`
  - [ ] `T..`
  - [ ] `..`
- [ ] Resolve these into canonical type registry forms.
- [ ] Define structural layouts for range values.
- [ ] Add parser tests for expression and type precedence.

### Phase 3: Range Expressions in Sema and Lowering

- [ ] Type-check range expressions.
- [ ] Lower range values into their structural representation.
- [ ] Decide and document field accessibility, if any.
- [ ] Add tests for assigning, passing, and returning range values.

### Phase 4: Match Range De-Hardcoding

- [ ] Parse match range arms as range expressions where possible.
- [ ] Convert scalar range pattern coverage to consume range expression facts.
- [ ] Keep compiler-known static coverage for integer/bool-compatible range
  forms.
- [ ] Treat opaque pattern values as runtime-only patterns that do not prove
  exhaustiveness.
- [ ] Remove `MatchPatternKind::Range` if the unified representation can fully
  replace it.

### Phase 5: Slice Through Slicer

- [ ] Replace slice-bound private grammar with range expression consumption.
- [ ] Introduce or model `Slicer[R]`.
- [ ] Implement slice consumption for:
  - [ ] `usize..usize`
  - [ ] `usize..=usize`
  - [ ] `..usize`
  - [ ] `..=usize`
  - [ ] `usize..`
  - [ ] `..`
- [ ] Preserve existing slice semantics and diagnostics.

### Phase 6: For and Library Range Migration

- [ ] Implement iterator behavior for range type families.
- [ ] Make `for (i: 0 .. n)` work.
- [ ] Migrate library/tests/tutorials from `range(0, n)` to `0 .. n`.
- [ ] Decide whether `range(...)` helpers remain as wrappers or are removed
  before 1.0.

### Phase 7: Documentation and Cleanup

- [ ] Update `docs/design.md` with the full model.
- [ ] Update tutorials in English and Chinese.
- [ ] Remove stale wording that says range syntax is reserved only for slices
  and match patterns.
- [ ] Add examples showing `match`, `slice`, and `for` consuming the same range
  values.
- [ ] Run full compiler and library tests.

## Non-Goals For The First Cut

- Do not make `Eq` automatically imply pattern semantics.
- Do not add named builtin range structs.
- Do not introduce parallel constructors for range values.
- Do not let opaque user-defined patterns prove exhaustiveness unless the
  compiler has a static coverage model for them.
