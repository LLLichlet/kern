# The `kernc` Compiler Guide

This document describes how to use `kernc`, the Kern compiler driver.

`kernc` is intentionally positioned as a compiler and linker driver, not as a package manager. It should be able to accept enough explicit configuration to fit into many build environments, while leaving dependency resolution, package graphs, caching, and workspace orchestration to a future dedicated package manager.

## Scope and Responsibilities

`kernc` is responsible for:

- Parsing, analyzing, lowering, and code generating a single Kern source entry point.
- Emitting LLVM IR for inspection.
- Emitting a linker input artifact such as an object file.
- Invoking a system linker driver with explicit inputs and link configuration.

`kernc` is not responsible for:

- Resolving package versions.
- Downloading dependencies.
- Building a workspace dependency graph.
- Performing artifact caching across packages or targets.
- Acting as a lockfile-aware package manager.

In practice, this means a future package manager should decide what to build and in which order, while `kernc` should execute a well-defined compile or link action with explicit parameters.

## Command Synopsis

```text
kernc [OPTIONS] [input.rn]
```

The positional source input is required for compile modes and forbidden in link-only mode.

## Driver Modes

`kernc` exposes four explicit driver modes:

- Default mode: compile the source input and then link the final binary.
- `-c`: compile only. Emit a linker input artifact and stop before the final system link step.
- `--emit-llvm`: compile only and print LLVM IR to stdout.
- `--link-only`: skip the frontend and code generation stages and invoke the linker driver using explicit linker inputs.

These modes are mutually exclusive.

## Basic Examples

Build a Kern program with the Kern standard library:

```bash
kernc --use-std examples/hello_world.rn -o hello
```

Compile only and keep the object file:

```bash
kernc -c --use-std examples/hello_world.rn -o hello.o
```

Inspect generated LLVM IR:

```bash
kernc --emit-llvm --use-std examples/hello_world.rn
```

Link an existing object file:

```bash
kernc --link-only --link-input hello.o -o hello
```

Split compile and link explicitly:

```bash
kernc -c --use-std app.rn -o app.o
kernc --link-only --link-profile kern --link-input app.o -o app
```

## Source and Module Configuration

### Source Input

Compile modes accept one positional `.rn` input file:

```bash
kernc --use-std src/main.rn -o app
```

`--link-only` does not accept a source input. Use `--link-input` instead.

### Module Aliases

Use `-M <name=path>` to map a module root name to a physical path:

```bash
kernc -M std=./library/std app.rn
```

This is the core mechanism for wiring module roots into the compiler. It is intentionally explicit and package-manager-friendly.

### Standard Library Injection

`--use-std` enables the Kern standard library and automatically maps `std` if no manual `-M std=...` mapping is provided.

`kernc` resolves the standard library path in this order:

1. `KERN_STD_PATH`
2. A path relative to the current executable
3. `library/std` in the repository layout

`--use-std` can be combined with hosted C library linkage.

When `--link-profile hosted` (or `--link-libc`) is active, `kernc` still injects `std`, but the standard library can prune Kern-specific runtime entry shims via compile-time conditions such as `#[if(kern_rt)]`. This allows `std` facilities like `std.io` or allocators to coexist with a hosted C startup without forcing the Kern `_start` / `mainCRTStartup` path.

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

### Target Triple

Use `--target <triple>` to select a target triple:

```bash
kernc --target x86_64-unknown-linux-gnu --use-std app.rn -o app
```

The target triple affects:

- Conditional compilation pruning
- Pointer size and layout decisions
- LLVM target selection
- Platform-specific default link behavior

### Assembly Dialect

Use `--asm-dialect intel` or `--asm-dialect att` to configure inline assembly formatting.

### Conditional Compilation Defines

Use `-D <key=value>` to feed custom values into compile-time attribute pruning:

```bash
kernc -D debug_mode=true -D board=qemu app.rn
```

These values are available to `#[if(...)]` and `#![if(...)]` conditions handled by the frontend pruning pass.

In addition to user-provided `-D` values, `kernc` injects a small set of driver-controlled condition variables:

- `link_profile`: one of `"kern"`, `"freestanding"`, `"hosted"`, or `"none"`
- `hosted`: `true` when using the hosted link profile
- `libc`: currently mirrors `hosted`
- `kern_rt`: `true` when `std` should provide Kern runtime entry shims

## Linking Model

Kern uses explicit language-level ABI boundaries and explicit driver-level link configuration.

At the language level:

- `extern` defines a C ABI boundary.
- `#[export_name("...")]` overrides the exported symbol name.
- `#[link_section("...")]` selects the output section for a function or global.

At the driver level:

- `kernc` decides whether to compile, link, or do both.
- Linker inputs, library paths, libraries, entry symbols, and raw linker arguments are all explicit CLI configuration.

This split is intentional. Symbol shape belongs to the language and semantic pipeline. Final artifact composition belongs to the driver and, eventually, to higher-level tooling.

## Link Profiles

Use `--link-profile <profile>` to select the default link policy:

- `kern`: Kern-oriented defaults. This is the current default.
- `freestanding`: no hosted runtime assumptions, but without Kern-specific platform libraries.
- `hosted`: hosted C environment defaults.
- `none`: no default link flags at all.

Examples:

```bash
kernc --link-profile hosted app.rn -o app
```

```bash
kernc --link-only --link-profile none --link-input app.o --link-arg -Wl,--gc-sections -o app
```

Compatibility aliases:

- `--link-libc` is equivalent to `--link-profile hosted`
- `--no-default-link-flags` is equivalent to `--link-profile none`

## Explicit Linker Configuration

### Linker Driver

Use `--cc <cmd>` or `--linker <cmd>` to select the linker driver command:

```bash
kernc --linker clang --use-std app.rn -o app
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
  --link-profile none \
  --link-input app.o \
  --link-arg -nostdlib \
  --link-arg -Wl,--gc-sections \
  -o app
```

### Entry Symbol

Use `--entry <symbol>` to override the default entry symbol used by `kernc`'s platform link profiles:

```bash
kernc --link-only \
  --link-profile freestanding \
  --entry boot_main \
  --link-input kernel.o \
  -o kernel.bin
```

### Print the Final Link Command

Use `--print-link-command` to inspect the resolved system linker invocation:

```bash
kernc --link-only \
  --print-link-command \
  --link-profile kern \
  --link-input app.o \
  -o app
```

This is especially useful for build scripts and future package-manager integration.

## Recommended Usage Patterns

### Small Direct Builds

Use compile-and-link directly:

```bash
kernc --use-std app.rn -o app
```

### Build-System Integration

Split the pipeline explicitly:

```bash
kernc -c --target x86_64-unknown-linux-gnu app.rn -o app.o
kernc --link-only --link-profile none --link-input app.o --link-arg ... -o app
```

### Future Package Manager Integration

A future package manager should treat `kernc` as an execution backend:

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
- `-D <key=val>`: define a variable for conditional compilation
- `-M <name=path>`: map a module name to a physical directory
- `-O0` to `-O3`: select optimization level

### Targeting and Code Generation

- `--target <T>`: set the target triple
- `--asm-dialect <D>`: set the inline assembly dialect
- `--emit-llvm`: print LLVM IR to stdout

### Linking

- `--cc <cmd>`: set the linker driver command
- `--linker <cmd>`: alias for `--cc`
- `--link-profile <p>`: select `kern`, `freestanding`, `hosted`, or `none`
- `--link-input <path>`: add an extra linker input
- `-L <dir>`: add a linker search path
- `-l <name>`: link against a library
- `--link-arg <arg>`: pass a raw linker argument
- `--entry <symbol>`: override the entry symbol used by `kernc`
- `--print-link-command`: print the resolved link command
- `--no-default-link-flags`: alias for `--link-profile none`
- `--link-libc`: alias for `--link-profile hosted`

### Standard Library

- `--use-std`: enable the Kern standard library; hosted links automatically prune `std.rt` entry shims

### Information

- `-h`, `--help`: print help
- `-v`, `--version`: print compiler version

## See Also

- [Kern Language Design Document](./design.md)
- [Project README](../README.md)
