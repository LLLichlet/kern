# Kern Source Style

This document records the current source-style guidance for Kern code in this
repository.

It is intentionally short.
Language semantics live in [`docs/design.md`](./design.md); this file only says
how to express those semantics clearly in real code.

## Goals

Prefer source that makes Kern's design visible:

- explicit where the machine-facing boundary matters
- concise where local context is already unambiguous
- expression-driven without hiding control flow
- based on orthogonal language mechanisms, not special-case habits

## Current Guidance

### 1. Use `let else` for straight-line error propagation

When a branch only unwraps a value and immediately returns on failure, prefer
`let else` over a two-arm `match`.

```kern
let .{ Ok: span } = parse_value_span(text, index)
    else .{ Err: err } => return .{ Err: err };
```

Use `match` when both branches do substantial work, when you need more than one
success arm, or when the shape is no longer a simple unwrap-and-return.

### 2. Use `;` for intentionally empty loop bodies

In scanner-style code, an empty loop body is clearer as an explicit empty
statement than as an empty block.

```kern
for (; index < #text and is_ws(text.[index]); index += 1);
```

This is not treated as a C leftover. In Kern, `;` directly states that the loop
body is a no-op.

Reserve this mainly for loops whose whole job is to advance until a condition
fails. Do not generalize the same style to `if (...) ;` by default, because
that form is much easier to write accidentally.

Current implementation note: this is the intended source style, but the current
frontend still rejects `for (...);` in loop position. Until that parser gap is
closed, repository code should keep the equivalent `for (...) {}` form.

### 3. Be explicit when width is part of the meaning

Kern supports contextual typing, so local literals like `count == 0` are often
good style.

But when bit width is part of the logic, keep the type visible:

```kern
if (byte < u8.{0x20}) { ... }
```

This especially applies to byte parsers, masks, shifts, pointer-adjacent code,
and other low-level boundaries.

### 4. Omit enum qualification only when the local context is obvious

Kern may infer enum variants from context:

```kern
return .{ Err: .EmptyInput };
```

That is good style when the expected type is immediate and local.

Keep the type name when the reader would otherwise need to recover the enum type
from a more distant function signature or parameter list:

```kern
parse_literal(text, index, "true", Kind.Bool)
```

### 5. Prefer mechanisms over privileged library types

Kern source style should lean on general language features such as enums,
patterns, and explicit control flow.

That means the preferred error-propagation style is built on `let else` and
pattern matching first, not on treating `Result` or `Option` as privileged
language objects.
