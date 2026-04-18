---
title: "ABI Layout And `extern struct`"
summary: "Use `extern type Name = struct { ... }` when layout is part of the contract, and keep ABI-visible pointers typed explicitly."
order: 15
---

The first `extern` chapter covered symbol boundaries.

This chapter covers data-layout boundaries.

## A Validated Example

The following package was built and run successfully while writing this guide:

```kern
use std.io;

extern type Header = struct {
    tag: u8,
    size: u32,
};

extern {
    fn header_score(header: *Header) i32;
}

#[export_name("header_score")]
extern fn header_score_impl(header: *Header) i32 {
    return (header.tag as i32) + (header.size as i32);
}

fn main() i32 {
    let header = Header.{ tag: 3, size: 9 };
    let score = header_score(header.&);
    io.println("score={}", .{score,});
    return 0;
}
```

For the validated run, this printed:

```text
score=12
```

## The Important Syntax Rule

Named extern structs use this form:

```kern
extern type Header = struct {
    tag: u8,
    size: u32,
};
```

That spelling matters.

The compiler's current rule is not:

```kern
type Header = extern struct { ... }
```

For named declarations, `extern` belongs before `type`.

## Why Use `extern` Here

Ordinary Kern structs are free to use the language's native layout policy.

`extern type ... = struct` is what you use when field order and ABI shape must
match an external contract, such as:

- C-facing data exchange
- hardware-facing headers
- OS ABI structures

In other words, use `extern` when layout is part of the interface rather than
an internal optimization detail.

## ABI Pointers Stay Explicit Too

The imported function takes:

```kern
fn header_score(header: *Header) i32;
```

and the call site passes:

```kern
header.&
```

That is consistent with the rest of Kern's model:

- the ABI-facing type is explicit
- the pointer type is explicit
- the address-of operation is explicit

Nothing about crossing an ABI boundary makes Kern fall back to hidden reference
semantics.
