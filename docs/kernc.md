# The `kernc` Compiler Guide

This document describes how to use `kernc`, the Kern compiler driver.

`kernc` is a compiler and linker driver, not a package manager. It accepts
enough explicit configuration to fit into different build environments, while
leaving dependency resolution, package graphs, caching, and workspace
orchestration to higher-level tooling such as `craft`.

## Scope and Responsibilities

`kernc` is responsible for:

- Parsing, analyzing, lowering, and code generating a single Kern source entry point.
- Emitting LLVM IR for inspection at explicit pipeline stages.
- Emitting a linker input artifact such as a native object file or ThinLTO prelink bitcode.
- Invoking a system linker driver with explicit inputs and link configuration.

`kernc` is not responsible for:

- Resolving package versions.
- Downloading dependencies.
- Building a workspace dependency graph.
- Performing artifact caching across packages or targets.
- Acting as a lockfile-aware package manager.

In practice, higher-level tooling decides what to build and in which order,
while `kernc` executes a well-defined compile or link action with explicit
parameters.

## Command Synopsis

```text
kernc [OPTIONS] [input.rn]
```

The positional source input is required for compile modes and forbidden in link-only mode.

## Driver Modes

`kernc` exposes four explicit driver modes:

- Default mode: compile the source input and then link the final binary.
- `-c`: compile only. Emit a linker input artifact and stop before the final system link step.
- `--emit-llvm[=raw|verified|optimized]`: compile only and print LLVM IR to stdout.
- `--link-only`: skip the frontend and code generation stages and invoke the linker driver using explicit linker inputs.

These modes are mutually exclusive.

## Basic Examples

Build a Kern program with the Kern standard library:

```bash
kernc --runtime-entry rt --library-bundle std examples/hello_world.rn -o hello
```

Compile only and keep the object file:

```bash
kernc -c --runtime-entry rt --library-bundle std examples/hello_world.rn -o hello.o
```

Inspect raw generated LLVM IR:

```bash
kernc --emit-llvm --runtime-entry rt --library-bundle std examples/hello_world.rn
```

Inspect LLVM IR after target setup and LLVM optimization passes:

```bash
kernc --emit-llvm=optimized -O2 --runtime-entry rt --library-bundle std examples/hello_world.rn
```

Link an existing object file:

```bash
kernc --link-only --link-input hello.o -o hello
```

Split compile and link explicitly:

```bash
kernc -c --runtime-entry rt --library-bundle std app.rn -o app.o
kernc --link-only --link-input app.o --entry-symbol _start -o app
```

## Source and Module Configuration

### Source Input

Compile modes accept one positional `.rn` input file:

```bash
kernc --runtime-entry rt --library-bundle std src/main.rn -o app
```

`--link-only` does not accept a source input. Use `--link-input` instead.

### Module Aliases

Use `--module-path <name=path>` to map a module root name to a physical path:

```bash
kernc --module-path std=./library/std app.rn
```

This is the core mechanism for wiring module roots into the compiler. It is
intentionally explicit and easy for package tools to drive.

### Runtime And Library Axes

Prefer the structured runtime/library flags:

- `--runtime-entry <none|rt|crt>`
- `--runtime-libc <yes|no>`
- `--library-bundle <none|base|std>`

`--library-bundle std` enables the Kern standard library bundle and maps the
official `std` root alias if no manual `--module-path std=...` mapping is
provided.

The official library roots are:

- `base`: runtime-independent foundation facilities
- `sys`: provider and OS boundaries
- `rt`: startup and minimal runtime glue
- `std`: high-level user-facing facilities

Configured alias wiring intentionally exposes only the public library surface:

- `base` is injected only for explicit `--library-bundle base` or `--library-bundle std`
- `sys` is injected only for explicit `--library-bundle std`
- `std` is injected only for `--library-bundle std`
- `rt` is not injected by library bundle selection alone; `kernc` injects it only as the companion runtime root when `runtime_entry != none`

The `rt` companion-root rule is startup wiring, not ordinary name injection:

