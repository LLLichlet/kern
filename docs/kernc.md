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
kernc [OPTIONS] [input.kn]
```

The positional source input is required for compile modes and forbidden in link-only mode.

## Driver Modes

`kernc` exposes four explicit driver modes:

- Default mode: compile the source input and then link the final binary.
- `-c`: compile only. Emit a linker input artifact and stop before the final system link step.
- `--cc`: compile a C-family source file to a native object file and stop.
- `--emit-llvm[=raw|verified|optimized]`: compile only and print LLVM IR to stdout.
- `--link-only`: skip the frontend and code generation stages and invoke the linker driver using explicit linker inputs.

These modes are mutually exclusive.

## Common Workflows

If you are not integrating `kernc` into another build system yet, start from
one of these concrete patterns.

### Hosted User Program

Build and link one source file with the hosted `rt` startup path and the Kern
standard library bundle:

```bash
kernc \
  --runtime-entry rt \
  --runtime-libc no \
  --library-bundle std \
  src/main.kn \
  -o app
```

### Compile Only, Then Link Later

This is the right split when another tool wants to inspect or stage the object
file before the final link:

```bash
kernc -c \
  --runtime-entry rt \
  --runtime-libc no \
  --library-bundle std \
  src/main.kn \
  -o app.o
```

```bash
kernc --link-only \
  --link-input app.o \
  -o app
```

### Freestanding `_start` Binary

For kernels, boot stages, or other freestanding binaries, disable the runtime
entry contract and export your own entry symbol:

```kern
#[export_name("_start")]
fn kmain() void {
    while true {}
    @unreachable();
}
```

```bash
kernc \
  --runtime-entry none \
  --runtime-libc no \
  --library-bundle base \
  --entry-symbol _start \
  src/main.kn \
  -o kernel.bin
```

With `--runtime-entry none`, `kernc` does not require a program `main`.

### Custom Linker Script

When the final artifact must be linked with your own script, pass the linker
script through explicitly:

```bash
kernc \
  --runtime-entry none \
  --runtime-libc no \
  --library-bundle base \
  --entry-symbol _start \
  --link-arg -T \
  --link-arg kernel.ld \
  src/main.kn \
  -o kernel.bin
