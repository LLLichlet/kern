# Kern Language Design (v0.5.0)

## Table of Contents

1.  [Core Philosophy](#1-core-philosophy-and-manifesto)
2.  [Type System](#2-type-system)
3.  [Declarations and Storage](#3-declarations-and-storage)
4.  [Enum Structures](#4-data-structures)
5.  [Functions and Traits](#5-functions-and-traits)
6.  [Control Flow](#6-control-flow)
7.  [Modules](#7-modules)
8.  [Interoperability](#8-interoperability)
9.  [Enum Types (`data`) and Pattern Matching](#9-enum-types-enum-and-pattern-matching)
10. [Stateless Anonymous Functions (Lambdas)](#10-stateless-anonymous-functions-lambdas)
11. [Inline Assembly (`@asm`)](#11-inline-assembly-asm)
12. [AST Attributes and Metadata (`#[...]`)](#12-ast-attributes-and-metadata--and-)
13. [Compiler Intrinsics (`@...`)](#13-compiler-intrinsics-)

-----

## 1\. Core Philosophy and Manifesto

**Kern** is a systems‑level language for operating system kernels, embedded firmware, and high‑performance infrastructure.

Kern’s design is based on the observation that languages trade off **abstraction capability** against **policy constraints**. Kern aims to occupy the fourth quadrant: **high abstraction, low policy**.

### 1.1 Core Values

#### 1\. Clarity over novelty

  * Syntax must be simpler and more consistent than C.
  * Remove features that make generated assembly unpredictable.
  * Fix C legacy warts (spiral declarations, implicit array decay).
  * Goal: what you write is what the machine executes.

#### 2\. Explicit over implicit

  * No implicit heap allocation.
  * No exceptions, no background GC, no implicit destructor chains.
  * Unless explicitly introduced, Kern binaries have no runtime dependencies.

#### 3\. Mechanism Trinity

To achieve “high abstraction, low policy”, Kern provides three core mechanisms:

1.  **Module system** – modern namespaces and visibility control.
2.  **Generics** – strongly‑typed code reuse via monomorphisation (zero runtime cost).
3.  **Enum Types & Pattern Matching** – precise state management without implicit control flow.

### 1.2 Non‑Goals

  * **Compile‑time enforced memory safety** – no borrow checker.
  * **Standard library design** – Kern is freestanding.
  * **Optimisation that exploits undefined behaviour** – ambiguous behaviour (integer overflow, uninitialised reads) is either defined or a compile‑time error.

## 2\. Type System

### 2.1 Primitive Types

  * **Integers**: `i8`, `i16`, `i32`, `i64`, `i128` (signed); `u8`, `u16`, `u32`, `u64`, `u128` (unsigned); `usize`, `isize` (pointer‑sized).
  * **Floats**: `f32`, `f64`.
  * **Boolean**: `bool` (1 byte).
  * **Never**: `!` (diverging computations).

### 2.2 Mutability Model

In Kern, **mutability is a property of storage, not an intrinsic part of the base type.** This means `i32` is the only integer type, but it can be stored in either mutable or immutable memory.

  * **Variable Bindings**: Controlled by the `mut` keyword in the binding pattern.
      * `let x = i32.{10};` (Immutable binding)
      * `let mut y = i32.{20};` (Mutable binding)
  * **Top-Down Bidirectional Flow**: Kern uses contextual typing. Literals like `10` are "type-neutral" and absorb the **Expected Type** flowing down from declarations or function signatures.

### 2.3 Pointers and Volatility

Pointers explicitly carry mutability permissions for the memory they point to.

  * **Normal Pointers**:
      * `*T`: Immutable pointer. Allows reading from `T`.
      * `*mut T`: Mutable pointer. Allows reading and writing to `T`.
  * **Volatile Pointers**: Used for hardware MMIO.
      * `^T`: Immutable volatile pointer.
      * `^mut T`: Mutable volatile pointer.
  * **Address-of Operator (`.&` / `..&`)**:
      * `obj.&`: Obtains an immutable pointer (`*T`).
      * `obj..&`: Obtains a mutable pointer (`*mut T`). This is only valid if `obj` is a mutable location (e.g., declared with `let mut`).
  * **Dereference**: `ptr.*` (postfix).
  * **Pointer Arithmetic**: Kern natively supports pointer arithmetic via the `+` and `-` operators.
      * **Implicit Scaling**: Offsets are implicitly scaled by the element size (`@sizeOf[T]()`).
      * **Strict Types**: When adding or subtracting an offset, the integer operand **must** be of type `usize` or `isize` (implicit promotion from smaller integers is forbidden to prevent bugs).
      * **Pointer Subtraction**: Subtracting two identical pointer types (`ptr1 - ptr2`) yields an `isize` representing the distance in elements between them.
      * **Property Retention**: The resulting pointer strictly inherits all modifiers of the base pointer (e.g., `let next = mmio_ptr + 1;` where `mmio_ptr` is `^mut u32`, guarantees `next` is also `^mut u32`).

### 2.4 Arrays and Slices

  * **Arrays**: `[N]T` – Fixed-size value type.
  * **Slices**: `[]T` or `[]mut T` – A fat pointer containing a pointer and a `usize` length.
  * **String Literals**: `"Hello"` evaluates to `[]u8` (an immutable slice).

## 3\. Declarations and Storage

  * **Local Variables**: `let [mut] name = Expr;`
  * **Global Statics**: `static [mut] name = Expr;`
  * **Constants**: `const NAME = Expr;`
  * **Uninitialized Memory**: `let mut x = Type.{undef};`

## 4\. Enum Structures

### 4.1 Structs

```kern
type Point = struct {
    x: i32,
    y: i32,
};
```

  * **Generics**: `type Point[T] = struct { x: T, y: T };`
  * **Default fields**: `type Config = struct { port: u16 = 8080, host: u32 = 0 };`
  * **Layout**: default reorder/padding for size; `extern type …` guarantees C‑compatible memory layout and alignment.
  * **Initialization and `undef`**: When initializing a struct using `Type.{ ... }`, any field without a default value **must** be explicitly provided; omitting it is a strict compile-time error. If you intentionally want to leave a field uninitialized, you must explicitly use `undef` (e.g., `priority = u8.{undef};`).

<!-- end list -->

```kern
// Immutable 
let p1 = Point.{x: 10, y: 20};       

// Mutable binding (Type provided on the right, mutability on the left)
let mut p2 = Point.{x: 10, y: 20}; 
```

### 4.2 Unions

```kern
type Payload = union {
    as_int: i32,
    as_float: f32,
    raw: [4]mut u8,
};
```

No active‑field tracking; no default values.

### 4.3 Simple Enum (formerly Enums)

In Kern v0.5.0, C-style integer constant sets and complex Algebraic Enum Types are unified under the `data` keyword. For simple sets, the backing type can be explicitly defined (defaults to `u32`).

```kern
type Color: u8 = data {
    Red = 0,
    Green, // 1
    Blue,  // 2
};
```

  * **Enum Sanitization**: Kern **forbids** using the `as` operator or intrinsics to implicitly cast untrusted dynamic integers (e.g., hardware port reads) into Enum variants. Valid variants are constructed directly (`Color.Red`). Sanitizing external data must be explicitly handled by the programmer via an exhaustive `match` block:

<!-- end list -->

```kern
let raw_data = inb(0x60); // Read u8 from port
let color = match (raw_data) {
    0 => Color.Red,
    1 => Color.Green,
    2 => Color.Blue,
    _ => Color.Red, // Mandatory fallback for unexpected values
};
```

### 4.4 Conversions

In Kern v0.5.0, type conversions are explicitly and uniformly handled by the `as` operator.

  * **Numeric Conversions**: `as` is used for all safe and unsafe numeric conversions, including bit-width truncation, zero/sign-extension, and integer/floating-point conversions (e.g., `i32 as u8`, `f32 as i32`).
  * **Pointer Reinterpretation**: `as` preserves the physical bit pattern when casting between pointer types or between pointers and `usize`/`isize`.
  * **Strict Boundaries**: The `as` operator **cannot** be used to implicitly construct Trait Objects, nor can it cast arbitrary integers directly into `data` variants. Fat pointer construction requires Explicit Constructor Syntax (`Trait.{ ptr }`).

## 5\. Functions and Traits

### 5.2 Implementation Blocks (`impl`)

`impl` blocks attach methods to a concrete type (including pointer types). The `self` parameter is implicitly injected and managed by the Semantic Analyzer.

```kern
type Point = struct { x: i32, y: i32 };

impl *mut Point {
    // 'self' is implicitly available as *mut Point
    pub fn move_by(dx: i32, dy: i32) void {
        self.x += dx; 
        self.y += dy;
    }
}
```

### 5.4 Traits

Traits define a VTable contract. Methods implicitly receive a `self` reference.

```kern
type Writer = trait {
    write: fn([]u8) usize,
};
```

### 5.5 Trait Objects (Fat Pointers)

A Trait Object is a runtime-dynamic fat pointer consisting of a data pointer and a VTable pointer. They are constructed using **Explicit Constructor Syntax**.

  * **Construction**: You assemble a trait object by passing a concrete pointer to the Trait's constructor.
  * **Safety Rule**: To prevent stack-size ambiguity, a Trait Object can only be constructed from a pointer type.

<!-- end list -->

```kern
let mut file = File.{ ... };
// Assemble a mutable Trait Object from a mutable pointer
let w = *mut Writer.{ file..& }; 
w.write("Kern\0");
```

## 6\. Control Flow

### 6.1 Conditional Expressions

`if` is an expression.

```kern
let a = if (b < 10) i32.{10} else i32.{20};
```

### 6.2 Match Expressions

Enhanced pattern matching and branching. In Kern v0.5.0, `match` completely replaces `switch` for all branching logic (integers, strings, and `data` variants). No fallthrough.

  * **Ranges**: `..` defines a left-closed, right-open range. `..=` defines a fully inclusive range.

<!-- end list -->

```kern
let result = match (val) {
    1..10 => 10,       // 1 to 9
    11, 12, 13 => 20,
    14..=15 => 30,     // 14 and 15
    _ => 0,
};
```

  * **Exhaustiveness**: Match expressions must be exhaustive. When matching on a `data` type, `else =>` is not required if all variants are explicitly matched.

### 6.3 For Loops

Only `for` (no `while`, `do‑while`).

```kern
for (let i = 0; i < 10; i += 1) { … }
for (; cond ;) { … }          // while
for (;;) { … }                // infinite loop
```

### 6.4 Defer

Executes an expression or block when the **current lexical scope (block `{\}`)** exits (LIFO). `defer` is strictly block‑scoped, not function‑scoped.

```kern
let ptr = malloc(1024);
defer free(ptr);
```

### 6.5 Blocks, Expressions, and Discards

Blocks evaluate to their last expression.
Kern strictly mandates that returned values cannot be implicitly ignored to prevent logical errors in systems programming.

  * **Explicit Discard**: If a function or expression returns a value that is intentionally unused, it **must** be bound to the discard identifier `_`.
    `let _ = file.write(buf); // Explicitly discard the returned usize`
  * `expr;` evaluates to `void`. Dropping a non-void return value by simply appending a semicolon is a compiler error.

**Evaluation Order with Defer:**
When a block `{ … }` evaluates as an expression and contains `defer` statements, the exact exit sequence is:

1.  **Evaluate**: Compute the value of the final expression.
2.  **Execute**: Run all `defer` statements registered in the current block in LIFO order.
3.  **Yield**: Pass the computed value to the outer context.

> **Warning**: Returning a pointer to a resource that is freed by a `defer` within the exact same block will result in a dangling pointer. Kern prioritizes explicit execution order over implicit memory protection.

## 7\. Modules

Kern's module system is designed to be explicit, highly predictable, and strictly controlled by the programmer. In v0.4.0, Kern transitioned to an explicit module tree declaration model to support robust visibility control, re-exports, and conditional compilation.

### 7.1 Explicit Module Tree (`mod`)

Files and directories do not implicitly become part of the compilation unit just by existing on the filesystem. A module must be explicitly declared using the `mod` keyword.

  * **File Modules**: `mod utils;` instructs the compiler to look for `utils.kr`.
  * **Directory Modules**: If `utils` is a directory, the compiler looks for `utils/init.kr`.
  * **Visibility**: By default, modules are private. Use `pub mod utils;` to expose the module and its public contents to outer scopes.

<!-- end list -->

```kern
// Explicitly build the module tree
mod memory;
pub mod process;

// Conditional module compilation (e.g., in std/os/init.kr)
#[if(os == "linux")]
mod linux;

#[if(os == "windows")]
mod windows;
```

### 7.2 Imports and Path Resolution (`use`)

Absolute paths in Kern are resolved through two precise roots:

1.  **Compiler Root Directory**: The root module entry point provided to `kernc` (e.g., treating the project root similar to `crate::`).
2.  **CLI Alias Mappings**: External package paths explicitly mapped via compiler options (e.g., `-M std=./libs/std` allows `use std.io;`).

Paths are navigated strictly:

  * **Absolute import**: `use std.io.File;`
  * **Relative import (Current)**: `use .utils;` (Starts from the current module)
  * **Relative import (Parent)**: `use ..common.types;` (Starts from the parent module)
  * **Grouped imports**: `use std.os.{Handle, write, exit};`

### 7.3 Facade Pattern and Re-exports (`pub use`)

Kern supports the Facade pattern via `pub use`. This allows you to construct a clean, unified public API while keeping the internal module layout complex and conditionally compiled.

```kern
// std/os/init.kr
#[if(os == "linux")]
mod linux;

// Re-export symbols from the private `linux` module to the public `std.os` API
#[if(os == "linux")]
pub use .linux.{Handle, get_stdout_handle, write, exit};
```

### 7.4 Multi-Pass Resolution

Kern utilizes a multi-pass Semantic Analyzer. Circular type dependencies across different module files (e.g., Module A uses a struct from Module B, which contains a pointer to a struct from Module A) are fully supported natively. There is no need for C-style forward declarations or header files.

## 8\. Interoperability

Kern uses the C Application Binary Interface (ABI) as the universal language for all external communication.

### 8.1 Exporting Functions to C/Assembly

Use the `extern` modifier on the function definition. This instructs the compiler to use the standard C calling convention and disables name mangling. (Note: `pub` is a frontend semantic modifier for Kern modules; `extern` alone handles external linkage).

```kern
extern fn _start() void { ... }
```

### 8.2 Importing External Functions and Statics

External C functions can use the `...` syntax to support C-style variadic arguments. External statics must be declared using `T.{undef}`. Items inside an `extern` block can be marked `pub` to expose them through the Kern module system.

```kern
extern {
    pub fn malloc(size: usize) *mut u8;
    pub fn printf(format: *u8, ...) i32;
    pub static MULTIBOOT_MAGIC = u32.{undef};
}
```

## 9\. Enum Types (`enum`) and Pattern Matching

Kern v0.5.0 unifies all tagged unions and enumerations under the `enum` keyword, paired exclusively with `match` for branching.

### 9.1 Defining Enum Types

Use the `enum` keyword to define tagged unions with payloads (Algebraic Enum Types).

```kern
pub type Option[T] = enum {
    Some: T,
    None,
};
```

### 9.2 Elided Initialization Syntax

Where the target type context is strictly explicit (e.g., function returns, arguments, explicit variable declarations), **any type** (including Enum, Arrays, and Structs) can be initialized using the elided literal syntax `.{ ... }`.

```kern
fn safe_divide(a: i32, b: i32) Result[i32, i32] {
    if (b == 0) return .{ Err: -1 }; 
    return .{ Ok: a / b };
}
```

### 9.3 Pattern Matching (`match`)

Pattern matching is the only way to access the payload of a `enum` variant. Bindings within a match arm can be made mutable.

```kern
match (opt) {
    .Some: mut val => { 
        val += 1; 
        printf("%d\n\0", val);
    },
    .None => printf("Nothing\n\0"),
}
```

## 10\. Stateless Anonymous Functions (Lambdas)

To support inline callbacks without violating Kern's strict memory rules, the language supports stateless anonymous functions.

### 10.1 Strict Statelessness

Anonymous functions use the `fn(...) ReturnType { ... }` syntax.
Crucially, Kern **strictly forbids environmental capturing (closures)**. An anonymous function cannot access local variables from its enclosing scope. This physical limitation guarantees that anonymous functions compile down to pure, static function pointers (`fn`), entirely preventing use-after-free bugs caused by stack-allocated environments escaping their scope.

```kern
let arr = [3]mut i32.{ 3, 1, 2 };

// Safe, zero-allocation callback
arr.sort(fn(a: i32, b: i32) bool {
    return a < b;
});
```

## 11\. Inline Assembly (`@asm`)

To maintain Kern's philosophy of "explicit over implicit", inline assembly does not use format strings with hidden index bindings. Instead, it leverages Kern's elided struct literal syntax (`.{ ... }`) to create a strict, named mapping between CPU registers and Kern variables.

### 11.1 Syntax, Register Binding, and MAST Evaluation

The parameters passed to `@asm` (such as the `asm` string array, `clobbers`, and `volatile` flag) are **not runtime structures**. They are resolved and evaluated entirely at compile-time during the MAST (Monomorphized Abstract Syntax Tree) phase.

```kern
pub fn outb_and_read(port: u16, data: u8) u8 {
    let mut status = u8.{undef};

    @asm(.{
        asm: .{
            "out dx, al",
            "in al, dx"
        },
        outputs: .{ al: status..& },   // Binds register to mutable pointer
        inputs: .{ dx: port, al: data },
        clobbers: .{ "memory" },      // Compile-time known
        volatile: true                // Compile-time known
    });

    return status;
}
```

## 12\. AST Attributes and Metadata (`#[...]` and `#![...]`)

Kern completely rejects traditional C-style preprocessor macros, substituting them with an **Attribute Mini-Language**. Attributes are strictly parsed by the frontend and natively understood by the compiler backend to control memory layout, linkage, and optimization.

### 12.1 Scope: Outer vs. Inner Attributes

  * **Outer Attributes (`#[...]`)**: Attached to the immediately following AST node (e.g., a function, struct, or variable declaration).
  * **Inner Attributes (`#![...]`)**: Applies to the entire enclosing lexical scope (usually the file). If placed at the top of an `init.kr` file, the attribute applies to the entire module.

### 12.2 Mutually Exclusive Content

Kern strictly enforces single-responsibility for attribute brackets. The content inside the brackets `[...]` must be **either** a condition evaluator **or** a list of metadata tags.

#### 1\. Conditional Compilation (`if(...)`)

Uses a strict boolean evaluator at compile-time. If the condition evaluates to `false`, the target node (or file) is entirely pruned before semantic analysis. It supports logical operators (`and`, `or`, `not`) and checking custom compiler flags (`-D key=value`).

```kern
#![if(os == "bare_metal")]
#[if(not debug_mode)]
```

#### 2\. Metadata Tags

A comma-separated list of tags attached to the AST for compiler side-effects. Metadata tags are grouped by their specific impact on the generated binary:

**A. Linkage & FFI Control**

  * `export_name("...")`: Overrides the mangled name with a specific string for the linker.
  * `link_section("...")`: Forces a global variable or function into a specific ELF/Mach-O/COFF section (crucial for OS bootloaders, e.g., `#[link_section(".multiboot")]`).

**B. Memory Layout**

  * `packed`: Removes all padding between struct/union fields. The size becomes exactly the sum of its fields, at the cost of potential unaligned memory access penalties.
  * `align(N)`: Forces the alignment of a struct or static variable to `N` bytes (e.g., `#[align(4096)]` for page tables).

**C. Optimization & Control Flow**

  * `cold`: Marks a function as rarely executed, moving it out of the hot instruction cache and optimizing branching.
  * `naked`: Instructs the compiler to omit the standard function prologue and epilogue. Strictly used for hardware interrupt handlers and contextual context-switching alongside `@asm`.
  * `inline(always)` / `inline(never)`: *(Planned)* Overrides the LLVM inliner's heuristic.

-----

## 13\. Compiler Intrinsics (`@...`)

Intrinsics are special functions implemented directly within the Kern compiler backend (e.g., LLVM). They are prefixed with `@` to strictly separate them from user-defined functions. They are used for operations that alter data representation, query compile-time information, or emit specialized CPU instructions.

### 13.1 Compile-Time Type Information

These intrinsics are evaluated completely at compile-time and result in a constant `usize`.

  * `@sizeOf[T]() -> usize`: Returns the memory footprint (size in bytes) of type `T`.
  * `@alignOf[T]() -> usize`: Returns the ABI-required alignment (in bytes) of type `T`.

### 13.2 Hardware & Execution Control

  * `@unreachable() -> !`: Emits an unreachable instruction. Informs the optimizer that a control flow path is physically impossible, allowing it to eliminate dead branches (often used in exhaustiveness fallback).
  * `@trap() -> !`: Emits an illegal instruction (`llvm.trap`) to deliberately crash/halt the program securely.
  * `@fence()`: Emits a strictly sequentially-consistent memory fence (`mfence`) to prevent instruction reordering around sensitive MMIO operations.
  * `@breakpoint()`: Triggers a hardware breakpoint (`llvm.debugtrap`) for system debuggers.

*(Note: Kern does not provide `@volatileLoad` or `@volatileStore` intrinsics. Instead, Kern treats volatility as a first-class type qualifier (`^T` and `^mut T`). Hardware register accesses are performed via standard dereferencing `ptr.*` on a volatile pointer, yielding perfectly predictable code without intrinsic clutter.)*

### 13.3 Bitwise Math & Memory Operations

Mapped directly to single-cycle CPU instructions and highly optimized backend primitives where available:

  * `@popCount[T: Integer](val: T) -> T`: Returns the number of set bits (1s).
  * `@clz[T: Integer](val: T) -> T`: Count leading zeros.
  * `@ctz[T: Integer](val: T) -> T`: Count trailing zeros.
  * `@bswap[T: Integer](val: T) -> T`: Reverses the byte order of an integer value (useful for endianness conversions).
  * `@memcpy(dest: *mut u8, src: *u8, len: usize) void`: Performs a highly-optimized bulk memory copy.
  * `@memset(dest: *mut u8, val: u8, len: usize) void`: Performs a highly-optimized bulk memory fill.
