# Kern Programming Language

![Kern logo](./logo.svg)

> **Status:** v0.7.0 (Experimental)
> *High Abstraction, Low Policy.*

Kern is a systems-level programming language explicitly designed for operating system kernels, embedded firmware, and high-performance infrastructure.

Kern is built on the observation that languages often trade off abstraction capabilities against policy constraints. Kern aims to occupy the "fourth quadrant": providing powerful, modern abstractions (like explicit module systems, generics, and algebraic data types) while enforcing virtually **zero hidden runtime policies**. There are no implicit allocations, no garbage collection, no exceptions, and no closures with hidden captures.

## Core Philosophy

* **Clarity over Novelty:** What you write is what the machine executes. Kern fixes C's legacy warts while maintaining entirely predictable assembly generation.
* **Explicit over Implicit:** Mutability is a property of storage, not the type itself (`let mut x`). Pointer math is strictly typed. All type conversions require the explicit `as` operator. Return values cannot be silently ignored.
* **Zero-Cost Abstractions:** Features like monomorphized Generics, Algebraic Data Types (`enum`), and strictly Stateless Lambdas compile down to highly optimized, flat LLVM IR with zero runtime overhead.
* **Mechanism Trinity:** Kern relies on three core mechanisms to maintain its philosophy: a strictly explicit module tree (`mod`), strongly-typed zero-cost generics, and precise state management via exhaustive `match` blocks.
* **Freestanding by Default:** Kern assumes nothing about your target environment. It is a pure bare-metal compiler with zero OS dependencies, which makes it suitable for kernel and firmware work.

## Compiler Architecture (Workspace)

The `kernc` compiler is built as a highly decoupled, multi-pass Rust workspace. This clean pipeline guarantees maintainability and clear semantic boundaries:

* `kernc_lexer` & `kernc_parser`: Transforms `.rn` source code into an unverified Abstract Syntax Tree.
* `kernc_ast`: Defines the frontend syntax nodes and attributes.
* `kernc_sema`: The heart of the compiler. Performs strict top-down bidirectional type checking, robust 3-phase constant evaluation (ConstEval), exhaustive pattern matching verification, and explicit module path resolution.
* `kernc_lower` & `kernc_mast`: Lowers the semantically verified AST into the Monomorphized Abstract Syntax Tree (MAST), resolving all generics, applying let-binding hoisting to prevent side-effects, and laying out vtables.
* `kernc_codegen`: Translates MAST into highly optimized LLVM IR.
* `kernc_driver` & `kernc_cli`: Manages the compilation session, external linkage, and user-facing CLI.
* `kernc_utils`: Handles rigorous internal diagnostics, span tracking, and ICE (Internal Compiler Error) reporting.

## Official Library Layers

Kern ships four explicit public library layers:

* `base`: runtime-independent foundation types, containers, and allocation building blocks.
* `sys`: operating-system and provider boundaries.
* `rt`: startup and minimal runtime glue.
* `std`: high-level user-facing facilities built on top of `base` and `sys`.

`std` no longer mirrors `base`, `sys`, or `rt` namespaces. Low-level code should import the owning layer directly.
Hosted support is an OS concern, not a C concern: `std` remains ordinary Kern code layered on `base` plus `sys`, while libc stays an optional external runtime/provider choice rather than a foundation for `std`.
Freestanding in Kern is therefore libc-free in the strong sense: `std` can remain fully usable without libc, `sys` owns the hosted OS boundary, and `rt` owns startup glue.

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
        
        // 2. Structured Inline Assembly (Compile-time MAST evaluation)
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

Official release artifacts are published for Linux `x86_64`, Windows `x86_64`, macOS `x86_64`, and macOS `aarch64`.

**For Linux / macOS:**

```bash
curl -sSf https://raw.githubusercontent.com/softfault/kern/main/install.sh | bash
```

**For Windows (PowerShell):**

```powershell
powershell -Command "Set-ExecutionPolicy Bypass -Scope Process -Force; Invoke-Expression (Invoke-WebRequest -Uri https://raw.githubusercontent.com/softfault/kern/main/install.ps1 -UseBasicParsing).Content"
```

*(After installation, you may need to restart your terminal or update your PATH variables as prompted by the script).*

## VS Code Extension

The first-party VS Code extension lives under [`editors/vscode`](./editors/vscode).
It now ships with the Kern logo as the language icon, and the repository also
includes a `Kern Icons` file icon theme for `.rn` files.

If your current VS Code file icon theme does not surface the Kern language icon,
switch the File Icon Theme to `Kern Icons` to force the `.rn` explorer icon to
use the bundled logo.

## Building from Source

If you prefer to build the compiler from source, ensure you have the latest stable Rust toolchain and LLVM development libraries installed.

```bash
# Clone the repository
git clone https://github.com/softfault/kern.git
cd kern

# Build the modular workspace
cargo build --release
```

This produces `kernc`, `craft`, and `kern-lsp` in `target/release/`.

## Documentation

  * **[The `kernc` Compiler Guide](docs/kernc.md)**: CLI usage, driver modes, linking profiles, and build-system integration guidance.
  * **[Runtime And Library Architecture](docs/runtime-architecture.md)**: the `base`/`sys`/`rt`/`std` split, hosted versus freestanding, and why libc is optional rather than foundational.
  * **[Kern Language Design Document](docs/design.md)**: A comprehensive dive into the language mechanics, memory rules, and syntax for the current version.
  * **[`craft` Package And Build Guide](docs/craft.md)**: the current package, lockfile, dependency-resolution, and build-planning model.
  * **[Source Style Guide](docs/style.md)**: repository-level guidance for writing Kern code clearly and consistently.

## Contributing

Contributions are welcome\! Whether it's reporting a bug, improving documentation, or proposing new features. Please see our [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

**Note on language design:** Kern is a founder-led project. To maintain a cohesive language architecture, major design changes and new syntax proposals will be evaluated strictly against the core philosophy of "High abstraction, low policy." Please open an issue to discuss significant changes before writing code.

## License

Kern is open-source software licensed under the [MIT License](LICENSE).