```

If the linker script path is not relative to your current working directory,
pass an absolute path instead.

## Basic Examples

Build a Kern program with the Kern standard library:

```bash
kernc --runtime-entry rt --library-bundle std examples/hello_world.kn -o hello
```

Compile only and keep the object file:

```bash
kernc -c --runtime-entry rt --library-bundle std examples/hello_world.kn -o hello.o
```

Inspect raw generated LLVM IR:

```bash
kernc --emit-llvm --runtime-entry rt --library-bundle std examples/hello_world.kn
```

Inspect LLVM IR after target setup and LLVM optimization passes:

```bash
kernc --emit-llvm=optimized -O2 --runtime-entry rt --library-bundle std examples/hello_world.kn
```

Compile a C-family source into an object with the resolved SDK C compiler:

```bash
kernc --cc native/support.c -o support.o
```

When the link driver is left at the default `cc`, Kern treats that default as an
SDK-owned driver slot and resolves it to the active SDK/toolchain `clang`. It
does not fall back to the host `cc` when SDK clang is missing. Repair the SDK or
set `KERN_TOOLCHAIN_ROOT`/`--toolchain-root`; use `--link-driver` or `CC` only
when you intentionally want an external driver.

Link an existing object file:

```bash
kernc --link-only --link-input hello.o -o hello
```

Split compile and link explicitly:

```bash
kernc -c --runtime-entry rt --library-bundle std app.kn -o app.o
kernc --link-only --link-input app.o --entry-symbol _start -o app
```

## Source and Module Configuration

### Source Input

Compile modes accept one positional `.kn` input file:

```bash
kernc --runtime-entry rt --library-bundle std src/main.kn -o app
```

`--link-only` does not accept a source input. Use `--link-input` instead.

### Module Aliases

Use `--module-path <name=path>` to map a module root name to a physical path:

```bash
kernc --module-path std=./library/std app.kn
```

In a source checkout this path is inside the in-tree `library/` workspace.
Installed SDKs normally use `--library-bundle` or `KERNLIB_PATH` instead of a
repository-relative path.

This is the core mechanism for wiring module roots into the compiler. It stays
explicit and easy for package tools to drive.

### Runtime And Library Axes

Prefer the structured runtime/library flags:

- `--runtime-entry <none|rt|crt>`
- `--runtime-libc <yes|no>`
- `--library-bundle <none|base|std>`

`--library-bundle std` enables the Kern standard library bundle and maps the
official `std` root alias if no manual `--module-path std=...` mapping is
provided.

The official roots derived from that workspace are:

- `base`: runtime-independent foundation facilities
- `rt`: startup and minimal runtime glue
- `std`: high-level user-facing facilities

Configured alias wiring intentionally exposes only the public library surface:

- `base` is injected only for explicit `--library-bundle base` or `--library-bundle std`
- `std` is injected only for `--library-bundle std`
- `rt` is not injected by library bundle selection alone; `kernc` injects it only as the companion runtime root when `runtime_entry != none`

The `rt` companion-root rule is startup wiring, not ordinary name injection:

- it makes the `library/rt` root available so hosted startup symbols such as `_start` or `main` can be linked
- it does not auto-import `rt.*` APIs into user scope
- it does not auto-inject `base`
- ordinary runtime/library APIs still require explicit `use` like any other module

`kernc` resolves official library roots from one official Kern library
workspace. `KERNLIB_PATH` may point at that workspace root, which must contain
`Craft.toml` plus the `base`, `rt`, and `std` member directories. Without
`KERNLIB_PATH`, `kernc` searches for `library` relative to the current
executable, then for the SDK layout at `lib/kern`, and finally falls back to
`library` in the repository layout. In a source checkout, `library/` is checked
in as the official library workspace.

The model is:

- library choice is independent from startup ownership
- libc linkage is independent from whether `std` is available
- hosted process access is provided through hosted services in `std.host`, not implied by libc linkage
- startup shims live under `rt`, not under `std`
- `rt` implementation choice is handled through ordinary module paths or packages, not a dedicated runtime-selection flag
- low-level APIs stay in their owning layer instead of being mirrored through `std`

If you select `--runtime-entry` without selecting an official library bundle,
`kernc` only wires `rt` itself. The official `rt` is kept independent from
`base` and `std` so this remains valid with `--library-bundle none`.

When `--runtime-entry rt` or `--runtime-entry crt` is active, the root `main` must match the program-entry contract: `fn main() i32` or `fn main(argc: i32, argv: &&u8) i32`.

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
- ThinLTO compile-only native-object preservation keeps the planned per-CGU
  native linker inputs when they remain directly linkable, and otherwise falls
  back to a single merged native linker input. If higher-level tooling needs
  the exact ThinLTO prelink artifacts instead, keep the linker inputs as
  bitcode rather than native objects.

### Target Triple

Use `--target <triple>` to select a target triple:

```bash
kernc --target x86_64-unknown-linux-gnu --runtime-entry rt --library-bundle std app.kn -o app
```

The target triple affects:

- Conditional compilation pruning
- Pointer size and layout decisions
- LLVM target selection
- Platform-specific default link behavior

### Windows Host-Tool Distribution Notes

Do not conflate three different things on Windows:

- the Kern program being compiled
- the Rust host tool that is doing the compilation (`kernc`, `craft`, `kern-lsp`)
- the final package/distribution policy for those host tools

`--runtime-entry`, `--runtime-libc`, `--entry-symbol`, and library-bundle
selection describe the **compiled Kern program**. They do not control whether
the Rust host tools themselves link the MSVC CRT statically or dynamically.

For the host tools:

- a plain `cargo build --release` on `x86_64-pc-windows-msvc` may still produce
  binaries that depend on `VCRUNTIME140*.dll` and the UCRT redistributable set
- a source build still needs a full LLVM 21 development prefix; the installed
  end-user SDK intentionally does not contain all source-build LLVM assets
- official Windows release packaging must therefore build with
  `-C target-feature=+crt-static`
- that static-CRT policy removes the VC++ redistributable dependency for the
  shipped host tools, but it does **not** remove ordinary Win32 system-library
  imports such as `KERNEL32.dll`, `ADVAPI32.dll`, `SHELL32.dll`, `ole32.dll`,
  `bcryptprimitives.dll`, or `api-ms-win-core-synch-l1-2-0.dll`

Those remaining imports are host-OS ABI dependencies, not hidden libc baggage
in Kern's language/runtime model.

The official Windows packaging path is therefore:

```powershell
$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = "-C target-feature=+crt-static"
cargo build --release --target x86_64-pc-windows-msvc -p kernc_cli --bin kernc
cargo build --release --target x86_64-pc-windows-msvc -p craft
cargo build --release --target x86_64-pc-windows-msvc -p kern-lsp
```

Or, equivalently, use the Rust repository worker:

```powershell
cargo run -q -p kernworker -- release package --version v0.7.8 --target x86_64-windows-msvc
```

For the local source-build environment, including `LLVM_SYS_211_PREFIX` and the
Windows `libxml2s.lib`/vcpkg setup, see
[Windows Distribution Guide](./windows-distribution.md#local-development-build).

Two Windows-specific details are easy to miss:

- the archive label is `x86_64-windows-msvc`
- the actual Cargo target triple is `x86_64-pc-windows-msvc`

The packaging script handles that translation and packages from
`target/x86_64-pc-windows-msvc/release/`, not from `target/release/`.

Current practical boundary:

- official Windows host-tool archives are meant for modern Windows systems
- very old Windows versions should not be promised implicitly just because the
  binaries no longer depend on the VC++ redistributable
- static CRT solves the redistributable problem; it does not erase the Win32
  API baseline required by the host tools

See also:

- [Windows Distribution Guide](./windows-distribution.md)
- [Unix Distribution Guide](./unix-distribution.md)

### Unix Host-Tool Distribution Notes

Do not conflate three different things on Linux/macOS:

- the Kern program being compiled
- the Rust host tool that is doing the compilation (`kernc`, `craft`, `kern-lsp`)
- the final package/distribution policy for those host tools

`--runtime-entry`, `--runtime-libc`, `--entry-symbol`, and library-bundle
selection still describe the **compiled Kern program**. They do not mean the
Unix host tools themselves are fully static or universally portable.

Current practical boundary:

- official Unix host-tool archives are host-native release artifacts
- the Linux release baseline is currently the pinned `ubuntu-24.04` workflow
  host, not a promise of universal glibc portability
- the macOS release baselines are currently `macos-15-intel` for
  `x86_64-apple-darwin` and `macos-14` for `aarch64-apple-darwin`
- official Unix installers must verify that `kernc`, `craft`, and `kern-lsp`
  actually start before claiming success
- older or more minimal Unix systems may need additional runtime libraries or a
  source build on the target machine

For the full policy and packaging constraints, see:

- [Unix Distribution Guide](./unix-distribution.md)

### Assembly Dialect

Use `--asm-dialect intel` or `--asm-dialect att` to configure inline assembly formatting.

### Conditional Compilation Defines

Use `--define <key=value>` to feed custom values into compile-time attribute pruning:

```bash
kernc --define debug_mode=true --define board=qemu app.kn
```

These values are available to `#[if ...]` and `#![if ...]` conditions handled by the frontend pruning pass.

