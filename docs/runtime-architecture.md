# Runtime And Library Architecture

This document defines the runtime and library split used by the current 0.7.0 toolchain.

This document describes the current split that keeps Kern freestanding by
default while making hosted startup, toolchain-owned startup, libc usage, and
standard-library selection explicit and orthogonal.

## Design Goals

- keep the language itself freestanding
- make startup ownership explicit
- separate system-layer implementation choice from compiler runtime flags
- keep hosted OS interaction separate from libc linkage
- keep `kernc` as a low-level executor
- move package-level defaults and presets into `craft`
- avoid Rust-style "special crate" coupling between the compiler and a privileged `std/core` split

## Freestanding Means Libc-Free

In Kern, "freestanding" is a statement about dependency direction, not a statement
about whether useful libraries exist.

The dependency graph is:

- the language and compiler stand on their own
- `base` stands on its own
- `sys` is the operating-system and provider boundary
- `rt` is startup and minimal runtime glue
- `std` is ordinary Kern library code built on `base` plus `sys`
- `libc` is an optional external package choice

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
- select libc explicitly for performance, ABI, or ecosystem reasons

That makes `kern` a genuine alternative foundation rather than a thin front-end over C.

## The Current Axes

The current toolchain model uses three explicit compiler axes plus ordinary module selection.

### Runtime Entry

`runtime_entry` selects who owns the program entry contract.

- `none`: no program entry contract is synthesized
- `rt`: toolchain-owned runtime startup is enabled
- `crt`: the platform C runtime owns initial process startup

This axis decides whether the root module must provide a program `main`.

### Runtime Libc

`runtime_libc` is a direct yes/no switch for whether libc is linked.

This exists because "uses libc" and "uses CRT startup" are related but not identical concerns. The toolchain should be able to express both explicitly instead of hiding them behind one overloaded mode.

`runtime_libc` does not define whether hosted facilities exist. Hosted process access is modeled through the OS/provider boundary in `sys`; libc linkage is only one possible implementation choice for that boundary.

`runtime_libc` also does not define whether `std` exists. `std` is a normal Kern
library layer and remains valid without libc.

### Library Bundle

`library_bundle` selects which official library root aliases are added from the
shipped toolchain libraries.

- `none`
- `base`
- `std`

This is alias wiring only. It is not a prelude and it does not put names into
scope without `use`.

This bundle axis stays coarse-grained on purpose: startup semantics stay
separate from library selection.

### `sys` / `rt` Implementation Choice

The implementation behind `sys` or `rt` is not selected by a dedicated compiler runtime flag.

That choice belongs to ordinary module/package resolution:

- official toolchain roots may be selected explicitly through `--module-path`
- package tooling such as `craft` may wire `sys` or `rt` to project-provided packages
- custom kernels and embedded targets may provide their own `sys`, their own `rt`, or neither

This keeps `kernc` low-level and explicit while avoiding a privileged compiler-side "provider" model.

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

When this contract is enabled, the toolchain also loads `rt` as the startup companion root even if the source program never writes `use rt;`.

That behavior is intentionally narrow:

- it exists only so the selected startup shim can contribute `_start`, `main`, and related entry glue
- it is not a general visibility shortcut for runtime APIs
- it does not imply automatic `base` or `sys` injection
- explicit module imports remain mandatory for ordinary `rt.*`, `base.*`, `sys.*`, or `std.*` symbols

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

`std` does not mirror modules such as `std.coll`, `std.mem`, `std.cmp`, `std.hash`, `std.num`, `std.cffi`, `std.os`, or `std.rt`. Code should import `base.*`, `sys.*`, or `rt.*` directly when it needs those boundaries.

Before 1.0, Kern intentionally avoids carrying compatibility surface just to preserve superseded structure or spelling. The repository is kept on the current model only.

Kern does not use a Rust-style semantic split where the compiler secretly
relies on a special crate boundary. Library layering remains a normal toolchain
and package-architecture problem.

## Tooling Model

### `kernc`

`kernc` exposes the raw axes directly:

- `--runtime-entry`
- `--runtime-libc`
- `--library-bundle`
- `--entry-symbol` for raw linker entry selection when needed

`--entry-symbol` is intentionally lower-level than `runtime_entry`. It controls the final linker entry symbol and remains valid even when `runtime_entry = none`, which keeps naked freestanding builds fully explicit.

### `craft`

`craft` owns package-level defaults through `Craft.toml`:

```toml
[runtime]
entry = "rt"
libc = false
bundle = "std"
```

This is the place for project policy. Most users set runtime/library intent in
`Craft.toml` rather than repeating low-level `kernc` flags in every build
invocation.

`craft` also keeps the default runnable profile pure-first:

- `lib` defaults to `runtime_entry = none`, `runtime_libc = false`, `library_bundle = std`
- `bin`, `example`, and `test` default to `runtime_entry = rt`, `runtime_libc = false`, `library_bundle = std`
- libc / CRT startup remains an explicit opt-in policy choice, not the default executable baseline

## Windows Host Tools Versus Kern Runtime Policy

Windows is an easy place to blur unrelated layers together, so the separation
must stay explicit.

For Kern programs:

- `runtime_entry` controls who owns program startup
- `runtime_libc` controls whether libc is linked into the compiled program
- neither axis says anything about how the Rust host tools are distributed

For the shipped host tools (`kernc`, `craft`, `kern-lsp`):

- they are ordinary Windows user processes, not freestanding kernels
- official release archives use a static CRT build so the tools do not require
  the VC++ redistributable on a clean user machine
- that packaging policy is about tool distribution hygiene, not about Kern
  language semantics

This distinction matters:

- a static-CRT host tool can still import normal Win32 system DLLs
- that does not mean Kern secretly depends on libc
- it also does not mean `runtime_libc = yes`
- it only means the host executable is calling the Windows OS ABI directly, as
  any normal native Windows tool would

So the rule for Kern's philosophy is:

- pure-first still refers to the compiled Kern program and its runtime choices
- Windows host-tool packaging should avoid unnecessary redistributable baggage
- system-ABI imports for the host tools are acceptable when they reflect the
  real OS boundary rather than an avoidable extra runtime layer

## Summary

The model is simple:

- Kern the language is freestanding.
- `main` is a special root symbol only when a runtime entry contract is selected.
- `base`, `sys`, `rt`, and `std` are the only public library/runtime layers.
- `hosted` means "running with an OS process environment", not "depends on C".
- `std` stays libc-free and reaches hosted services through `sys`.
- `rt` owns low-level startup/runtime glue.
- `sys` and `rt` implementation choice is resolved through normal module/package wiring, not a dedicated compiler runtime flag.
- `std` owns public reusable facilities.
- `craft` owns package policy.
- `kernc` executes explicit compile and link actions.
