# Pattern Protocol Completion Plan

This replaces the earlier range/slice/for audit as the active implementation
plan. Range values were only the first cleanup step. The final goal of this
round is to make `match` consume a real `Pattern[T]` protocol, while preserving
the compiler-owned pieces Kern still needs for binding, privacy, and static
coverage analysis.

## Source Of Truth

- `docs/design.md` defines Kern semantics.
- `docs/style.md` records how repository code should express them.
- Kern is freestanding. `kernc`, `craft`, and language semantics must not
  depend on `base`, `std`, `rt`, or any replaceable library implementation.

## Core Model

`match` is a sequence of pattern applications over the scrutinee type:

```kern
trait Pattern[T] {
    type Bind;
    fn apply(value: T) ?Bind;
}
```

- `Bind = void` means the pattern introduces no arm bindings.
- `Bind = struct { name: T, ... }` introduces fields as arm-local bindings.
- All alternatives in one arm must produce the same canonical bind shape:
  same names, same field types, same mutability metadata.
- User-defined opaque `Pattern` values are allowed in match arms, but do not
  prove exhaustiveness or unreachable coverage.
- Compiler-known patterns are represented through the same internal model, but
  carry extra metadata for exhaustiveness, shadowing, enum/struct privacy, and
  binding syntax.

## Pattern Forms

### User Pattern Values

Any expression in pattern position may be consumed as a pattern value when its
type implements `Pattern[Scrutinee]`.

```kern
match (value) {
    is_ascii_digit => ...,
    positive_i32 => ...,
    _ => ...,
}
```

Lowering is conceptually:

```kern
let __p = pattern;
let __matched = __p.apply(scrutinee);
match (__matched) {
    .{ Some: bind } => arm_body,
    .None => next_pattern,
}
```

The actual lowering may keep compiler-known patterns on optimized paths.

### Structural Patterns

Syntax such as:

```kern
.{ Some: value }
Point.{ x, y }
```

is still compiler parsed, because syntax introduces binding names and because
field/variant access must respect language rules. Semantically, the compiler
generates a pattern adapter whose `Bind` is the canonical binding structure,
for example:

```kern
struct { value: T }
```

This is not a separate match kernel. It is a compiler-generated `Pattern`
instance with compiler-known coverage metadata.

### Value Patterns

Value patterns remain exact-value patterns. They are compiler-known no-binding
patterns and keep the existing equality capability used by `==`. This preserves
current Kern style guidance while placing the result in the same `Bind = void`
model.

### Range Patterns

Range syntax constructs builtin range values:

```kern
0 ... 10
0 ..= 10
...10
..=10
10...
...
```

Closed scalar range values in match position are compiler-known no-binding
patterns. They also act like `Pattern[T]` where the scalar bound domain is
valid, so they share the same model without giving slice bounds or iterator
syntax special treatment.

Open-ended range patterns remain rejected in match until the coverage semantics
are explicitly designed.

## Slice And For Boundaries

Slice construction remains language-owned memory syntax:

```kern
buf.&[0...n]
buf..&[0...n]
```

`SliceBounds` is a compiler-owned marker like `Integer`: user code can mention
it, but cannot implement it. It classifies accepted slice-bound range values
and does not perform slicing.

`for (pat: expr) body` remains the existing parser desugaring through
`hidden..&.next()`. Ranges only work in `for` when ordinary method resolution
makes the range expression iterable in the compiled package.

Reverse traversal is an iterator adapter on existing range values, not a new
range constructor family. The base package exposes `(a...b).rev()` and
`(a..=b).rev()` as ordinary combinators returning iterator state; it should not
expose standalone helpers such as `range_down`.

## Implementation Phases

### Phase 1: Real Pattern Trait

- [x] Keep range syntax/type work already done.
- [x] Keep `SliceBounds` as a compiler-owned marker.
- [x] Inject builtin `Pattern[T]` with associated type `Bind` and method
  `apply(value: T) ?Bind`.
- [x] Permit user impls of `Pattern`; do not classify it as a compiler-owned
  marker.
- [x] Add semantic facts recording that a match expression-pattern uses
  `Pattern[Scrutinee, Bind = B]`.

### Phase 2: Match Sema

- [x] For expression patterns, check compiler-known forms first:
  exact values, enum values, struct literals, closed scalar ranges.
- [x] If the expression is not a compiler-known structural/value/range pattern,
  type-check it as a pattern value and require `Pattern[Scrutinee]`.
- [x] Resolve and normalize its `Bind`.
- [x] Accept `Bind = void`.
- [x] Accept `Bind = struct { ... }` and expose fields as arm bindings.
- [x] Reject other `Bind` shapes for now with a clear diagnostic.
- [x] Keep opaque user patterns out of exhaustiveness and unreachable proof.

### Phase 3: Match Lowering

- [x] Lower user pattern values to `pattern.apply(scrutinee)`.
- [x] Branch on the returned builtin optional:
  `.Some` enters the arm, `.None` falls through.
- [x] For `Bind = void`, no local bindings are introduced.
- [x] For `Bind = struct { ... }`, bind each field into the arm scope.
- [x] Preserve existing optimized compiler-known lowering paths while their
  generated semantics match the `Pattern` model.

### Phase 4: Tests

- [x] User `Pattern[i32]` with `Bind = void`.
- [x] User `Pattern[i32]` with `Bind = struct { value: i32 }`.
- [x] Same-arm alternatives with identical user `Bind` shapes accepted.
- [x] Same-arm alternatives with different `Bind` shapes rejected.
- [x] User opaque pattern requires catch-all for non-ADT scrutinees.
- [x] User pattern works in generic and closure-adjacent contexts.
- [x] Supertrait/projection cases cannot forge invalid `Pattern.Bind`.
- [x] Existing exact value, range, and structural exhaustiveness tests still
  pass.

### Phase 5: Documentation And Cleanup

- [x] Update `docs/design.md` so `Pattern` is real protocol semantics, not just
  conceptual explanation.
- [x] Keep style guidance for equality value patterns unless the equality model
  deliberately changes later.
- [x] Remove wording that implies range internalization is the final goal.

## Non-Goals For This Cut

- Do not make slice syntax user-overloadable.
- Do not introduce `Slicer[R]` as a user-implemented slicing protocol.
- Do not make `for` depend on range syntax.
- Do not make compiler/tooling depend on library range adapters.
- Do not expose range values as ordinary user-declared structs.
- Do not expose descending range constructor families such as `range_down`;
  use range adapters such as `.rev()` instead.
- Do not fully delete compiler-known structural pattern analysis; it remains
  needed for binding syntax, privacy, and exhaustiveness metadata.
