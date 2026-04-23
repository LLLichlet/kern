# Kern Programming Language

![Kern logo](./logo.svg)

> **Status:** v0.7.0 (Experimental)
> *High Abstraction, Low Policy.*

Kern is a systems-level programming language explicitly designed for operating system kernels, embedded firmware, and high-performance infrastructure.

Kern is built on the observation that languages often trade off abstraction capabilities against policy constraints. Kern aims to occupy the "fourth quadrant": providing powerful, modern abstractions (like explicit module systems, generics, and algebraic data types) while enforcing virtually **zero hidden runtime policies**. There are no implicit allocations, no garbage collection, no exceptions, and no closures with hidden captures.

## Core Philosophy

* **Clarity over Novelty:** What you write is what the machine executes. Kern removes C's declaration and decay hazards while maintaining entirely predictable assembly generation.
* **Explicit over Implicit:** Mutability is a property of storage, not the type itself (`let mut x`). Pointer math is strictly typed. All type conversions require the explicit `as` operator. Return values cannot be silently ignored.
* **Zero-Cost Abstractions:** Features like monomorphized Generics, Algebraic Data Types (`enum`), and strictly Stateless Lambdas compile down to highly optimized, flat LLVM IR with zero runtime overhead.
* **Mechanism Trinity:** Kern relies on three core mechanisms to maintain its philosophy: a strictly explicit module tree (`mod`), strongly-typed zero-cost generics, and precise state management via exhaustive `match` blocks.
* **Freestanding by Default:** Kern assumes nothing about your target environment. It is a pure bare-metal compiler with zero OS dependencies, which makes it suitable for kernel and firmware work.

Pointers remain pointer-first raw values: `*T` / `*mut T` are plain pointers,
`^T` / `^mut T` are the volatile/MMIO pointer family, and `?T` / `T!E` are
built-in enum forms rather than implicit nullable/reference machinery.

## Compiler Architecture (Workspace)

The `kernc` compiler is built as a highly decoupled, multi-pass Rust workspace.
The current high-level pipeline is:

```text
source
  -> lexer/parser -> AST
  -> semantic analysis
  -> Flow analysis
  -> MAST lowering
  -> MIR construction and optimization
  -> LLVM IR / object emission / linking
```

The main workspace crates are:

* `kernc_lexer` & `kernc_parser`: transform `.rn` source code into an unverified AST.
* `kernc_ast`: defines the frontend syntax nodes and attributes.
* `kernc_db`: provides the small query-driven incremental engine used for staged compiler caching.
* `kernc_sema`: performs type checking, constant evaluation, exhaustiveness checks, and explicit module/path resolution.
* `kernc_driver`: orchestrates staged analysis, incremental reuse, `Flow`, and compile-time reporting used by both CLI and LSP.
* `kernc_flow`: defines shared flow-analysis contracts consumed across compiler layers.
* `kernc_lower` & `kernc_mast`: lower semantically verified programs into the monomorphized lowering IR (MAST), including reachability-driven emission, closure lowering, and vtable/layout materialization.
* `kernc_mir` & `kernc_mir_lower`: lift MAST into Kern MIR, run MIR verification and optimization passes, and produce backend-oriented mid-level IR.
* `kernc_codegen`: translates MIR into LLVM IR, native linker inputs, and ThinLTO artifacts.
* `kernc_cli`: exposes the user-facing `kernc` command.
* `kernc_utils`: handles diagnostics, spans, source/session state, and ICE reporting.

## Official Library Layers

Kern ships four explicit public library layers:

* `base`: runtime-independent foundation types, containers, and allocation building blocks.
* `sys`: operating-system and provider boundaries.
* `rt`: startup and minimal runtime glue.
* `std`: high-level user-facing facilities built on top of `base` and `sys`.

`std` does not mirror `base`, `sys`, or `rt` namespaces. Low-level code should import the owning layer directly.
Hosted support is an OS concern, not a C concern: `std` remains ordinary Kern code layered on `base` plus `sys`, while libc stays an optional external C ABI compatibility choice rather than a foundation for `std`.
Freestanding in Kern is therefore libc-free in the strong sense: `std` can remain fully usable without libc, `sys` owns the hosted OS boundary, and `rt` owns startup glue.
`craft` follows the same pure-first policy by default: runnable targets use `rt`
startup without libc unless a project opts into libc/CRT explicitly.

Before 1.0, Kern does not preserve historical syntax baggage or compatibility shims just because an older spelling once existed. When the language or toolchain is cleaned up, the current form becomes the only supported form across the repository.

## A Taste of Kern (v0.7.0)

Kern elegantly combines low-level hardware control with high-level expression. The following example demonstrates explicit storage mutability, structured inline assembly, exhaustive pattern matching, and the elided initialization syntax.

