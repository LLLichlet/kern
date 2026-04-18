---
title: "ABI And `extern`"
summary: "Cross ABI boundaries explicitly: import foreign symbols with `extern { ... }` blocks and export ABI-visible symbols with `extern fn` plus attributes."
order: 27
---

Kern treats ABI boundaries as something you spell out, not something the
compiler guesses.

That is why importing and exporting use related but distinct forms.

## A Validated Example

The following package was built and run successfully while writing this guide:

```kern
// src/main.rn
use std.io;

mod bridge_mod;

extern {
    fn bridge(args: [][]u8) i32;
}

fn main() i32 {
    let argv = [2][]u8.{ "alpha", "beta gamma", };
    let status = bridge(argv);
    io.println("bridge={}", .{status,});
    return 0;
}
```

```kern
// src/bridge_mod.rn
#[export_name("bridge")]
extern fn bridge_impl(args: [][]u8) i32 {
    if (#args != 2) {
        return 1;
    }

    let first = args.[0];
    let second = args.[1];
    if (!first.eq("alpha")) {
        return 2;
    }
    if (!second.eq("beta gamma")) {
        return 3;
    }
    return 0;
}
```

For the validated run, this printed:

```text
bridge=0
```

## The Rule That Matters

### Importing Uses `extern { ... }`

This declaration:

```kern
extern {
    fn bridge(args: [][]u8) i32;
}
```

is the correct form for imported ABI symbols.

Even a single imported function still goes inside an `extern` block. Kern does
not use standalone `extern fn foo(...);` declarations for imports.

### Exporting Uses A Top-Level `extern fn`

This definition:

```kern
#[export_name("bridge")]
extern fn bridge_impl(args: [][]u8) i32 { ... }
```

creates an ABI-visible exported symbol.

`extern` marks the ABI boundary, and `#[export_name("bridge")]` forces the
final linker-visible name.

### `main` Is Still Special

The validated example exports `bridge`, not `main`.

That distinction matters because program-entry `main` is governed by the
runtime-entry contract, and current Kern rules forbid declaring the root
program `main` as `extern`.

So the practical split is:

- ordinary exported ABI symbols use top-level `extern fn`
- imported ABI symbols use `extern { ... }`
- program `main` stays an ordinary root function when runtime entry is enabled

## Why The Example Uses Slices

The example passes `[][]u8` across the boundary on purpose.

That shows the ABI-facing function is not limited to trivial integer
signatures. Kern can expose richer language-level data layouts as long as both
sides agree on the ABI contract.

For foreign C interop, keep using the explicit pointer/layout forms described
in `docs/design.md`.