In addition to user-provided `--define` values, `kernc` injects a small set of driver-controlled condition variables:

- `runtime_entry`: one of `"none"`, `"rt"`, or `"crt"`
- `library_bundle`: one of `"none"`, `"base"`, or `"std"`
- `libc`: `true` when libc linkage is enabled
- `crt_startup`: `true` when CRT startup owns initial process entry
- `rt_role`: toolchain-controlled role selection for `rt`
- `test`: `true` only when compiling in test mode

### Test Mode

`--test-mode` asks the compiler to build a test adapter instead of the normal root `main` adapter. In this mode, `#[test]` functions are collected as test cases and `#[if test]` content is retained.

Legal test functions are:

- `#[test] fn name() i32`
- `#[test] fn name(argc: i32, argv: &&u8) i32`

`--test-metadata-output <file>` writes the discovered case manifest:

```text
version=1
case=0	alpha
case=1	nested::beta
```

The adapter is a private compiler/tooling protocol. Tools invoke one case per process and pass user test arguments through to that case's `(argc, argv)` when requested. Case return value `0` means pass; any other value means fail.

## Linking Model

Kern uses explicit language-level ABI boundaries and explicit driver-level link configuration.

At the language level:

- `extern` defines a C ABI boundary.
- `#[export_name("...")]` overrides the exported symbol name.
- `#[link_section("...")]` selects the output section for a function or global.
- `#[retain]` keeps a function or global in the output even if no reachable Kern code references it.

