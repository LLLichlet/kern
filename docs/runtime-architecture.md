# Runtime And Library Architecture

This document defines the runtime and library split introduced after the v0.6.7 `main` cleanup.

The goal is to keep Kern freestanding by default while making hosted startup, toolchain-owned startup, libc usage, and standard-library selection explicit and orthogonal.

## Design Goals

- keep the language itself freestanding
- make startup ownership explicit
- separate runtime provider choice from library choice
- keep hosted OS interaction separate from libc linkage
- keep `kernc` as a low-level executor
- move package-level defaults and presets into `craft`
- avoid Rust-style "special crate" coupling between the compiler and a privileged `std/core` split

## Freestanding Means Libc-Free

In Kern, "freestanding" is a statement about dependency direction, not a statement
about whether useful libraries exist.

The intended dependency graph is:

- the language and compiler stand on their own
- `base` stands on its own
- `sys` is the operating-system and provider boundary
- `rt` is startup and minimal runtime glue
- `std` is ordinary Kern library code built on `base` plus `sys`
- `libc` is an optional external package/provider choice

The important rule is that `std` does not become "real" by depending on libc.
`std` is already complete as a Kern library layer because its hosted capabilities
flow through `sys`, not through an implicit C foundation.

Stated another way:

- hosted is an OS/process-environment concern, not a C-language concern
- an OS can exist without libc
- libc cannot exist without an OS or equivalent host environment
- therefore libc is downstream of the hosted boundary, while `sys` owns that boundary directly

This is why Kern treats libc as optional even for hosted programs. A project may:

- stay fully freestanding with no libc at all
- use `std` while still remaining libc-free
- select libc explicitly as one provider/runtime choice for performance, ABI, or ecosystem reasons

That makes `kern` a genuine alternative foundation rather than a thin front-end over C.

## The Four Current Axes

The current toolchain model uses four explicit axes.

### Runtime Entry

`runtime_entry` selects who owns the program entry contract.

- `none`: no program entry contract is synthesized
- `rt`: toolchain-owned runtime startup is enabled
- `crt`: the platform C runtime owns initial process startup

This axis decides whether the root module must provide a program `main`.

### Runtime Provider

`runtime_provider` selects who provides the low-level runtime and platform shims.

- `none`: no provider is assumed
- `toolchain`: use the toolchain-owned runtime/provider implementation
- `libc`: use the platform libc and CRT environment

This is intentionally separate from `runtime_entry`. A program may use the `rt` entry shim without making libc the provider, or may use CRT startup while still importing normal Kern libraries.

### Runtime Libc

`runtime_libc` is a direct yes/no switch for whether libc is linked.

This exists because "uses libc" and "uses CRT startup" are related but not identical concerns. The toolchain should be able to express both explicitly instead of hiding them behind one overloaded mode.

`runtime_libc` does not define whether hosted facilities exist. Hosted process access is modeled through the OS/provider boundary in `sys`; libc linkage is only one possible implementation choice for that boundary.

`runtime_libc` also does not define whether `std` exists. `std` is a normal Kern
library layer and remains valid without libc.

### Library Bundle

`library_bundle` selects which official library bundle is injected automatically.

- `none`
- `base`
- `std`

Today this is still coarse-grained. That is acceptable for the current migration, but the architecture is deliberately shaped so future bundles or presets can be added without redefining startup semantics.

## Main Contract

When `runtime_entry != none`, Kern treats the root `main` as a special program-entry symbol.

The only legal entry signatures are:

```kern
fn main() i32
fn main(argc: i32, argv: **u8) i32
```

Rules:

- `main` must live in the root module
- `main` must not be `extern`
- `main` must not be generic
- `main` must return `i32`
- `argv` uses the raw C-style process ABI

This is the correct low-level contract for Kern's philosophy. It is explicit, stable, and decoupled from allocation or slice construction.

