---
title: "External Statics And `link_section`"
summary: "Import foreign data with `extern { static ... = T.{undef}; }`, export named globals with `#[export_name]`, and place globals into explicit output sections."
order: 34
---

The earlier ABI chapters established function imports and exports.

This chapter covers the data side of that same boundary.

## A Validated Example

The following package was checked and run successfully while writing this
guide:

```kern
// src/main.rn
use std.io;

mod abi_data;

extern {
    static extern_magic = u32.{undef};
}

fn main() i32 {
    io.println("magic={}", .{extern_magic,});
    if (extern_magic != 42) {
        return 1;
    }
    return 0;
}
```

```kern
// src/abi_data.rn
#[export_name("extern_magic")]
#[link_section(".kern_demo")]
static MAGIC_SOURCE = u32.{42};
```

For the validated run, this printed:

```text
magic=42
```

## Importing Foreign Data

The imported symbol used this exact form:

```kern
extern {
    static extern_magic = u32.{undef};
}
```

Two details matter:

- imported statics live inside an `extern { ... }` block
- the initializer must be `T.{undef}`

That `undef` initializer is not decorative. It is how the current toolchain
spells "this storage is defined somewhere else".

## Exporting A Named Global Symbol

The data definition itself used:

```kern
#[export_name("extern_magic")]
static MAGIC_SOURCE = u32.{42};
```

That is the currently validated export path for data symbols:

- define an ordinary top-level `static`
- force the linker-visible name with `#[export_name("...")]`

Notice that the exported definition did not use an `extern` block.

So the practical split is:

- imported data uses `extern { static ... }`
- exported data uses a top-level `static` plus export metadata

## Placing Globals Into A Specific Section

The same definition also used:

```kern
#[link_section(".kern_demo")]
```

That attribute was validated with `kernc --emit-llvm`. The generated LLVM
contained:

```text
@extern_magic = constant i32 42, section ".kern_demo"
```

That proves the section assignment survived lowering and reached codegen.

## Why This Pattern Matters

This is the shape you need when data location and symbol identity are part of
the contract, for example:

- boot protocol headers
- linker-discovered tables
- firmware or kernel metadata blocks
- foreign code that expects a specific exported data symbol

The important point is that Kern keeps the contract explicit:

- symbol import is explicit
- symbol export name is explicit
- output section is explicit

Nothing here depends on libc. This is a lower-level linker and ABI mechanism
that fits Kern's direct systems model.
