---
title: "Inline Assembly"
summary: "Use `@asm(.{ ... })` as a compile-time validated assembly specification with explicit template, register bindings, clobbers, and volatility."
order: 25
---

Kern treats inline assembly as structured compiler input, not as a loose string
escape hatch.

That is why `@asm` takes a configuration object instead of a C-style format
string with hidden positional rules.

## A Validated Example

The following package was built and run successfully while writing this guide:

```kern
fn main() i32 {
    @asm(.{
        asm: "nop",
        volatile: true,
    });
    return 0;
}
```

This is intentionally minimal, but it proves the current surface:

- `@asm` is accepted as an ordinary expression statement
- the `asm` template can be a single string literal
- the configuration is validated by the frontend before lowering

## Configuration Shape

Current `@asm` expects exactly one anonymous struct argument:

```kern
@asm(.{
    asm: "nop",
    volatile: true,
});
```

The important fields are:

- `asm`
- `inputs`
- `outputs`
- `clobbers`
- `volatile`

Not every field is required on every use, but the `asm` template itself is.

## The `asm` Field Is Compiler Metadata

The `asm` field is not a runtime string object passed to some helper function.

It is compile-time configuration consumed by the compiler.

The current language accepts exactly one string literal.

For multi-line templates, use Kern's multiline string syntax:

```kern
@asm(.{
    asm:
        \\out dx, al
        \\in al, dx
    ,
    volatile: true,
});
```

Historical string-array forms are rejected rather than preserved as legacy
syntax.

## Register Bindings Stay Explicit

When you use `inputs` or `outputs`, Kern wants named register bindings through
anonymous struct fields rather than hidden operand numbering.

The current design direction is:

```kern
@asm(.{
    asm:
        \\out dx, al
        \\in al, dx
    ,
    outputs: .{ al: status..& },
    inputs: .{ dx: port, al: data },
    clobbers: .{ "memory" },
    volatile: true,
});
```

Two practical rules matter:

- `outputs` must be bound to mutable pointers
- `clobbers` must be compile-time string literals

If those shapes are wrong, the compiler rejects the `@asm` configuration during
semantic checking.

## Why Kern Uses This Shape

This structured form makes inline assembly fit Kern's larger design:

- explicit register names
- explicit mutability for outputs
- explicit side-effect signaling through `volatile`
- compile-time validation before lowering/codegen

That is much closer to "compiler-owned IR input" than to "paste a raw assembly
blob and hope".

## Practical Takeaway

Treat `@asm` as a validated specification object:

- always pass a single `.{ ... }` configuration
- keep the template explicit in `asm` as one string literal
- bind inputs and outputs by named registers
- mark side-effectful assembly with `volatile: true`

If you approach inline assembly that way, it stays consistent with the rest of
Kern's explicit systems model.
