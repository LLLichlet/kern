# Kern Tutorial

English | [简体中文](./zh/README.md)

This is the official introductory Kern tutorial. It is not a replacement for
[`design.md`](../design.md), [`craft.md`](../craft.md), or
[`runtime-architecture.md`](../runtime-architecture.md). It is a guided tour:
it starts with tools, syntax, and library usage, then gradually moves into
Kern's low-level programming model.

This tutorial follows the current Kern 0.8.1 implementation. Kern is still
pre-1.0, so syntax and library APIs may change as the design settles. If a
detail conflicts with another document, prefer the reference documents under
`docs/` and the runnable examples in this repository.

## Audience

This tutorial is for readers who already have basic programming experience and
want to learn Kern's systems programming model. It assumes you understand ideas
such as compilation, linking, stack/heap storage, pointers, and integer widths.
It does not assume you already understand Kern modules, runtime configuration,
library layering, or error handling style.

Kern targets kernels, firmware, freestanding software, and infrastructure code
that needs low-level control. It provides modern language structures such as
modules, generics, algebraic data types, traits, exhaustive pattern matching,
and package tooling while avoiding hidden runtime policy: no garbage collector,
no exceptions, no implicit heap allocation, and no implicit prelude namespace.

## Route

1. [Quick Start](./en/01-quick-start.md): install Kern, create a package, run a program, and understand the roles of `craft` and `kernc`.
2. [Language Basics](./en/02-language-basics.md): functions, bindings, types, strings, formatted output, and mutability.
3. [Data And Control Flow](./en/03-data-and-control-flow.md): structs, enums, `match`, option/result values, and error propagation.
4. [Memory, Slices, And Collections](./en/04-memory-slices-and-collections.md): arrays, slices, pointers, explicit allocation, `List`, and `String`.
5. [Modules, Packages, And Library Layers](./en/05-modules-packages-and-libraries.md): `use`, official library layers, `Craft.toml`, examples, and tests.
6. [Freestanding And Runtime Basics](./en/06-freestanding-and-runtime.md): runtime entry, the `base` bundle, custom `_start`, and linker-script entry points.
7. [Aggregates And Initialization](./en/07-aggregates-and-initialization.md): struct defaults, field puns, layout, anonymous structs, unions, and enum initialization.
8. [Impls, Traits, And Generic Bounds](./en/08-impl-traits-and-generics.md): methods, trait objects, associated types, builtin traits, and operator boundaries.
9. [Closures And Function Values](./en/09-closures-and-function-values.md): `&fn`, `&Fn`, captures, escaping closures, and `#` state extraction.
10. [Attributes, Intrinsics, And Low-Level Operations](./en/10-attributes-intrinsics-and-operators.md): `#[...]`, `@sizeOf`, atomics, SIMD, and `@asm`.
11. [Next Steps](./en/11-next-steps.md): where to continue in the reference docs, standard library source, and example projects.

## Recommended Practice

Keep these two directories open while reading:

- [`examples/`](../../examples): small runnable examples.
- [`library/`](../../library): the in-tree official library workspace, especially the boundaries between `base`, `rt`, and `std`.

Tutorial code prefers `craft`. Direct `kernc` usage is called out explicitly,
because `kernc` is a lower-level compile/link driver rather than a package
manager.