Higher-level argument handling belongs in ordinary libraries, not in the compiler-owned ABI itself. The current wrapper lives in `std.proc` as `std.proc.Args` and `std.proc.args(argc, argv)`.

## Library Organization

The public library/runtime split is:

- `library/base`: runtime-independent foundation types, memory primitives, and containers
- `library/sys`: provider and operating-system boundaries
- `library/rt`: startup entry glue and minimal runtime support
- `library/std`: high-level user-facing facilities

These are ordinary public layers, not compiler-privileged crates.

This keeps the roles clear:

- language semantics stay in the compiler
- foundation facilities stay in `base`
- provider and OS boundaries stay in `sys`
- startup/runtime glue stays in `rt`
- reusable high-level facilities stay in `std`

The practical rule is:

- `std` may depend on `base` and `sys`
- `std` must not require libc as a semantic foundation
- hosted `std` facilities depend on OS/provider services exposed by `sys`, not on libc as a semantic prerequisite
- libc may be used as an implementation detail behind `sys` or as an explicitly linked external package
- `rt` stays a separate runtime-owned layer and is not mirrored through `std`
- low-level modules such as allocators, collection primitives, ABI helpers, and page-backed memory stay in their owning layer instead of being duplicated under `std`

As part of this cleanup, legacy mirror modules such as `std.coll`, `std.mem`, `std.cmp`, `std.hash`, `std.num`, `std.cffi`, `std.os`, and `std.rt` are removed. Code should import `base.*`, `sys.*`, or `rt.*` directly when it needs those boundaries.

Kern should not grow a Rust-style semantic split where the compiler secretly relies on a special crate boundary. Library layering remains a normal toolchain and package-architecture problem.

## Tooling Model

### `kernc`

`kernc` should expose the raw axes directly:

- `--runtime-entry`
- `--runtime-provider`
- `--runtime-libc`
- `--library-bundle`
- `--entry-symbol` for raw linker entry selection when needed

`--entry-symbol` is intentionally lower-level than `runtime_entry`. It controls the final linker entry symbol and remains valid even when `runtime_entry = none`, which keeps naked freestanding builds fully explicit.

### `craft`

`craft` should own package-level defaults through `Craft.toml`:

```toml
[runtime]
entry = "rt"
provider = "toolchain"
libc = false
bundle = "std"
```

This is the correct place for project policy. Most users should set runtime/library intent in `Craft.toml`, not by manually repeating low-level `kernc` flags in every build invocation.

The current implementation supports package-level `[runtime]` configuration. Future work should add clearer workspace/profile inheritance and named presets.

## Current Implementation Status

Done in this refactor:

- `CompileOptions` uses structured runtime/library fields directly
- `kernc` exposes the structured CLI flags directly
- `Craft.toml` supports a package-level `[runtime]` section
- `rt` owns startup/runtime glue
- `sys` owns platform/provider boundaries
- `std` is layered on top of `base` and `sys` without mirroring their namespaces
- `rt` is treated as a runtime companion layer injected only when a runtime entry contract is selected

## Next Steps

### Policy centralization

Next:

- let `craft` define profile/workspace runtime presets
- keep `kernc` explicit and mostly stateless
- reduce ad hoc per-command policy in downstream tools such as LSP

### Finer-grained library packaging

Later:

- define whether `std` stays one bundle or exposes finer library presets
- keep those presets ordinary tooling/library policy
- avoid reintroducing hidden compiler coupling

## Current Direction Summary

The intended steady state is simple:

- Kern the language is freestanding.
- `main` is a special root symbol only when a runtime entry contract is selected.
- `base`, `sys`, `rt`, and `std` are the only public library/runtime layers.
- `hosted` means "running with an OS process environment", not "depends on C".
- `std` stays libc-free and reaches hosted services through `sys`.
- `rt` owns low-level startup/runtime glue.
- `std` owns public reusable facilities.
- `craft` owns package policy.
- `kernc` executes explicit compile and link actions.