At the driver level:

- `kernc` decides whether to compile, link, or do both.
- Linker inputs, library paths, libraries, entry symbols, and raw linker arguments are all explicit CLI configuration.

This split is intentional. Symbol shape belongs to the language and semantic pipeline. Final artifact composition belongs to the driver and to higher-level tooling that drives it.

## Explicit Linker Configuration

### Linker Driver

By default Kern links through the active SDK/toolchain `clang`. Use
`--link-driver <cmd>` to explicitly select an external linker driver command:

```bash
kernc --link-driver clang --runtime-entry rt --library-bundle std app.kn -o app
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
kernc --runtime-entry rt --library-bundle std app.kn -o app
```

### Build-System Integration

Split the pipeline explicitly:

```bash
kernc -c --target x86_64-unknown-linux-gnu app.kn -o app.o
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
- `--cc`: compile a C-family source to a native object and skip the Kern frontend
- `--link-only`: skip frontend/codegen and invoke the linker driver only
- `--define <key=val>`: define a variable for conditional compilation
- `--module-path <name=path>`: map a module name to a physical directory
- `--module-interface-path <name=path>`: map a module name to an imported metadata root
- `--metadata-output <dir>`: emit a module metadata snapshot directory
- `--test-mode`: compile a test target and enable the driver-controlled `test` condition
- `--test-metadata-output <file>`: emit the discovered test case manifest
- `--module-root-name <name>`: override the compiled root module name
- `-O0` to `-O3`: select optimization level

### Targeting and Code Generation

- `--target <T>`: set the target triple
- `--asm-dialect <D>`: set the inline assembly dialect
- `--emit-llvm[=raw|verified|optimized]`: print LLVM IR to stdout

### Linking

- `--link-driver <cmd>`: explicitly set an external linker driver command
- `--runtime-entry <m>`: select `none`, `rt`, or `crt`
- `--runtime-libc <yes|no>`: control whether libc is linked
- `--library-bundle <b>`: select `none`, `base`, or `std`
- `--link-input <path>`: add an extra linker input
- `--link-search <dir>`: add a linker search path
- `--link-lib <name>`: link against a library
- `-L <dir>`: add a linker search path
- `-l <name>`: link against a library
- `--link-arg <arg>`: pass a raw linker argument
- `--cc-arg <arg>`: pass a raw C compiler argument when using `--cc`
- `--entry-symbol <symbol>`: set the final linker entry symbol explicitly
- `--print-link-command`: print the resolved link command
- `--timings`: print compiler phase timings

### Information

- `-h`, `--help`: print help
- `-v`, `--version`: print compiler version

## See Also

- [Runtime And Library Architecture](./runtime-architecture.md)
- [Windows Distribution Guide](./windows-distribution.md)
- [Kern Language Design Document](./design.md)
- [Project README](../README.md)
