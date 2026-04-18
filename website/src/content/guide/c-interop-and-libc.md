---
title: "C Interop And libc"
summary: "Call real libc functions through `extern { ... }`, use variadic imports explicitly, and build zero-terminated strings for C APIs."
order: 19
---

The earlier ABI chapters established the syntax.

This chapter shows a real opt-in libc interop path that was validated against
the current toolchain.

## A Validated Example

The package used for validation declared hosted libc access explicitly:

```toml
[runtime]
entry = "crt"
libc = true
bundle = "std"
```

The program itself was:

```kern
use base.abi;
use base.mem.alloc.GPA;
use sys.mem.Page;

extern {
    fn printf(format: *u8, ...) i32;
    fn puts(msg: *u8) i32;
}

fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
    let fmt = "value=%d\n\0";

    let written = printf(fmt.[0].&, 7);
    if (written <= 0) {
        return 1;
    }

    let .{ Some: msg } = abi.cstr.alloc_z(gpa, "hello from cstr") else return 2;
    defer abi.cstr.free_z(gpa, "hello from cstr", msg);

    if (puts(msg) < 0) {
        return 3;
    }

    return 0;
}
```

For the validated run, this printed:

```text
value=7
hello from cstr
```

## What This Example Proves

### Variadic Imports Work Through `extern`

This declaration:

```kern
fn printf(format: *u8, ...) i32;
```

is the current variadic import model.

The `...` spelling is part of the imported signature itself. Kern does not
hide C variadics behind a wrapper type.

### libc Linkage Must Be Chosen Explicitly

The manifest used:

```toml
entry = "crt"
libc = true
bundle = "std"
```

That combination matters:

- `entry = "crt"` selects hosted process startup through the C runtime path
- `libc = true` enables libc linkage
- `bundle = "std"` still gives access to `base`, `sys`, and `std` roots

This is consistent with the rest of Kern's design. Hosted process behavior and
libc linkage are explicit runtime choices, not hidden defaults.

It is also important not to misread what this means:

- `sys` remains Kern's own OS/provider boundary
- `std` remains ordinary Kern code on top of `base` and `sys`
- opting into libc does not suddenly make libc the base layer of hosted Kern

In practice, libc is here as an intentional compatibility interface for cases
where a program wants that ABI or needs to link against real-world foreign
libraries that expect it.

### String Literals Do Not Implicitly Become `*u8`

The first validation attempt failed on purpose with:

```text
expected `*u8`
found `[]u8`
```

That happened when passing a string literal directly to `printf`.

The working form was:

```kern
let fmt = "value=%d\n\0";
printf(fmt.[0].&, 7);
```

So the current rule is clear:

- string literals are `[]u8`
- C functions that want `*u8` still need an explicit pointer

### `base.abi.cstr` Is The Right Helper Layer

For dynamically built C strings, the example used:

```kern
let .{ Some: msg } = abi.cstr.alloc_z(gpa, "hello from cstr") else return 2;
defer abi.cstr.free_z(gpa, "hello from cstr", msg);
```

That helper exists specifically to bridge ordinary Kern byte slices into
zero-terminated foreign strings.

## Practical Takeaway

For real C interop on the current toolchain:

- import functions through `extern { ... }`
- enable libc intentionally in `[runtime]`
- pass `*u8` where the C ABI expects raw C strings
- use `base.abi.cstr` when the string must be allocated dynamically

That is the low-level, explicit model Kern is aiming for. It does not try to
pretend C strings and Kern slices are the same thing.
