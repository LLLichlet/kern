# Runtime And Library Architecture

This document defines the runtime and library split introduced after the v0.6.7 `main` cleanup.

The goal is to keep Kern freestanding by default while making hosted startup, toolchain-owned startup, libc usage, and standard-library selection explicit and orthogonal.

## Design Goals

- keep the language itself freestanding
- make startup ownership explicit
- separate runtime provider choice from library choice
- keep `kernc` as a low-level executor
- move package-level defaults and presets into `craft`
- avoid Rust-style "special crate" coupling between the compiler and a privileged `std/core` split

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

Kern should not grow a Rust-style semantic split where the compiler secretly relies on a special crate boundary. Library layering remains a normal toolchain and package-architecture problem.

## Tooling Model

### `kernc`

`kernc` should expose the raw axes directly:

- `--runtime-entry`
- `--runtime-provider`
- `--runtime-libc`
- `--library-bundle`
- `--entry` for raw linker entry selection when needed

`--entry` is intentionally lower-level than `runtime_entry`. It controls the final linker entry symbol and remains valid even when `runtime_entry = none`, which keeps naked freestanding builds fully explicit.

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
- `std` is layered on top of `base`, `sys`, and `rt`

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
- `rt` owns low-level startup/runtime glue.
- `std` owns public reusable facilities.
- `craft` owns package policy.
- `kernc` executes explicit compile and link actions.
