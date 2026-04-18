---
title: "Attributes And Metadata"
summary: "Use `#[...]` and `#![...]` to attach compiler-understood linkage, optimization, layout, and pruning metadata instead of relying on a preprocessor."
order: 26
---

Kern does not use a C-style preprocessor as its main metadata channel.

Instead, the compiler understands structured attributes directly.

That keeps linkage, optimization, layout, and pruning rules visible in the
source language instead of hiding them behind textual macro expansion.

## A Validated Example

While writing this guide, the current toolchain successfully emitted LLVM IR
for this source:

```kern
#[inline]
fn hot_add(lhs: i32, rhs: i32) i32 {
    return lhs + rhs;
}

#[noinline]
fn cold_add(lhs: i32, rhs: i32) i32 {
    return lhs - rhs;
}

#[export_name("kern_bridge")]
fn bridge(x: i32) i32 {
    return x + 1;
}

fn main() i32 {
    return hot_add(bridge(4), cold_add(9, 3));
}
```

The emitted LLVM IR contained:

```text
attributes #0 = { alwaysinline }
attributes #1 = { noinline }
define i32 @kern_bridge(...)
```

That proves the current attribute pipeline is not decorative.
These attributes survive semantic checking and reach backend-visible output.

## Outer And Inner Attributes

Kern distinguishes:

- outer attributes: `#[...]`
- inner attributes: `#![...]`

Outer attributes attach to the following declaration or item.

Inner attributes apply to the enclosing lexical scope, commonly the file/module
scope.

## Stable Everyday Attributes

The most important currently user-facing attributes are:

- `#[inline]`
- `#[noinline]`
- `#[export_name("...")]`
- `#[link_section("...")]`
- `#[target_feature("...")]`
- `#![if(...)]` / `#[if(...)]`

Different attributes affect different compiler layers:

- optimization hints
- symbol naming
- section placement
- target feature requirements
- conditional pruning before later analysis

## Marker Attributes, Not Legacy Forms

Current Kern intentionally rejects some older or more C++/Rust-like spelling
variants.

For example, the compiler rejects removed forms such as:

```kern
#[inline_always]
#[inline(always)]
```

and points users toward the stable marker attributes:

- `#[inline]`
- `#[noinline]`

That is worth documenting because it keeps the guide aligned with the actual
frontend instead of teaching stale spellings.

## `export_name` Versus Normal Mangling

Kern normally uses its own symbol naming and mangling rules.

Use:

```kern
#[export_name("kern_bridge")]
```

when a symbol must appear with a specific external name for:

- foreign callers
- boot/runtime entrypoints
- linker-script contracts
- assembly or firmware integration

This is the same general direction as other Kern ABI tools: explicit external
contracts stay explicit.

## Conditional Attributes Are Structural

`#![if(...)]` and `#[if(...)]` are not textual macro systems.

They are compiler-understood pruning rules applied structurally to modules or
items.

That matters because it means conditional inclusion is part of the language's
own AST pipeline, not a second textual language layered on top.

## Practical Takeaway

Keep these ideas straight:

- attributes are compiler metadata, not preprocessor tricks
- outer and inner attributes target different scopes
- stable marker forms such as `#[inline]` and `#[noinline]` are preferred over removed variants
- linkage and section control belong in explicit attributes such as `#[export_name]` and `#[link_section]`

Used that way, attributes fit Kern's overall goal: explicit control without a
macro-preprocessor culture.
