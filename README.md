# Kern Programming Language

> **Status:** v0.1.0 (Experimental) 
> 
> *High Abstraction, Low Policy.*

Kern is a systems-level programming language explicitly designed for operating system kernels, embedded firmware, and high-performance infrastructure. 

Kern is built on the observation that languages often trade off abstraction capabilities against policy constraints. Kern aims to occupy the "fourth quadrant": providing powerful, modern abstractions (like generics and module systems) while enforcing virtually zero hidden runtime policies (no implicit allocations, no GC, no implicit exceptions).

## Core Philosophy

* **Clarity over Novelty:** What you write is what the machine executes. Kern fixes C's legacy warts (like spiral declarations and implicit array decays) while maintaining predictable assembly generation.
* **Explicit over Implicit:** Mutability must be explicitly declared (`mut T`). Pointer mathematics requires explicit casting. No implicit background magic.
* **Seamless Interoperability:** The C ABI is the universal language. Kern communicates natively with C and Assembly with zero overhead.

## Quick Look

```kern
use std.io;

type Reader = trait {
    read: fn([]u8) usize,
};

type File = struct { 
    fd: i32 
};

impl *mut File : Reader {
    pub fn read(buf: []u8) usize {
        // Implementation here...
        0
    }
}

pub extern fn _start() void {
    let file = mut File.{ fd: 1 };
    let reader = file.& as mut Reader; // Safe trait object cast
}

```

## Roadmap & Current Status

The compiler is currently in its initial `v0.1.x` series. It successfully parses the core syntax, performs semantic analysis/typechecking, and generates LLVM IR.

* **v0.1.x (Core Stabilization - Current):** Focus on hardening the compiler pipeline, expanding the test suite, improving diagnostic/error messages, and introducing basic built-in functions.
* **v0.2.x (Advanced Types):** Introduction of Algebraic Data Types (ADT), pattern matching (`match`), and stateless anonymous functions (lambdas).
* *Note on Classes: The `class` keyword (instance-level polymorphism) is currently prototyped in the design spec. However, its final inclusion is deferred. I will evaluate its absolute necessity by writing real-world Kern projects (e.g., standard library, HTTP parsers) before committing to it.*


* **v0.3.x (Meta-programming):** Introduction of AST attribute markers (e.g., `#[]`) to support robust conditional compilation.
* **v0.4.x (Low-level Control):** Support for Inline Assembly (ASM) and the initial bootstrapping of the Kern Standard Library.
* **v0.5.x (Standard Library Maturation):** Full development and stabilization of the built-in standard library.

## Building the Compiler

The Kern compiler is written in Rust. To build it from source, ensure you have the latest stable Rust toolchain installed.

```bash
# Clone the repository
git clone https://github.com/softfault/kern.git
cd kern

# Build the compiler
cargo build --release

```

## Documentation

For a comprehensive dive into the language mechanics, type system, and memory rules, please read the [Kern Language Design Document](design.md).