- it makes the `library/rt` root available so hosted startup symbols such as `_start` or `main` can be linked
- it does not auto-import `rt.*` APIs into user scope
- it does not auto-inject `base` or `sys`
- ordinary runtime/library APIs still require explicit `use` like any other module

`kernc` resolves the official library paths through these environment variables first:

1. `KERN_STD_PATH`
2. `KERN_BASE_PATH`
3. `KERN_SYS_PATH`
4. `KERN_RT_PATH`

Each root then falls back to a path relative to the current executable and finally to `library/<name>` in the repository layout.

The model is:

- library choice is independent from startup ownership
- libc linkage is independent from whether `std` is available
- hosted process access is provided through `sys`, not implied by libc linkage
- startup shims live under `rt`, not under `std`
- `sys` and `rt` implementation choice is handled through ordinary module paths or packages, not a dedicated runtime-provider flag
- low-level APIs stay in their owning layer instead of being mirrored through `std`

If you select `--runtime-entry` without selecting an official library bundle, `kernc` only wires `rt` itself. If that `rt` implementation depends on `base` or `sys`, map them explicitly with `--module-path` or choose a bundle.

When `--runtime-entry rt` or `--runtime-entry crt` is active, the root `main` must match the program-entry contract: `fn main() i32` or `fn main(argc: i32, argv: **u8) i32`.

`kernc` exposes the four structured axes directly. Configure runtime and library policy through those axes rather than through compatibility aliases.

## Compilation Controls

### Output File

Use `-o <file>` to set the output path.

Default output names:

- Compile-and-link: `a.out`
- Compile-only: `<input-stem>.o`

### Optimization Level

Use one of:

- `-O0`
- `-O1`
- `-O2`
- `-O3`

### Codegen Units And LTO

Use `--codegen-units <N>` to partition lowering/codegen into multiple codegen
units.

Use `--lto <none|full|thin>` to control cross-CGU optimization:

- `none`: keep the ordinary multi-object path
- `full`: merge partitioned LLVM modules back into one whole-program module and
  run the LLVM module pipeline once
- `thin`: keep partitioned codegen, but enable the compiler-owned summary and
  cross-CGU import/export planning path

Current explicit boundary:

- `--emit-llvm` with `--codegen-units > 1` requires `--lto full`
- preserved per-CGU linker-input directories are incompatible with `--lto full`
- `--link-only` cannot perform LTO

### Target Triple

Use `--target <triple>` to select a target triple:

```bash
kernc --target x86_64-unknown-linux-gnu --runtime-entry rt --library-bundle std app.rn -o app
```

The target triple affects:

- Conditional compilation pruning
- Pointer size and layout decisions
- LLVM target selection
- Platform-specific default link behavior

### Assembly Dialect

Use `--asm-dialect intel` or `--asm-dialect att` to configure inline assembly formatting.

### Conditional Compilation Defines

Use `--define <key=value>` to feed custom values into compile-time attribute pruning:

```bash
kernc --define debug_mode=true --define board=qemu app.rn
```

These values are available to `#[if(...)]` and `#![if(...)]` conditions handled by the frontend pruning pass.

In addition to user-provided `--define` values, `kernc` injects a small set of driver-controlled condition variables:

- `runtime_entry`: one of `"none"`, `"rt"`, or `"crt"`
- `library_bundle`: one of `"none"`, `"base"`, or `"std"`
- `libc`: `true` when libc linkage is enabled
- `crt_startup`: `true` when CRT startup owns initial process entry
- `rt_role`: toolchain-controlled role selection for `rt`

## Linking Model

Kern uses explicit language-level ABI boundaries and explicit driver-level link configuration.

At the language level:

- `extern` defines a C ABI boundary.
- `#[export_name("...")]` overrides the exported symbol name.
- `#[link_section("...")]` selects the output section for a function or global.

At the driver level:

- `kernc` decides whether to compile, link, or do both.
- Linker inputs, library paths, libraries, entry symbols, and raw linker arguments are all explicit CLI configuration.

This split is intentional. Symbol shape belongs to the language and semantic pipeline. Final artifact composition belongs to the driver and to higher-level tooling that drives it.