```kern
// main.rn
use std.io;
use base.Result;

// 1. Traits (Implicit 'self')
type HardwareDevice = trait {
    init: fn() Result[bool, i32],
};

type SerialPort = struct { port: u16 = 0x3F8 };

impl *mut SerialPort {
    pub fn init() Result[bool, i32] {
        // Explicit uninitialized memory
        let mut status = u8.{undef};
        
        // 2. Structured Inline Assembly (Compile-time validated specification)
        @asm(.{
            asm: .{ "in al, dx", },
            outputs: .{ al: status..& }, // Mutable address-of operator
            inputs: .{ dx: self.port },
            volatile: true
        });
        
        // Elided enum initialization
        if (status == 0xFF) return .{ Err: -1 }; 
        return .{ Ok: true };
    }
}

// 3. AST Attributes for Linker/Memory Control
#[link_section(".text.boot")]
#[export_name("_start")]
extern fn start() ! {
    // Mutability is a property of the binding, not the type
    let mut serial = SerialPort.{ port: 0x3F8 };
    
    // 4. Explicit Trait Object Construction
    let device = *mut HardwareDevice.{ serial..& };
    
    // 5. Exhaustive Pattern Matching & Explicit Discards
    let _ = match (device.init()) {
        .{ Ok: _ } => io.print("Serial ready\n\0", .{}),
        .{ Err: code } => @trap(), // Compiler intrinsic for llvm.trap
    };
    
    // Infinite loop
    for (;;) {}
    @unreachable();
}
```

## Current Status

The current repository state targets **`v0.7.0`**.

The shipped toolchain surface is:

* `kernc`: the compiler and linker driver
* `craft`: the package manager and build orchestrator
* `kern-lsp`: the language server
* `base`, `sys`, `rt`, and `std`: the official library layers

The project is still experimental, but the repository documentation now describes the current implementation rather than a rolling roadmap.

## Installation

The easiest way to install Kern is via our official installation scripts. This will automatically download and install the pre-compiled toolchain (`kernc`, `craft`, `kern-lsp`, and the standard library) to your local environment (`~/.kern` on Unix, `%USERPROFILE%\.kern` on Windows).

The installed SDK is meant to be the smallest runtime-complete host toolchain
that lets those shipped tools run. It is not intended to double as a full LLVM
development prefix for building Kern itself from a Git checkout.

Current official host-tool release artifacts are produced for these bounded
baselines:

- Linux `x86_64-linux-gnu`, built on `ubuntu-24.04`
- Windows `x86_64-windows-msvc`, built on `windows-latest` with static CRT
- macOS `x86_64-apple-darwin`, built on `macos-15-intel`
- macOS `aarch64-apple-darwin`, built on `macos-14`

Those labels describe the current official release build baselines. They should
not be read as a blanket promise that one archive runs on every historical
distro, every older macOS release, or every old Windows version.

Windows release packaging has one important explicit rule: official host-tool
artifacts are shipped as static-CRT binaries. This is not a cosmetic choice.
The default Rust/MSVC release path can depend on `VCRUNTIME140*.dll` and the
UCRT redistributable set, which means a "clean" user machine may fail before
`kernc`, `craft`, or `kern-lsp` even start. Official Windows archives are
therefore built with `-C target-feature=+crt-static` so the shipped tools do
not require the VC++ redistributable.

That does **not** mean the Windows tools are freestanding in the bare-metal
sense. They are still normal Win32 user processes and still import Windows
system DLLs such as `KERNEL32.dll`, `ADVAPI32.dll`, `SHELL32.dll`, `ole32.dll`,
and `bcryptprimitives.dll`. Those are OS ABI dependencies for the host tools
themselves, not hidden libc baggage for Kern programs.

Linux and macOS currently have a different host-tool reality: the shipped Unix
archives are not yet fully static. `kernc` and `craft` currently inherit host
runtime dependencies from the Rust + LLVM + C++ toolchain stack, which means a
clean machine can still fail if its shared-library set or libc baseline is too
old. The official installers now verify the installed binaries immediately
after extraction and print targeted remediation hints instead of silently
claiming success.

**For Linux / macOS:**

```bash
curl -sSf https://raw.githubusercontent.com/softfault/kern/main/install.sh | bash
```

**For Windows (PowerShell):**

```powershell
powershell -Command "Set-ExecutionPolicy Bypass -Scope Process -Force; Invoke-Expression (Invoke-WebRequest -Uri https://raw.githubusercontent.com/softfault/kern/main/install.ps1 -UseBasicParsing).Content"
```

On Windows, the first install downloads the bundled LLVM/Clang host toolchain
inside the SDK archive. On slower links, or on machines where Defender scans
large archives aggressively, that can take several minutes. If you would rather
download once and reuse the archive, use the offline archive path instead:

