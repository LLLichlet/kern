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
    else {
        .{ Err: err } => return .{ Err: err },
    };
```

Use `.?` and `.!` when the surrounding return type already makes the
propagation rule obvious and the operator makes the code shorter and clearer:

```kern
let next = iter.next().?;
let file = open(path).!;
```

When the only extra work is to lift one error type into another, prefer
`map_err(...).!` over spelling the same `Err -> Err` bridge by hand:

```kern
let file = open(path)
    .map_err([](err: fs.Error) Error { return .{ Fs: err }; })
    .!;
```

Use `match` when both branches do substantial work, when you need more than one
success arm, or when the control flow is no longer a simple unwrap-and-return.

Do not force `.?` or `.!` into places where a visible pattern is clearer than a
symbol.

### 2. Use `{}` for intentionally empty loop bodies

In scanner-style code, an empty loop body should still use a real Kern block:

```kern
while (index < #text and is_ws(text.[index])) {
    index += 1;
}
```

Reserve this mainly for loops whose whole job is to advance until a condition
fails. Do not generalize the same style to `if (...) {}` by default when a more
direct expression shape would be clearer.

`while (...);` is not valid Kern syntax and should not appear in repository
code or documentation.

### 3. Let contextual typing do the routine work

Kern has strong source- and context-driven type inference. When the local type
source already fixes the type, omit the type/provider. Repository code,
standard-library code, incubator examples, and docs should exercise that
inference instead of spelling types out defensively.

Do not write redundant annotations or providers when the type is already fixed
by the local context:

```kern
let mut i = 0;
while (i < #text) {
    ...
    i += 1;
}
```

The same rule applies to BNC, enum variants, associated items, generic
qualification, and literal providers more broadly: if the receiving site
already provides the type source, do not add qualification only because it is
available. Redundant providers make Kern code look half-inferred and weaken the
robustness coverage that the standard library should give the compiler.

Keep explicit providers when:

- width or signedness is part of the logic
- removing the provider would silently change the type to `i32`
- the provider materially improves local readability
- the boundary is ABI-facing, serialization-facing, or otherwise intentionally
  machine-facing

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
let page = page()..&;
let gpa = gpa().on(page)..&;
let t = test.report(io.stderr())..&;
let state = editor.empty(gpa).should_ok().sum(@loc(), t)..&;

state.handle_key(gpa, .{ Byte: b'i' });
type_text(state, gpa, "hello");
state.handle_key(gpa, .Esc);
```

This is good style when repeated `..&` or `.&` would otherwise dominate the
call sites and the code is semantically operating on one stack-local object.

Do not force stack mode for one or two isolated calls. In short stretches,
`value..&.method(...)` is often clearer.

String literals are byte-array value expressions. Use them directly when the
code wants fixed bytes or ordinary array-to-slice decay; bind the value first
when a longer-lived slice or repeated mutation needs a named storage location.

### 7. Prefer explicit module visibility over forwarding boilerplate

Use visibility to describe the intended sharing boundary directly:

- `pub` for package-facing API
- `pub/` for package-internal API
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

### 9. Put methods on the weakest useful receiver

Kern impls are for types, and method lookup can use shared-reference methods
from mutable references. If a method only observes a value, put it on `&T`; do
not duplicate the same method in `impl &mut T`.

```kern
impl &String {
    pub fn path() fs.Path {
        return .{ raw: self.as_str() };
    }
}
```

The method above is also available on `&mut String`, because a mutable reference
can be used where a shared receiver is enough.

Use `impl &mut T` only when the method mutates through the receiver, exposes
mutable storage, consumes mutation-only capability, or implements a trait whose
contract is intentionally mutable:

```kern
impl &mut Buffer {
    pub fn clear() void {
        ...
    }
}
```

When both shared and mutable behavior are useful, split them by capability:
query and view methods belong on `&T`; mutation, reservation, deinit, and
mutable-slice access belong on `&mut T`. Only add a more specific receiver such
as `&&mut T` when the distinction is part of the API contract, not as a workaround
for ordinary method lookup.

When a method returns a value that stores a borrow of the receiver, do not put
that method on a value receiver. Use a reference receiver so the API cannot
silently preserve a pointer into a temporary receiver value:

```kern
impl[N: usize] &[N]u8 {
    pub fn reader() io.SliceReader {
        return .{ data: self.*.&[0 .. N] };
    }
}
```

Use value receivers for lightweight handles and pure value operations whose
result does not borrow from the receiver.

### 10. Prefer fluent capability methods over module-shaped action helpers

When an operation is naturally about one receiver value, make the receiver carry
the public API:

```kern
reader.copy_to(writer);
"build {}".fmt(.{id}).debug();
path.path().write_all_atomic(gpa, bytes);
```

Avoid keeping a parallel public helper that takes the receiver value as an
ordinary argument only because that shape existed first. If shared
implementation is needed, put it behind a private or parent-private helper and
expose one ordinary method-shaped path to users.

For resources, prefer an owned handle that carries the metadata needed to release
it. Do not expose paired public helpers that require the caller to remember an
original slice, layout, or length just to free the resource:

```kern
let c_path = abi.cstr.owned(gpa, path).?..&;
defer c_path.deinit(gpa);
os.open_file(c_path.ptr(), options);
```

When the resource comes out of a pattern, bind the payload as mutable in the
pattern and then enter stack mode:

```kern
let .{ Some: mut owned_name } = abi.cstr.owned(gpa, name) else return false;
let c_name = owned_name..&;
defer c_name.deinit(gpa);
```

For tests, make assertions postfixed on the checked value and finish them with
`sum(@loc(), report)`. The report value is local, carries the output sink, and
keeps the assertion site explicit without global test state:

```kern
let t = test.report(io.stderr())..&;

"42".parse[i32]().should_ok().eq(42).sum(@loc(), t);
buffer.is_empty().should().sum(@loc(), t);
```

Avoid public test helpers whose only behavior is a silent `@trap()`. A test
failure should report at least its source location and failure kind.
