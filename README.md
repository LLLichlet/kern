# Kern Programming Language

> **Status:** v0.5.2 (Experimental)  
> *High Abstraction, Low Policy.*

Kern is a systems-level programming language explicitly designed for operating system kernels, embedded firmware, and high-performance infrastructure.

Kern is built on the observation that languages often trade off abstraction capabilities against policy constraints. Kern aims to occupy the "fourth quadrant": providing powerful, modern abstractions (like explicit module systems, generics, and algebraic data types) while enforcing virtually **zero hidden runtime policies**. There are no implicit allocations, no garbage collection, no exceptions, and no closures with hidden captures.

## Core Philosophy

  * **Clarity over Novelty:** What you write is what the machine executes. Kern fixes C's legacy warts while maintaining entirely predictable assembly generation.
  * **Explicit over Implicit:** Mutability is a property of storage, not the type itself (`let mut x`). Pointer math is strictly typed. All type conversions require the explicit `as` operator. Return values cannot be silently ignored.
  * **Zero-Cost Abstractions:** Features like monomorphized Generics, Algebraic Data Types (`enum`), and strictly Stateless Lambdas compile down to highly optimized, flat LLVM IR with zero runtime overhead.
  * **Mechanism Trinity:** Kern relies on three core mechanisms to maintain its philosophy: a strictly explicit module tree (`mod`), strongly-typed zero-cost generics, and precise state management via exhaustive `match` blocks.
  * **Freestanding by Default:** Kern assumes nothing about your target environment. It is a pure bare-metal compiler with zero OS dependencies—ideal for kernel development.

## Compiler Architecture (Workspace)

As of v0.5.0, the `kernc` compiler has been entirely rewritten into a highly decoupled, multi-pass Rust workspace. This clean pipeline guarantees maintainability and clear semantic boundaries:

  * `kernc_lexer` & `kernc_parser`: Transforms `.kr` source code into an unverified Abstract Syntax Tree.
  * `kernc_ast`: Defines the frontend syntax nodes and attributes.
  * `kernc_sema`: The heart of the compiler. Performs strict top-down bidirectional type checking, constant evaluation, exhaustive pattern matching verification, and explicit module path resolution.
  * `kernc_lower` & `kernc_mast`: Lowers the semantically verified AST into the Monomorphized Abstract Syntax Tree (MAST), resolving all generics and laying out vtables.
  * `kernc_codegen`: Translates MAST into highly optimized LLVM IR.
  * `kernc_driver` & `kernc_cli`: Manages the compilation session, external linkage, and user-facing CLI.

## A Taste of Kern (v0.5.0)

Kern elegantly combines low-level hardware control with high-level expression. The following example demonstrates explicit storage mutability, structured inline assembly, exhaustive pattern matching, and the updated elided initialization syntax.

```kern
// main.kr
use std.{io, Result};

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
        .Ok: _ => io.print("Serial ready\n\0", .{}),
        .Err: code => @trap(), // Compiler intrinsic for llvm.trap
    };
    
    // Infinite loop
    for (;;) {}
    @unreachable();
}
```

## Roadmap & Current Status

The compiler is currently in its **`v0.5.x`** series, marking a massive architectural shift to a modular workspace and refined language semantics.

  * **[Delivered] v0.1.x - v0.3.x:** Core stabilization, basic generics, initial LLVM backend, inline assembly (`@asm`), and basic AST attributes.
  * **[Delivered] v0.4.x (Standard Library):** Implementation of the explicit module tree system (`mod`), cross-platform freestanding standard library (`std`), and multi-pass type resolution.
  * **[Delivered] v0.5.x (Workspace & Type System Overhaul):** Complete decoupled compiler workspace (`kernc_*`), `.kr` file extension, unified `enum`/`data` types, exhaustive `match` branching replacing `switch`, and strict explicit type conversions via `as`.
  * **[Current Focus] v0.6.x (Tooling & Ecosystem):** Package management foundations, enhanced diagnostic reporting with span tracking, and Language Server Protocol (LSP) foundations.

## Installation

The easiest way to install Kern is via our official installation script. This script automatically downloads and installs the pre-compiled compiler and the standard library toolchain to `~/.kern`.

**For Linux / macOS:**

```bash
curl -sSf https://raw.githubusercontent.com/softfault/kern/main/install.sh | bash
```

*(After installation, you may need to restart your terminal or run `source ~/.bashrc` to update your PATH).*

## Building from Source

If you prefer to build the compiler from source, ensure you have the latest stable Rust toolchain and LLVM development libraries installed.

```bash
# Clone the repository
git clone https://github.com/softfault/kern.git
cd kern

# Build the modular workspace
cargo build --release
```

## Documentation

  * **[Kern Language Design Document](design.md)**: A comprehensive dive into the language mechanics, memory rules, and syntax for v0.5.0.
  * *(Coming Soon)* **The Kern Type System**: A guide to understanding Kern's Contextual Top-Down Bidirectional Type Checking model.

## Contributing

Contributions are welcome\! Whether it's reporting a bug, improving documentation, or proposing new features. Please see our [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

**Note on language design:** Kern is a founder-led project. To maintain a cohesive language architecture, major design changes and new syntax proposals will be evaluated strictly against the core philosophy of "High abstraction, low policy." Please open an issue to discuss significant changes before writing code.

## License

Kern is open-source software licensed under the [MIT License](LICENSE).