1. Download `install.ps1` and the matching Windows release zip from GitHub Releases.
2. Put them in the same directory, or note the full path to the zip.
3. Open PowerShell in that directory and run:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1 -Archive .\kern-v0.7.0-x86_64-windows-msvc.zip
```

If you renamed the zip, pass the version explicitly:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1 -Version v0.7.0 -Archive .\kern.zip
```

The user-facing installers are native scripts:

- `install.sh` performs Unix installation directly
- `install.ps1` performs Windows installation directly

*(After installation, you may need to restart your terminal or update your PATH variables as prompted by the script).*

If a Unix installer verification step fails, the most common causes are:

- missing shared libraries such as `libstdc++`, `zlib`, or `zstd`
- an older glibc baseline than the release archive was built against

In that situation, install the missing runtime libraries for your distro or
build Kern from source on the target machine.

If you need the detailed host-tool distribution policy rather than the short
installation summary here, see:

- [Windows Distribution Guide](docs/windows-distribution.md)
- [Unix Distribution Guide](docs/unix-distribution.md)

## VS Code Extension

The first-party VS Code extension lives under [`editors/vscode`](./editors/vscode).
It now ships with the Kern logo as the language icon, and the repository also
includes a `Kern Icons` file icon theme for `.rn` files.

If your current VS Code file icon theme does not surface the Kern language icon,
switch the File Icon Theme to `Kern Icons` to force the `.rn` explorer icon to
use the bundled logo.

## Building from Source

For the public step-by-step walkthrough, see
[`website/src/content/guide/building-kern-from-source.md`](website/src/content/guide/building-kern-from-source.md).
The short notes below remain as the repository-level summary.

If you prefer to build the compiler from source, ensure you have the latest stable Rust toolchain and LLVM development libraries installed.

```bash
# Clone the repository
git clone https://github.com/softfault/kern.git
cd kern

# Build the modular workspace
cargo build --release
```

This produces `kernc`, `craft`, and `kern-lsp` in `target/release/`.

On Windows, the command above is fine for local development, but it is **not**
the authoritative release-packaging path. A plain `cargo build --release` on
`x86_64-pc-windows-msvc` may produce host tools that still require the VC++
redistributable at runtime.

For official-style Windows release binaries, build the real Cargo target triple
explicitly and enable static CRT:

```powershell
$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = "-C target-feature=+crt-static"
cargo build --release --target x86_64-pc-windows-msvc -p kernc_cli --bin kernc
cargo build --release --target x86_64-pc-windows-msvc -p craft
cargo build --release --target x86_64-pc-windows-msvc -p kern-lsp
```

The Python packaging entrypoint already applies this policy on Windows:

```powershell
py -3 -m ops release package --version v0.7.0 --target x86_64-windows-msvc
```

One more Windows-specific footgun: the release archive label is
`x86_64-windows-msvc`, but the actual Cargo target triple is
`x86_64-pc-windows-msvc`. The packaging script handles that mapping explicitly.

For Linux and macOS, the current Unix packaging script is intentionally
host-native. The archive label must match the machine actually building the
release, because the script packages from the host's `target/release/` output
rather than pretending to do generic cross-target host-tool packaging.

## Documentation

  * **[Documentation Map](docs/documentation-map.md)**: which documents are authoritative for language semantics, tool behavior, implementation details, and release/distribution policy.
  * **[The `kernc` Compiler Guide](docs/kernc.md)**: CLI usage, driver modes, linking profiles, and build-system integration guidance.
  * **[Windows Distribution Guide](docs/windows-distribution.md)**: Windows host-tool release policy, static CRT packaging, install assumptions, and common packaging footguns.
  * **[Unix Distribution Guide](docs/unix-distribution.md)**: Linux/macOS host-tool release policy, bounded host baselines, installer verification, and Unix packaging footguns.
  * **[Runtime And Library Architecture](docs/runtime-architecture.md)**: the `base`/`sys`/`rt`/`std` split, hosted versus freestanding, and why libc is optional rather than foundational.
  * **[Kern Language Design Document](docs/design.md)**: A comprehensive dive into the language mechanics, memory rules, and syntax for the current version.
  * **[`craft` Package And Build Guide](docs/craft.md)**: the current package, lockfile, dependency-resolution, and build-planning model.
  * **[Source Style Guide](docs/style.md)**: repository-level guidance for writing Kern code clearly and consistently.

## Contributing

Contributions are welcome\! Whether it's reporting a bug, improving documentation, or proposing new features. Please see our [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

**Note on language design:** Kern is a founder-led project. To maintain a cohesive language architecture, major design changes and new syntax proposals will be evaluated strictly against the core philosophy of "High abstraction, low policy." Please open an issue to discuss significant changes before writing code.

## License

Kern is open-source software licensed under the [MIT License](LICENSE).
