---
title: "Pointers And Slices"
summary: "Work with `*T`, `*mut T`, `[]T`, and `[]mut T` using Kern's explicit permission model."
order: 18
---

Kern treats pointers and slices as ordinary first-class values, but it keeps
permissions visible in the syntax.

That is one of the language's core design choices.

## A Validated Example

The following example was built and run successfully while writing this guide:

```kern
use std.io;

type Counter = struct {
    value: i32,
};

impl *mut Counter {
    pub fn bump() void {
        self.value += 1;
    }
}

fn sum3(values: []u8) i32 {
    return (values.[0] as i32) + (values.[1] as i32) + (values.[2] as i32);
}

fn main() i32 {
    let bytes = [5]mut u8.{ 1, 2, 3, 4, 5 };
    let view = bytes..[1 .. 4];
    view.[0] = 9;

    let mut counter = Counter.{ value: 3 };
    counter..&.bump();

    io.println("sum={} len={} counter={}", .{
        sum3(bytes.[1 .. 4]),
        #view,
        counter.value,
    });
    return 0;
}
```

For the validated run, this printed:

```text
sum=16 len=3 counter=4
```

## What This Example Proves

### `..&` Produces A Mutable Pointer

This line:

```kern
counter..&.bump();
```

creates `*mut Counter`, because the storage is mutable and the call needs write
permission through the pointer.

The read-only form is `.&`, which produces `*T`.

### Slice Syntax Also Carries Permissions

This line:

```kern
let view = bytes..[1 .. 4];
```

produces `[]mut u8`.

That matters because the next line mutates through the view:

```kern
view.[0] = 9;
```

If you only need a read-only view, use `bytes.[1 .. 4]`, which produces `[]u8`.

### `#` Extracts Fat-Pointer Metadata

For slices and arrays, `#` returns the runtime length:

```kern
#view
```

In the validated run, that length was `3`.

## Two Mutability Rules Worth Remembering

Kern splits two questions that other languages often blur together:

- is the binding itself mutable?
- does the access path permit writes through it?

That is why these spellings are distinct:

- `let mut counter = ...` means the binding may be rebound
- `*mut Counter` means the pointer grants write access
- `[5]mut u8` means the array elements themselves are writable
- `[]mut u8` means the slice view grants writes through the view

This is the same general rule the rest of Kern follows: keep storage and access
permissions visible instead of hiding them behind default reference semantics.
