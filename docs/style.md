# Kern Source Style

This document records the current source-style guidance for Kern code in this
repository.

It is intentionally short. Language semantics live in
[`docs/design.md`](./design.md); this file only says how to express those
semantics clearly in real code.

## Goals

Prefer source that makes Kern's design visible:

- explicit where the machine-facing boundary matters
- concise where local context is already unambiguous
- expression-driven without hiding control flow
- based on orthogonal language mechanisms, not special-case habits

## Current Guidance

### 1. Prefer straight-line unwrapping for simple propagation

When control flow only unwraps a value and immediately exits on failure, keep
the success path straight.

Use `let else` when the shape stays local and the failure arm is still easy to
read:

```kern
let .{ Ok: span } = parse_value_span(text, index)
    else .{ Err: err } => return .{ Err: err };
```

Use `.?` and `.!` when the surrounding return type already makes the
propagation rule obvious and the operator makes the code shorter and clearer:

```kern
let next = iter.next().?;
let file = open(path).!;
```

Use `match` when both branches do substantial work, when you need more than one
success arm, or when the control flow is no longer a simple unwrap-and-return.

Do not force `.?` or `.!` into places where a visible pattern is clearer than a
symbol.

### 2. Use `{}` for intentionally empty loop bodies

In scanner-style code, an empty loop body should still use a real Kern block:

```kern
for (; index < #text and is_ws(text.[index]); index += 1) {}
```

Reserve this mainly for loops whose whole job is to advance until a condition
fails. Do not generalize the same style to `if (...) {}` by default when a more
direct expression shape would be clearer.

`for (...);` is not valid Kern syntax and should not appear in repository code
or documentation.

### 3. Let contextual typing do the routine work

Kern has strong contextual typing, and plain integer literals default to
`usize`.

Do not write redundant providers when the type is already fixed by the local
context:

```kern
let mut i = 0;
for (; i < #text; i += 1) { ... }
```

The same rule applies to BNC more broadly: if the receiving site already fixes
the type, do not add qualification only because it is available.

Keep explicit providers when:

- width or signedness is part of the logic
- removing the provider would silently change the type to `usize`
- the provider materially improves local readability

### 4. Be explicit when width is part of the meaning

Kern supports contextual typing, so local literals like `count == 0` are often
good style.

But when bit width is part of the logic, keep the type visible:

```kern
if (byte < u8.{0x20}) { ... }
```

This especially applies to byte parsers, masks, shifts, pointer-adjacent code,
and other low-level boundaries.

### 5. Omit qualification only when the local context is obvious

Kern may infer enum variants from context:

```kern
return .{ Err: .EmptyInput };
```

That is good style when the expected type is immediate and local.

Keep the type name when the reader would otherwise need to recover the type
from a more distant function signature, parameter list, or trait context:

```kern
parse_literal(text, index, "true", Kind.Bool)
```

The same rule applies to associated items, generic qualification, and literal
providers in general: remove redundancy, but do not make the reader reconstruct
types from far away.

### 6. Use stack mode when repeated pointer-style access is the real shape

Kern does not auto-deref. When a local value is immediately used as a mutable
object for a run of calls, take its address once at the source and keep the
rest of the code pointer-shaped:

```kern
let page = Page.{}..&;
let gpa = GPA.{ backing: page }..&;
let state = test.expect_ok(editor.empty(gpa))..&;

state.handle_key(gpa, .{ Byte: b'i' });
type_text(state, gpa, "hello");
state.handle_key(gpa, .Esc);
```

This is good style when repeated `..&` or `.&` would otherwise dominate the
call sites and the code is semantically operating on one stack-local object.

Do not force stack mode for one or two isolated calls. In short stretches,
`value..&.method(...)` is often clearer.

String literals are not stack materialization sites. If you need a mutable byte
buffer, allocate or construct one explicitly instead of trying to take a mutable
address of rodata.

### 7. Prefer explicit module visibility over forwarding boilerplate

Use visibility to describe the intended sharing boundary directly:

- `pub` for package-facing API
- `pub~` for package-internal API
- `pub..` for parent-module-tree API

Prefer these over needless `init.rn` forwarding layers when the real intent is
simply "visible to this package" or "visible inside this parent module tree".

Choose the narrowest visibility that matches the actual boundary. Do not mark
items `pub` by default.

### 8. Prefer mechanisms over privileged library types

Kern source style should lean on general language features such as enums,
patterns, traits, impls, visibility, and explicit control flow.

That means the preferred error-propagation style is built on `let else`,
pattern matching, and `.?` / `.!` first, not on treating `Option` or `Result`
as privileged language objects.
