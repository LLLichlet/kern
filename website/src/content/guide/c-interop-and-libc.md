---
title: "C Interop And libc"
summary: "Use `extern { ... }` for C ABI boundaries, keep libc optional by default, and opt into it only when you intentionally want that foreign interface."
order: 33
---

The earlier ABI chapters established the syntax.

This chapter covers two different cases that are easy to blur together:

- calling foreign code through the C ABI
- opting into libc itself

Those are related, but they are not the same thing.

## C Interop Does Not Automatically Require libc

While writing this guide, the current toolchain successfully built and ran a
package that linked a local C static library while keeping libc disabled:

```toml
[runtime]
entry = "rt"
libc = false
bundle = "std"
```

```kern
// build.rn
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    b.link_search("native");
    b.link_system_lib("demo");
}
```

```kern
// src/main.rn
use std.io;

extern {
    fn ext_add(lhs: i32, rhs: i32) i32;
}

fn main() i32 {
    let value = ext_add(40, 2);
    io.println("native-no-libc={}", .{value,});
    if (value != 42) {
        return 1;
    }
    return 0;
}
```

The validated run printed:

```text
native-no-libc=42
```

That example is important because it shows the current Kern position clearly:

- foreign C ABI calls are real
- `build.rn` can provide the native link inputs
- libc still remains optional

So if you want to link a real foreign library or your own C object code, the
first question is not "how do I turn libc on?".

The first question is simply "what ABI and link inputs do I actually want?".

## A Validated libc Example

libc is still supported as an explicit interface when you want it.

The package used for validation declared that choice directly:

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

This is consistent with the rest of Kern's design:

- hosted process behavior is an explicit runtime choice
- libc linkage is an explicit runtime choice
- neither one is a hidden default

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

For real C interop on the current toolchain, separate these cases:

- use `extern { ... }` whenever you need a C ABI boundary
- keep `libc = false` if you only need ordinary foreign linkage and do not want libc itself
- turn on `entry = "crt"` and `libc = true` only when you intentionally want libc / CRT participation

For actual foreign calls:

- import functions through `extern { ... }`
- pass `*u8` where the C ABI expects raw C strings
- use `base.abi.cstr` when the string must be allocated dynamically

That is the current low-level interop model.
Kern slices, C strings, and libc remain separate concepts.