## Explicit Linker Configuration

### Linker Driver

Use `--link-driver <cmd>` to select the linker driver command:

```bash
kernc --link-driver clang --runtime-entry rt --library-bundle std app.rn -o app
```

### Additional Link Inputs

Use `--link-input <path>` to add object files, archives, shared libraries, or response files:

```bash
kernc --link-only \
  --link-input app.o \
  --link-input runtime.o \
  --link-input libsupport.a \
  -o app
```

### Library Search Paths

Use `-L <dir>`:

```bash
kernc --link-only --link-input app.o -L ./out/lib -o app
```

### Libraries

Use `-l <name>`:

```bash
kernc --link-only --link-input app.o -L ./out/lib -l support -o app
```

### Raw Linker Arguments

Use `--link-arg <arg>` when an exact driver argument must be forwarded:

```bash
kernc --link-only \
  --link-input app.o \
  --link-arg -nostdlib \
  --link-arg -Wl,--gc-sections \
  -o app
```

### Entry Symbol

Use `--entry-symbol <symbol>` to set the final linker entry symbol explicitly. This is independent from the language-level `main` contract and can be used in naked freestanding builds where `runtime_entry = none`.

```bash
kernc --link-only \
  --entry-symbol boot_main \
  --link-input kernel.o \
  -o kernel.bin
```

### Print the Final Link Command

Use `--print-link-command` to inspect the resolved system linker invocation:

```bash
kernc --link-only \
  --print-link-command \
  --link-input app.o \
  -o app
```

This is especially useful for build scripts and for `craft` or other external
build tooling.

## Recommended Usage Patterns

### Small Direct Builds

Use compile-and-link directly:

```bash
kernc --runtime-entry rt --library-bundle std app.rn -o app
```

### Build-System Integration

Split the pipeline explicitly:

```bash
kernc -c --target x86_64-unknown-linux-gnu app.rn -o app.o
kernc --link-only --link-input app.o --entry-symbol boot_main --link-arg ... -o app
```

### Build Tool Integration

Package tools treat `kernc` as an execution backend:

1. Resolve the package graph.
2. Build the dependency order.
3. Call `kernc -c` for each compilation unit or final package target.
4. Call `kernc --link-only` with the exact object files, archives, search paths, and policy required for the final artifact.

That separation keeps policy in the package manager and keeps `kernc` deterministic and reusable.

## CLI Reference

### Build Options

- `-o <file>`: write output to `<file>`
- `-c`: emit linker input and skip the final system link step
- `--link-only`: skip frontend/codegen and invoke the linker driver only
- `--define <key=val>`: define a variable for conditional compilation
- `--module-path <name=path>`: map a module name to a physical directory
- `--module-interface-path <name=path>`: map a module name to an imported metadata root
- `--metadata-output <dir>`: emit a module metadata snapshot directory
- `--module-root-name <name>`: override the compiled root module name
- `-O0` to `-O3`: select optimization level

### Targeting and Code Generation

- `--target <T>`: set the target triple
- `--asm-dialect <D>`: set the inline assembly dialect
- `--emit-llvm[=raw|verified|optimized]`: print LLVM IR to stdout

### Linking

- `--link-driver <cmd>`: set the linker driver command
- `--runtime-entry <m>`: select `none`, `rt`, or `crt`
- `--runtime-libc <yes|no>`: control whether libc is linked
- `--library-bundle <b>`: select `none`, `base`, or `std`
- `--link-input <path>`: add an extra linker input
- `--link-search <dir>`: add a linker search path
- `--link-lib <name>`: link against a library
- `-L <dir>`: add a linker search path
- `-l <name>`: link against a library
- `--link-arg <arg>`: pass a raw linker argument
- `--entry-symbol <symbol>`: set the final linker entry symbol explicitly
- `--print-link-command`: print the resolved link command
- `--timings`: print compiler phase timings

### Information

- `-h`, `--help`: print help
- `-v`, `--version`: print compiler version

## See Also

- [Runtime And Library Architecture](./runtime-architecture.md)
- [Kern Language Design Document](./design.md)
- [Project README](../README.md)
