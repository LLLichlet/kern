# Kern Language Design (v0.7.6)

## Table of Contents

1.  [Core Philosophy](#1-core-philosophy-and-manifesto)
2.  [Type System](#2-type-system)
3.  [Declarations and Storage](#3-declarations-and-storage)
4.  [Const and Compile-Time Evaluation](#4-const-and-compile-time-evaluation)
5.  [Enum Structures](#5-enum-structures)
6.  [Functions and Traits](#6-functions-and-traits)
7.  [Control Flow](#7-control-flow)
8.  [Modules](#8-modules)
9.  [Interoperability](#9-interoperability)
10. [Enum Types (`enum`) and Pattern Matching](#10-enum-types-enum-and-pattern-matching)
11. [Closures and Anonymous Functions](#11-closures-and-anonymous-functions)
12. [Inline Assembly (`@asm`)](#12-inline-assembly-asm)
13. [AST Attributes and Metadata (`#[...]`)](#13-ast-attributes-and-metadata--and-)
14. [Compiler Intrinsics (`@...`)](#14-compiler-intrinsics-)

-----

## 1\. Core Philosophy and Manifesto

**Kern** is a systems level language for operating system kernels, embedded firmware, and high performance infrastructure.

Kern design is based on the observation that languages trade off **abstraction capability** against **policy constraints**. Kern aims to occupy the fourth quadrant: **high abstraction, low policy**.

### 1.1 Core Values

#### 1\. Clarity over novelty

  * Syntax must be simpler and more consistent than C.
  * Remove features that make generated assembly unpredictable.
  * Remove C declaration hazards such as spiral declarations and implicit array decay.
  * Goal: what you write is what the machine executes.

#### 2\. Explicit over implicit

  * No implicit heap allocation.
  * No exceptions, no background GC, no implicit destructor chains.
  * Unless explicitly introduced, Kern binaries have no runtime dependencies.

#### 3\. Mechanism Trinity

To achieve "high abstraction, low policy", Kern provides three core mechanisms:

1.  **Module system** - modern namespaces and visibility control.
2.  **Generics** - strongly-typed code reuse via monomorphisation (zero runtime cost).
3.  **Enum Types & Pattern Matching** - precise state management without implicit control flow.

### 1.2 Non goals

  * **Compile-time enforced memory safety** - no borrow checker.
  * **Standard library design** - Kern is freestanding.
  * **Optimisation that exploits undefined behaviour** - ambiguous behaviour (integer overflow, uninitialised reads) is either defined or a compile-time error.

## 2\. Type System

### 2.1 Primitive Types

  * **Integers**: `i8`, `i16`, `i32`, `i64`, `i128` (signed); `u8`, `u16`, `u32`, `u64`, `u128` (unsigned); `usize`, `isize` (pointer鈥憇ized).
  * **Floats**: `f32`, `f64`.
  * **Boolean**: `bool` (1 byte).
  * **SIMD primitives**: Kern also provides builtin SIMD types written directly as names such as `f32x4`, `i32x4`, `u8x16`, and `boolx4`.
  * **Never**: `!` (diverging computations).
  * **Void**: `void` - A zero-sized type (ZST). It represents the absence of a meaningful value. Used primarily as the default return type for functions that produce no data, or to construct untyped raw pointers (`&mut void` / `&void`) for FFI and memory allocation.

SIMD is part of the language, not a library abstraction and not an alias for arrays or slices. A type like `f32x4` is a first-class builtin type in its own right.

The fixed-width SIMD family uses these source forms:

  * Signed integer vectors: `i8xN`, `i16xN`, `i32xN`, `i64xN`, `i128xN`, `isizexN`
  * Unsigned integer vectors: `u8xN`, `u16xN`, `u32xN`, `u64xN`, `u128xN`, `usizexN`
  * Floating vectors: `f32xN`, `f64xN`
  * Mask vectors: `boolxN`

`N` is part of the type spelling and must be a positive lane count.

`boolxN` is the SIMD mask family. It is not interchangeable with scalar `bool`, and it is not an array of booleans.

### 2.2 Mutability Model

In Kern, **mutability is a property of storage, not an intrinsic part of the base type.** This means `i32` is the only integer type, but it can be stored in either mutable or immutable memory.

  * **Variable Bindings**: Controlled by the `mut` keyword in the binding pattern.
      * `let x = 10i32;` (Immutable binding)
      * `let mut y = 20i32;` (Mutable binding)
  * **No Automatic Upgrade Across Handles**: `let mut` does **not** silently upgrade every derived handle into a writable one. Rebinding a value, mutating an aggregate in place, mutating through a pointer, and producing a mutable slice are distinct questions.
      * `let mut arr = [4]u8.{ 1, 2, 3, 4 };` allows both `arr = [4]u8.{ 5, 6, 7, 8 };` and `arr.[0] = 9;` because the binding owns mutable storage for the whole array aggregate.
      * `let arr = [4]u8.{ 1, 2, 3, 4 };` rejects `arr.[0] = 9;` because the access path reaches immutable storage.
      * `let mut p = &(... as u8);` may rebind the pointer value itself, but `p.* = 9;` is still rejected. Write access through a pointer comes from `&mut T`, not from `let mut p`.
      * `let mut view = &[u8].{ ... };` may rebind the slice value itself, but it does not become `&mut [u8]`. Mutable slice permissions remain part of the slice type because a slice is a view, not a physical aggregate.
  * **Top-Down Bidirectional Flow**: Kern uses contextual typing. Literals like `10` are "type-neutral" and absorb the **Expected Type** flowing down from declarations or function signatures.

### 2.3 Pointers, Optionals, and Volatility

Pointers explicitly carry mutability permissions and pointer-family semantics.

In Kern, a pointer is a first-class plain value. It can be stored, passed,
returned, compared, used as the target of an `impl` block, and converted to or
from integer address values with explicit casts. Kern does not model pointers
as a hidden borrow/reference system.

Kern currently uses two pointer families:

  * **Object Pointers**:
      * `&T`: immutable raw pointer
      * `&mut T`: mutable raw pointer
      * these are ordinary pointer values for object access and general memory work
      * they may be cast to and from `usize` / `isize` explicitly
      * they are not hidden non-null references
  * **Address / Volatile Pointers**:
      * `^T`: immutable address / volatile pointer
      * `^mut T`: mutable address / volatile pointer
      * these are the explicit MMIO / fixed-address family
      * dereferencing them performs volatile memory access
      * they remain ordinary values and do not disable explicit casts or arithmetic

Bare function pointers (`&fn(Args) Ret`) are thin pointer values too. They are
part of Kern's explicit pointer reinterpretation model: `&fn(...) Ret`, `&T`,
`&mut T`, `^T`, `^mut T`, `usize`, and `isize` may be converted between one
another with `as`, preserving the raw bit pattern. Dynamic closure pointers
such as `&Fn(...) Ret` remain fat pointers and are not part of this thin
pointer cast family.

`?T` and `T!E` are builtin enum families, not pointer modifiers. This means:

  * `?&T` is simply `?` applied to `&T`
  * `?^T` is simply `?` applied to `^T`
  * builtin carriers do not receive hidden nullable-pointer compression or privileged ABI treatment
  * if an ordinary user-defined enum has the same shape, it has the same semantic standing

The cast boundary is intentionally explicit:

  * `usize as &T`, `usize as &mut T`, `usize as ^T`, and `usize as ^mut T` enter pointer space directly
  * pointer-to-integer exits such as `ptr as usize` are equally direct
  * `&fn(...) Ret` follows the same explicit `as` rules as other thin pointers, enabling FFI symbol loading and low-level code-pointer work without a separate cast primitive
  * optional carriers are constructed as ordinary enum values such as `?&u8.{ Some: ptr }` or `?&u8.None`

Core operators remain simple:

  * **Address-of (`.&` / `..&`)**:
      * `obj.&` obtains `&T`
      * `obj..&` obtains `&mut T` and requires a mutable location
  * **Dereference**: `ptr.*` (postfix)

Pointer arithmetic stays explicit:

  * `&T` / `&mut T` expose typed arithmetic through the ordinary `base.mem.ptr` implementations
  * `ptr + n` and `ptr - n` for object pointers scale by the element size (`@sizeOf[T]()`), while `ptr.byte_add(...)` and `ptr.byte_sub(...)` step in bytes
  * subtracting two identical object-pointer types yields an `isize` element distance
  * `^T` / `^mut T` retain builtin raw-address arithmetic with `usize` / `isize`, plus subtraction between identical address-pointer types
  * opaque FFI boundaries should use `&void` / `&mut void` instead of byte-pointer punning

### 2.4 Arrays and Slices

  * **Arrays**: `[N]T` - Fixed-size value type.
  * **Slices**: `&[T]` or `&mut [T]` - A fat pointer containing a pointer and a `usize` length.
  * **Arrays Are Physical Aggregates**: Arrays behave like inline structs, not like hidden reference handles.
      * `[N]T` is the only fixed-size array family. Kern does **not** define `[N]mut T`.
      * Array element writes are controlled by the mutability of the storage path that reaches the array.
      * `let mut arr = [4]u8.{ 1, 2, 3, 4 }; arr.[0] = 9;` is valid.
      * `let arr = [4]u8.{ 1, 2, 3, 4 }; arr.[0] = 9;` is rejected.
      * `struct Buffer { data: [4]u8 }; let mut buf = Buffer.{ data: [4]u8.{ 0; 4 } }; buf.data.[0] = 1;` is valid because the access path reaches mutable aggregate storage.
      * `fn fill(buf: &mut [4]u8) void { buf.*.[0] = 1; }` is valid because the mutable pointer reaches mutable array storage.
      * `fn fill(buf: &[4]u8) void { buf.*.[0] = 1; }` is rejected because the pointer path itself is read-only.
  * **Explicit Slice Permissions**: Slices remain views and therefore keep an explicit read/write split in the type.
      * `arr.&[a...b]` produces `&[T]` (read-only slice view).
      * `arr..&[a...b]` produces `&mut [T]` (mutable slice view), and requires the base storage path to be mutable.
      * The distinction is intentional: Kern does not silently upgrade a read-only view into a mutable one, even if the slice binding itself is declared with `let mut`.
      * Slice brackets accept compiler-owned `SliceBounds` range values with `usize` bounds, including `a...b`, `a...`, `...b`, `...`, `a..=b`, and `..=b`. This marker is slice-specific; signed scalar match ranges such as `-1...5` do not become valid slice bounds. Reverse traversal adapters such as `.rev()` produce iterator state values, not slice bounds.
  * **Semantic Checklist**:
      * `let mut arr = [N]T.{ ... };` makes the array storage mutable and permits `arr.[i] = ...`.
      * `let arr = [N]T.{ ... };` keeps the array storage immutable and rejects `arr.[i] = ...`.
      * `arr.[i]..&` yields `&mut T` only when the access path to `arr.[i]` is mutable; otherwise it yields `&T` or is rejected in a mutable context.
      * `arr.&[a...b]` is always `&[T]`.
      * `arr..&[a...b]` is `&mut [T]` only when the access path to `arr` is mutable.
      * Field access composes normally: mutability flows through `obj.field.[i]` from the full storage path, not from a special array-element type qualifier.
      * Passing `[N]T` across a boundary may decay to `&[T]` freely, but decay to `&mut [T]` still requires a mutable source location.
  * **String Literals**: `"Hello"` evaluates to `[5]u8`.
      * A string literal is syntax sugar for the equivalent byte array expression, such as `[5]u8.{ b'H', b'e', b'l', b'l', b'o' }`.
      * String literals do not include an implicit trailing NUL byte. Write `"\0"` explicitly when a NUL byte is part of the value.
      * Passing a string literal to a `&[u8]` parameter uses the ordinary array-to-slice decay path.
      * Storage follows the normal expression context. `static NAME = "Hello";` creates static array storage, while a call such as `write("Hello")` materializes an array value at the call site before slice decay.
  * **Compiler-owned member intrinsics**: Values with compiler-owned physical
    metadata expose that metadata through explicit `.@...()` projections rather
    than ordinary fields or methods. Arrays and slices support `.@len()` and
    `.@ptr()`. Ranges support `.@start()` and `.@end()` when the bound exists.
    Trait objects support `.@dataPtr()` and `.@vtablePtr()`. Closure fat
    pointers support `.@statePtr()` and `.@entryPtr()`. These are language
    primitives, not `impl` methods, so they remain available in freestanding
    code and do not occupy user method names.

### 2.5 SIMD Values

Kern models SIMD as explicit fixed-lane machine vectors.

  * Construction uses the ordinary typed initialization syntax:

```kern
let a = f32x4.{ 1.0, 2.0, 3.0, 4.0 };
let b = i32x4.{ 1, 2, 3, 4 };
let m = boolx4.{ true, false, true, false };
```

  * Lane access is syntax, not a library helper:

```kern
let x = a.[2];
```

  * Lane updates use the same syntax when the base storage is mutable:

```kern
let mut a = f32x4.{ 1.0, 2.0, 3.0, 4.0 };
a.[2] = 9.0;
```

  * For the current fixed-width model, SIMD lane indexes in `.[]` must be compile-time constants and must be in range.
  * SIMD values do not participate in slice semantics.
  * `#` has no SIMD meaning. Lane count is part of the type, not runtime metadata.
  * Kern does not define implicit conversion between `[N]T` and `TxN`.

### 2.6 Boundary Natural Conversion (BNC)

While Kern strictly enforces "explicit over implicit" (forbidding implicit integer narrowing, widening, or hidden control flow), it embraces **Boundary Natural Conversion (BNC)** to bridge compile-time static constraints with runtime dynamic interfaces ergonomically and safely.

BNC is a zero-cost compiler mechanism that naturally "decays" or "packages" a rigidly known compile-time type into a dynamic interface pointer when passing across function boundaries or assignments, without requiring the explicit `as` keyword.

Kern relies on four common BNC pathways:
1. **Array to Slice Decay**: A fixed-size array `[N]T` naturally converts into a dynamic slice `&[T]`. The compiler automatically extracts the memory address and synthesizes the fat pointer's length metadata using the compile-time `N`.
2. **Stateless Closure to Function Pointer**: An anonymous closure with an explicitly empty capture list (`[]`) has a memory footprint of `0`. It naturally decays into a standard C-ABI stateless function pointer `&fn(Args) Ret` (See Section 11.3).
3. **Named Struct to Anonymous Struct Decay**: A named structural type (e.g., `struct Vector { x: i32, y: i32 }`) naturally decays into an equivalent Anonymous Struct (`struct { x: i32, y: i32 }`) or its pointer variant when passed across a boundary. This enables secure "Duck Typing" without boilerplate. 
   * **Strict ABI Contract**: BNC is aggressively guarded by ABI compatibility. A native `struct` will **never** implicitly decay into an `extern struct` (and vice versa), as their underlying memory layouts are physically distinct.
4. **Trait Object Upcast**: A trait object pointer `&Sub` naturally boundary-converts to `&Super` if `Super` appears in `Sub`'s fully instantiated supertrait graph. This rewrites only the fat pointer metadata; the data pointer is unchanged.

BNC guarantees that the developer does not need to write boilerplate fat-pointer assembly code when the compiler already possesses absolute, statically proven knowledge of the underlying metadata.

## 3\. Declarations and Storage

  * **Local Variables**: `let [mut] name = Expr;`
  * **Global Statics**: `static [mut] name = Expr;`
  * **Constants**: `const NAME = Expr;`
  * **Uninitialized Memory**: `let mut x: Type = undef;`

## 4\. Const and Compile-Time Evaluation

`const` in Kern is a language-level compile-time execution mechanism, not a read-only storage class. Semantically it is much closer to a restricted `comptime` model: the compiler is allowed to interpret expressions, follow constant references, and execute explicitly marked functions in order to materialize values during compilation.

A `const` declaration binds a name to a compile-time value. It does not create a runtime object, a storage identity, a linker symbol, or a section entry. Referencing a `const` is semantically equivalent to using the compile-time value it names; any runtime storage that appears after lowering belongs to the expanded expression itself, such as anonymous backing bytes for a string literal.

Use `static` or `static mut` when a program needs a real global object with storage, identity, and addressability.

### 4.1 Design Position

Kern is freestanding by default, so compile-time evaluation is intentionally independent of the runtime library split.

  * `const` does **not** depend on `std`, `kernstd`, `libc`, or any startup object.
  * `const` does **not** enter the object file as a named entity. Public constants are still compile-time bindings, not exported data.
  * Kern does **not** need a Rust-style `core/std` split to model compile-time capability. Const evaluation is part of the language and compiler, not a special sub-library.
  * The standard library may expose more `const fn`, but the mechanism itself remains runtime-agnostic.

### 4.2 Constant Contexts

The compiler evaluates constant expressions wherever the language requires a compile-time value.

  * Global `const` initializers.
  * Array lengths such as `[N]T`.
  * Enum discriminant expressions.
  * Repeat-literal counts such as `.{ value; N }`.
  * Intrinsic or API operands that are explicitly specified as compile-time constants.

The guiding rule is simple: if a construct must be fully known before lowering and code generation, Kern routes it through the constant evaluator instead of inventing a separate ad hoc rule.

### 4.3 `const fn`

Kern uses explicit syntax:

```kern
const fn align_up(value: usize, align: usize) usize {
    let mask = align - 1;
    return (value + mask) & ~mask;
}
```

`const fn` has the following semantics:

  * It is still a normal function item in the type system and code generator.
  * It can be called at runtime like any other function.
  * It may additionally be executed by the compiler when it appears in a constant context.
  * It may be generic and may appear as a method inside `impl`.
  * It is **not** a separate ABI, calling convention, or second function model.
  * `extern const fn` is rejected. Crossing an external ABI boundary and compile-time interpretation are intentionally kept separate.

This means Kern does not hardcode special cases such as a magical compile-time-only `main`. Instead, `const fn` is the explicit marker that grants the evaluator permission to interpret a function body.

### 4.4 Evaluation Model

Kern reuses the normal semantic model as far as possible.

  * Constant evaluation resolves names in the owning module scope of the referenced constant or function.
  * `const fn` bodies may use local `let` bindings, nested blocks, `if`, `match`, and `return`.
  * Constant evaluation may call other `const fn` or supported compiler intrinsics.
  * Methods are evaluated with the same `self` model as ordinary methods; there is no separate const-method object model.

This is intentional robustness policy: Kern prefers one strong evaluator over many special-case folders spread across the compiler.

### 4.5 Rejection Policy

Kern is strict about rejecting constructs whose compile-time behavior is not yet fully specified or would imply hidden runtime effects.

  * Calling a non-`const fn` in a constant context is an error.
  * Runtime-only or effectful constructs are rejected instead of being silently approximated.
  * Unsupported constant constructs should fail loudly at compile time rather than degrading into partial evaluation with surprising semantics.

In other words, Kern treats `const` as an execution boundary with explicit admission rules, not as a best-effort optimizer hint.

## 5\. Enum Structures

### 5.1 Structs

```kern
struct Point {
    x: i32,
    y: i32,
};
```

  * **Generics**: `struct Point[T] { x: T, y: T };` (See 6.6 for Trait constraints via where clauses).
  * **Default fields**: `struct Config { port: u16 = 8080, host: u32 = 0 };`
  * **Zero-Cost Memory Layout**: By default, Kern employs a highly optimized physical layout engine. It aggressively reorders struct fields at compile-time (descending by alignment requirements, then size) to eliminate memory padding (empty holes). 
  * **C-ABI Compatibility (`extern`)**: If a struct must strictly maintain its source-code declaration order to interface with C or hardware, it must be prefixed with `extern` (e.g., `extern struct Header { ... };`). This disables reordering and guarantees standard C-ABI layout.
  * **Field puns in typed initialization**: Explicit field binding remains the canonical form (`x: x`), but typed struct initialization may use field puns when the field name and local binding name match (e.g., `Point.{x, y}`). Untyped `.{ ... }` keeps its existing contextual literal behavior and is not reclassified by syntax alone.
  * **Initialization and `undef`**: When initializing a struct using `Type.{ ... }`, any field without a default value **must** be explicitly provided; omitting it is a strict compile-time error. If you intentionally want to leave a field uninitialized, you must explicitly use `undef` (e.g., `priority: undef`). Standalone `undef` expressions require an expected type such as a field type or `let value: Type = undef;`.

```kern
// Immutable 
let p1 = Point.{x: 10, y: 20};       

// Mutable binding (Type provided on the right, mutability on the left)
let mut p2 = Point.{x: 10, y: 20}; 

let x = 10i32;
let y = 20i32;

// Standard explicit initialization
// Kern forces explicit binding to guarantee absolute clarity
let p3 = Point.{x: x, y: y};

// Typed field-pun initialization
let p4 = Point.{x, y};
```

### 5.2 Unions

```kern
union Payload {
    as_int: i32,
    as_float: f32,
    raw: [4]u8,
};
```

No active鈥慺ield tracking; no default values.

### 5.3 Simple Enum (formerly Enums)

Kern uses `enum` for strong algebraic data types. Payload-less enums may specify
a backing integer type for representation, but they remain distinct enum types
and do not implicitly inherit integer operators.

```kern
enum Color: u8 {
    Red = 0,
    Green, // 1
    Blue,  // 2
};
```

Use `extern enum` for C ABI enum shapes and other integer-backed external
protocols:

```kern
extern enum CMode: u32 {
    Read = 1,
    Write, // 2
};
```

An `extern enum` must specify an integer backing type and cannot carry payloads.
Values may flow to the exact backing integer type at explicit ABI boundaries,
for example when passing `CMode.Read` to a parameter expecting `u32`. The reverse
direction is intentionally not implicit: untrusted integers from C, hardware, or
wire formats must be validated before being treated as enum values.
An `extern enum` still does not inherit arithmetic or bitwise operator
capability; enter the backing integer type first when the value is calculation
material. C-style bitflag sets should remain integer constants until Kern grows
a dedicated flag-set model.

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

### 5.3.1 Match Value Patterns

`match` supports both structural patterns and value patterns. Structural
patterns describe the shape of the scrutinee, such as enum variants,
destructured payloads, struct fields, `_`, and ranges over scalar domains.

Value patterns are either compiler-known exact structural/scalar forms or
opaque values consumed through the `Pattern[T]` protocol. Equality is not a
pattern protocol: implementing `Eq[Rhs]` enables `==`, but it does not make a
value valid on the left side of a `match` arm.

```kern
struct IsCommand {
    name: &[u8],
};

impl IsCommand : Pattern[&[u8]] {
    type Bind = void;

    fn apply(value: &[u8]) ?Bind {
        if (value == self.name) {
            return .{ Some: {} };
        }
        return .None;
    }
}

fn classify(text: &[u8]) i32 {
    return match (text) {
        IsCommand.{ name: "kern" } => 1,
        IsCommand.{ name: "lang" } => 2,
        _ => 0,
    };
}
```

If a value expression in pattern position is not one of the compiler-known
exact scalar, enum, struct, or closed scalar range forms, Kern attempts to
consume it through the `Pattern[T]` protocol described in
[10.4 Pattern Matching](#104-pattern-matching-match). The exhaustiveness rules
remain conservative: structural enum and scalar matches can be proven
exhaustive by the compiler, while opaque user `Pattern` values still need a
catch-all arm unless another pattern form closes the domain.

### 5.4 Conversions

Type conversions are explicitly and uniformly handled by the `as` operator.

  * **Numeric Conversions**: `as` is used for all safe and unsafe numeric conversions, including bit-width truncation, zero/sign-extension, and integer/floating-point conversions (e.g., `i32 as u8`, `f32 as i32`).
  * **Pointer Reinterpretation**: `as` preserves the physical bit pattern when casting between pointer types or between pointers and `usize`/`isize`.
  * **Strict Boundaries**: The `as` operator can explicitly package compatible pointers into trait-object or closure-object fat pointers. It does not synthesize slice lengths from raw pointers, and it cannot cast arbitrary integers directly into `data` variants. Slice fat pointers must come from slice syntax such as `array.&[start...end]`.

### 5.5 Anonymous Structs

Kern treats Anonymous Structs as first-class citizens to facilitate lightweight data grouping, Duck Typing, and closure state management.

* **Structural Equivalence**: Unlike named types (where `PointA` and `PointB` are different types even if their fields match), anonymous structs are structurally typed. `struct { x: i32, y: i32 }` and `struct { y: i32, x: i32 }` are evaluated as the exact same type by the compiler through alphabetical field normalization.
* **Orthogonal `extern` Contract**: Kern's syntax is perfectly orthogonal. Just as named types can be `extern`, anonymous structs can also enforce C-ABI layout inline: `extern struct { a: u8, b: u64 }`. 
  * &Native Anonymous Structs* (`struct { ... }`): Subject to Kern's zero-cost memory reordering.
  * &Extern Anonymous Structs* (`extern struct { ... }`): Strictly preserves declaration order and padding.

```kern
// Native layout (optimized size)
let val = struct { a: u8, b: u64 }.{ a: 1, b: 2 };

// Extern layout (C-ABI compatible, maintains padding)
extern {fn process_c_data(data: &extern struct { a: u8, b: u64 }) void; }
```

## 6\. Functions and Traits

### 6.2 Implementation Blocks (`impl`)

`impl` blocks attach methods to a concrete type (including pointer types). The `self` parameter is implicitly injected and managed by the Semantic Analyzer.

The key rule is that `impl` is type-directed, not pointer-directed. A pointer type such as `&i32`, `&mut File`, or `&[&[u8]]` is simply another concrete type in the type system. If a design wants a trait to describe value semantics, it should be implemented for the value type itself (for example `impl i32 : Eq[i32]`). If a design wants pointer semantics, it should implement the pointer type explicitly.

```kern
struct Point { x: i32, y: i32 };

impl &mut Point {
    // 'self' is implicitly available as &mut Point
    pub fn move_by(dx: i32, dy: i32) void {
        self.x += dx; 
        self.y += dy;
    }
}
```

### 6.4 Traits

Traits define a VTable contract. Methods implicitly receive a `self` reference.
Traits may also declare associated types.

```kern
trait Write {
    fn write(bytes: &[u8]) usize;
};
```

```kern
trait Add[Rhs] {
    type Out;
    fn add(rhs: Rhs) Self.Out;
};
```

Kern also reserves a small set of **language-owned builtin traits** for operations and type classification. These traits are part of the compiler's semantic model, not definitions that happen to live in the standard library.

This distinction is deliberate:

  * **No compiler/std coupling**: Core operator semantics do not depend on `std` or any special "core crate".
  * **Clear generic constraints**: Generic code can state exactly which operations it needs, instead of relying on ad-hoc template-like behavior.
  * **Freestanding consistency**: Builtin operations remain available even when no standard library is linked.

Builtin traits split into two categories:

  * **Capability traits**: These describe operators and can participate in overload resolution. Equality and ordering use direct boolean-returning traits such as `Eq[Rhs]`, `Lt[Rhs]`, `Le[Rhs]`, `Gt[Rhs]`, and `Ge[Rhs]`. Arithmetic, bitwise, shift, and unary value operators use associated-result traits such as `Add[Rhs]`, `Sub[Rhs]`, `Mul[Rhs]`, `Div[Rhs]`, `Rem[Rhs]`, `BitAnd[Rhs]`, `BitOr[Rhs]`, `BitXor[Rhs]`, `Shl[Rhs]`, `Shr[Rhs]`, `Neg`, `BitNot`, and `Not`, each with an associated type `Out`.
  * **Marker traits**: These classify type families but do not imply operator capability by themselves. Kern provides `Integer`, `SignedInteger`, `UnsignedInteger`, `Float`, and `SliceBounds`.

The important rule is that marker traits are **not** shorthand for operator support or user-overload hooks. For example, `where T: Float` does not imply `where T: Add[T]`, `where T: SignedInteger` does not imply `where T: Neg`, and `where T: SliceBounds` does not mean user code can define custom slicing semantics. Generic code should constrain the exact capability it intends to use.

This keeps the system explicit:

  * use `Integer` / `SignedInteger` / `UnsignedInteger` / `Float` / `SliceBounds` when classification is the point;
  * use `Eq`, `Add`, `Neg`, and other operator traits when behavior is the point.

`SliceBounds` is compiler-owned and cannot be implemented by user code. It classifies builtin range values that may appear inside slice-view brackets, such as `usize...usize`, `usize...`, `...usize`, `...`, `usize..=usize`, and `..=usize`. Slice construction itself remains the language-owned `.&[...]` / `..&[...]` memory operation, including read/write slice permissions and storage-path mutability checks.

Associated types use direct names inside the owning trait or impl body, but in
ordinary type positions they must be projected through an explicit receiver and
trait path:

```kern
fn plus_one[T](value: T) T.Add[i32].Out
    where T: Add[i32],
{
    return value.add(1);
}
```

This keeps the owning trait visible at the projection site and avoids
unqualified `T.Out` ambiguity when multiple trait bounds are in scope.

### 6.4.1 Builtin Operators and Overloading Boundaries

Kern supports operator overloading only for operators whose meaning is ordinary value computation.

These operators are modeled through builtin capability traits:

  * equality and ordering: `==`, `!=`, `<`, `<=`, `>`, `>=`
  * arithmetic: `+`, `-`, `*`, `/`, `%`
  * bitwise and shifts: `&`, `|`, `^`, `<<`, `>>`
  * unary value operators: unary `-`, `~`, `!`

When both operands are the same SIMD shape, Kern also provides direct builtin SIMD operator semantics:

  * arithmetic and bitwise operators apply lane-wise and return the same SIMD type
  * comparison operators apply lane-wise and return `boolxN`
  * scalar control-flow sites still require plain `bool`, so SIMD masks must be reduced explicitly

Example:

```kern
let a = f32x4.{ 1.0, 2.0, 3.0, 4.0 };
let b = f32x4.{ 5.0, 1.0, 3.0, 0.0 };

let sum = a + b;   // f32x4
let mask = a < b;  // boolx4
```

Kern deliberately does **not** treat every piece of syntax as overloadable. The following remain language-owned and are not modeled as user-overridable traits:

  * short-circuit boolean operators: `and`, `or`
  * assignment family: `=`, `+=`, `-=`, `*=`, `/=`, `%=` and similar forms
  * address-of operators: `.&`, `..&`
  * dereference: `.*`
  * compiler-owned member intrinsics such as `.@len()` and `.@statePtr()`

The reason is semantic, not accidental:

  * `and` / `or` define short-circuit control flow and must preserve evaluation order and conditional execution of the right-hand side.
  * assignment forms mutate storage and belong to the language's lvalue and memory model.
  * address-of and dereference are direct memory operations, not ordinary value-level methods.
  * `#` exposes builtin runtime metadata for fat pointers and containers.

This boundary is intentional. Kern wants operator overloading where it improves generic expressiveness, but it rejects the C++ pattern where syntax that carries control-flow or memory semantics quietly becomes arbitrary user code.

### 6.5 Trait Objects (Fat Pointers)

A Trait Object is a runtime-dynamic fat pointer consisting of a data pointer and a VTable pointer. They are created from compatible pointer values.

  * **Construction**: You assemble a trait object by casting a concrete pointer with `as`, or by relying on an expected type context at a call or assignment boundary.
  * **Upcast Construction**: You may also cast a child trait object to a parent trait object, as long as the parent is present in the fully instantiated supertrait graph.
  * **Safety Rule**: To prevent stack-size ambiguity, a Trait Object can only be constructed from a pointer type.
  * **BNC Rule**: The same supertrait upcast is also allowed implicitly across assignment and call boundaries.
  * **Ambiguity Rule**: If multiple inherited parent traits contribute the same method name, an unqualified call is rejected as ambiguous.

Trait Object VTables use a two-part layout:

  * **Header**: A flattened table of all transitive parent-trait VTable pointers, expanded in declaration-order DFS after generic instantiation and deduplicated by the final instantiated trait type.
  * **Body**: Only the methods declared directly on the current trait, in declaration order.

This makes `&Sub -> &Super` upcasts a constant-time metadata rewrite while avoiding C++-style subobject pointer adjustment.

This pointer requirement belongs specifically to trait-object construction. It should not be confused with ordinary trait implementations. In other words, `&Trait` is an explicit runtime packaging form, not the semantic foundation of traits in Kern.

<!-- end list -->

```kern
let mut file = File.{ ... };
// Assemble a mutable Trait Object from a mutable pointer
let w = file..& as &mut Write;
w.write("Kern\0");
```

```kern
trait Read { fn read(buffer: &mut [u8]) usize; };
trait BufReader: Read { fn fill() void; };

let reader = file.& as &BufReader;
let base1 = reader as &Read;  // explicit upcast
use_reader(reader);           // implicit BNC upcast to &Read
```

### 6.6 Generic Constraints (`where` clauses)

Unlike some languages where generic parameter declaration and trait bounding are mixed, Kern enforces a strict separation between **generic introduction** and **type bounding** using `where` clauses. Because Kern is strictly type-oriented, constraints must explicitly specify the exact type derivation being bounded.

* **Explicit Separation**: Generic parameters are introduced first (e.g., `impl[T]`), and bounds are applied via `where`.
* **Orthogonal Pointer Constraints**: Kern's strict type system allows you to constrain different pointer derivations of the same generic type independently. For example, `where &T: TraitA, &mut T: TraitB` is entirely valid. The compiler treats each pointer level and mutability qualifier as a distinct type subject to its own traits.
* **Value-First Semantics**: If a trait models the behavior of a value, the natural bound should target the value type directly (`where T: Eq[T]`, `where K: Hash[K]`). Pointer-shaped bounds should be reserved for APIs whose semantics are genuinely about pointer types or explicit trait objects.

**Implementation Blocks with Constraints:**
In the following example, `impl[T]` introduces the generic `T`. `&List[T] : Formatable` defines that we are implementing the `Formatable` trait for the type `&List[T]`. The `where` clause specifies the prerequisite: this implementation only exists if `&T` itself is `Formatable`.

```kern
impl[T] &List[T] : Formatable
    where &T: Formatable,
{
    pub fn write_to(writer: &mut Write) void {
        let _ = writer.write("<List len=");
        // ... (implementation details)
        let _ = writer.write("]>");
    }
}
```

`"{}".fmt(args)` builds a zero-allocation formatting view over `Formatable` values, and that view can be printed or written to any `Write` sink. Kern's format spec is written directly inside the braces: `{8}` sets a minimum width, `{02}` uses zero padding, `{>8}` right-aligns, `{<8}` left-aligns, `{^8}` centers, and `{0>8}` combines an explicit fill byte with alignment. `{.12}` limits output to 12 bytes, and it can be combined with width as `{>8.12}`. Literal braces are written as `{{` and `}}`.

**Type Declarations with Constraints:**
`where` clauses are also used when defining generic data structures to enforce invariants at the type level.

```kern
struct Point[T]
    where &T: Formatable
{
    x: T,
    y: T,
}
```

## 7\. Control Flow

### 7.1 Conditional Expressions

`if` is an expression.

```kern
let a = if (b < 10) 10i32 else 20i32;
```

### 7.2 Match Expressions

Enhanced pattern matching and branching. `match` replaces `switch` for all
branching logic (integers, strings, and `enum` variants). No fallthrough.

  * **Ranges**: `...` defines a left-closed, right-open range. `..=` defines a fully inclusive range. In expression position these construct builtin range values with canonical type forms such as `T...T`, `T..=T`, `...T`, `..=T`, `T...`, and `...`. These are named builtin type families with compiler-owned layout; their `start` and `end` storage is not source-level field API. In match pattern position, closed scalar ranges are compiler-known no-binding patterns. All builtin integer types may be used as scalar match range bounds, including signed ranges such as `-1...5`. Descending traversal is not a separate range spelling; packages may expose adapters such as `(a...b).rev()` and `(a..=b).rev()` that consume the same range value and return iterator state.

<!-- end list -->

```kern
let result = match (val) {
    1...10 => 10,       // 1 to 9
    11, 12, 13 => 20,
    14..=15 => 30,     // 14 and 15
    _ => 0,
};
```

  * **Exhaustiveness**: Match expressions must be exhaustive. When matching on a `enum` type, `_ =>` is not required if all variants are explicitly matched.

### 7.3 Loops

Kern has explicit condition loops and iterator loops.

```kern
while (cond) { ... }
while (true) { ... }             // infinite loop
for (item: values.iter()) { ... }
```

`for (pat: expr) body` is parser sugar for binding `expr` to a mutable hidden
iterator and repeatedly calling `hidden..&.next()`. Range expressions have no
special loop privilege: `for (i: 0...n)` works only when the compiled package
provides an ordinary `next()` surface for that builtin range value. The compiler
and package tools do not depend on `base` or `std` range adapters. Reverse
iteration follows the same rule: `for (i: (0...n).rev())` is ordinary adapter
composition, not a second descending range syntax.

### 7.4 Defer

Executes an expression or block when the **current lexical scope (block `{\}`)** exits (LIFO). `defer` is strictly block鈥憇coped, not function鈥憇coped.

```kern
let ptr = malloc(1024);
defer free(ptr);
```

### 7.5 Blocks, Expressions, and Discards

Blocks evaluate to their last expression.
Kern strictly mandates that returned values cannot be implicitly ignored to prevent logical errors in systems programming.

  * **Explicit Discard**: If a function or expression returns a value that is intentionally unused, it **must** be bound to the discard identifier `_`.
    `let _ = file.write(buf); // Explicitly discard the returned usize`
  * `expr;` evaluates to `void`. Dropping a non-void return value by simply appending a semicolon is a compiler error.

**Evaluation Order with Defer:**
When a block `{ ... }` evaluates as an expression and contains `defer` statements, the exact exit sequence is:

1.  **Evaluate**: Compute the value of the final expression.
2.  **Execute**: Run all `defer` statements registered in the current block in LIFO order.
3.  **Yield**: Pass the computed value to the outer context.

> **Warning**: Returning a pointer to a resource that is freed by a `defer` within the exact same block will result in a dangling pointer. Kern prioritizes explicit execution order over implicit memory protection.

## 8\. Modules

Kern's module system is designed to be explicit, highly predictable, and
strictly controlled by the programmer. It uses an explicit module tree
declaration model to support robust visibility control, re-exports, and
conditional compilation.

### 8.1 Explicit Module Tree (`mod`)

Files and directories do not implicitly become part of the compilation unit just by existing on the filesystem. A module must be explicitly declared using the `mod` keyword.

  * **File Modules**: `mod utils;` instructs the compiler to look for `utils.kn`.
  * **Directory Modules**: If `utils` is a directory, the compiler looks for `utils/mod.kn`.
  * **Inline Modules**: `mod utils { ... }` declares the same child module without a separate entry file.
  * **Visibility**: By default, modules are private. Use `pub mod utils;` to expose a module publicly, `pub.. mod utils;` to expose it to the parent module subtree, or `pub/ mod utils;` to expose it throughout the current package.

<!-- end list -->

```kern
// Explicitly build the module tree
mod memory;
pub mod process;
pub.. mod detail;

mod inline_detail {
    pub.. fn helper() void {}
}

// Conditional module compilation (e.g., in std/os/mod.kn)
#[if(os == "linux")]
mod linux;

#[if(os == "windows")]
mod windows;
```

Inline modules are module nodes, not textual includes. A file-backed child declared inside an inline module is resolved under that module's child directory, so `mod api { mod detail; }` looks for `api/detail.kn` or `api/detail/mod.kn`.

### 8.2 Imports and Path Resolution (`use`)

Kern splits import roots explicitly instead of overloading one "absolute" syntax:

1.  **External package root**: bare imports such as `use std.io;` resolve only through CLI alias mappings like `--module-path std=./libs/std`.
2.  **Current module**: `use .utils;`
3.  **Parent module**: `use ..common.types;`
4.  **Current package root**: `use /host.os;`

Grouped imports keep the same anchor as their base path, for example `use /host.os.{Handle, write, exit};`.

### 8.3 Facade Pattern and Re-exports (`pub use`)

Kern supports the Facade pattern via `pub use`. This allows you to construct a clean, unified public API while keeping the internal module layout complex and conditionally compiled. Kern also supports `pub..` when an API should be visible throughout the parent module subtree, and `pub/` when it should stay package-internal without becoming fully public.

```kern
// host/os/mod.kn
#[if(os == "linux")]
mod linux;

// Re-export symbols from the private `linux` module to the public `host.os` API
#[if(os == "linux")]
pub use .linux.{Handle, get_stdout_handle, write, exit};

// Re-export a helper to the parent facade subtree.
pub.. use .linux.write as write_linux;
```

### 8.4 Multi-Pass Resolution

Kern utilizes a multi-pass Semantic Analyzer. Circular type dependencies across different module files (e.g., Module A uses a struct from Module B, which contains a pointer to a struct from Module A) are fully supported natively. There is no need for C-style forward declarations or header files.

## 9\. Interoperability

Kern uses the C Application Binary Interface (ABI) as the universal language for all external communication.

### 9.1 Name Mangling and Exporting to C/Assembly

To safely support Generics, Modules, and Trait implementations without symbol collisions, Kern uses a deterministic, **Itanium-style Name Mangling Engine** (e.g., a generic method might be compiled as `_K3std11collections15ArrayListI3i32E3new`).

Because of this, internal Kern functions are physically invisible to standard C linkers by their raw names. To export a function to C, Assembly, or to expose a runtime-facing symbol, you must use the `extern` modifier. 

The `extern` keyword acts as an explicit ABI boundary contract: it forces the compiler to use the standard C calling convention and **completely disables name mangling** for that symbol.

This top-level form is specifically for **exported ABI definitions** such as runtime entry points or functions intentionally exposed to C/Assembly. It is not the syntax for importing foreign symbols.

**Root Program Entry Symbol:**
Kern remains freestanding by default, but when a runtime entry contract is enabled the compiler treats the root `main` as a special program-entry symbol.

The legal forms are:

  * `fn main() i32`
  * `fn main(argc: i32, argv: &&u8) i32`

This is intentionally narrow:

  * `main` must live in the root module
  * `main` must not be `extern`
  * `main` must not be generic
  * `main` must return `i32`

The special treatment applies only to `main` under program-entry mode. Other exported ABI symbols still require explicit ABI-facing declarations and attributes.

**First-Class Test Cases:**
Kern test cases use the same low-level entry shape as `main`, but are selected with `#[test]` instead of a magic name.

The legal forms are:

  * `#[test] fn name() i32`
  * `#[test] fn name(argc: i32, argv: &&u8) i32`

The return value follows process-style status convention: `0` means pass and any non-zero value means fail. Kern does not assign portable semantic meaning to non-zero values beyond "this case failed"; platform-specific status truncation or encoding remains a property of the host/runtime boundary.

This is intentionally compiler-owned and library-independent:

  * `#[test]` functions must not be `extern`, `const`, generic, variadic, or bodyless
  * test functions may be private and may live in nested modules
  * each test case name is its module path plus function name, such as `math::adds`
  * `#[if(test)]` is enabled only when the driver compiles in test mode
  * the compiler and tools do not depend on `base`, `std`, `rt`, or `kernlib` to discover or dispatch test cases

Tooling should compile one test binary per test target, then invoke that binary once per discovered case. Each invocation selects one case and returns that case's `i32` status. This preserves the simple entry contract while giving tools process isolation, parallel scheduling, and direct failure attribution.

Startup ownership still belongs to the surrounding runtime/link environment:

  * a toolchain-owned runtime path such as `rt` may own startup and call the compiler-synthesized main adapter
  * a hosted C runtime may own initial process startup and call `main`
  * a freestanding object build may choose `runtime_entry = none`, in which case no special program entry is required

When `runtime_entry != none`, the toolchain also loads `rt` as the startup companion root even if the program never imports `rt` explicitly. This is startup assembly only. It does **not** make ordinary `rt.*` APIs visible without `use`, and it does **not** implicitly inject `base`.

Hosted does not imply libc. In Kern, "hosted" means an OS process environment exists. Libraries such as `std` reach hosted services through internal `std.host` modules, while libc remains an optional external package choice rather than a semantic prerequisite for the language or standard library.

When a runtime entry contract is enabled, the root `main` definition looks like:

```kern
use std.io;

fn main() i32 {
    "hello, {}!".fmt(.{"world"}).println();
    0
}
```

This does **not** mean arbitrary function names gain runtime meaning. It means the selected runtime entry contract consumes the root `main` definition when program-entry mode is enabled.

For argument-bearing `main`, Kern uses the explicit low-level ABI `argc: i32, argv: &&u8`. Higher-level wrappers belong in ordinary libraries such as `std.proc`, not in the compiler-owned entry contract itself.

### 9.2 Importing External Functions and Statics

External C functions can use the `...` syntax to support C-style variadic arguments. External statics are declarations and do not need initializers. Items inside an `extern` block can be marked `pub` to expose them through the Kern module system.

Kern intentionally splits the two directions of ABI usage:

* **Exporting** uses a top-level definition such as `fn main() i32 { ... }`.
* **Importing** uses an `extern { ... }` block such as `extern { fn printf(format: &u8, ...) i32; }`.

Single imported functions or statics must still use an `extern` block; they are not written as standalone `extern fn foo(...);` items.

```kern
extern {
    pub fn malloc(size: usize) &mut u8;
    pub fn printf(format: &u8, ...) i32;
    pub static MULTIBOOT_MAGIC: u32;
}
```

## 10\. Enum Types (`enum`) and Pattern Matching

Kern uses `enum` for all tagged unions and enumerations, paired exclusively
with `match` for branching.

### 10.1 Defining Enum Types

Use the `enum` keyword to define tagged unions with payloads (Algebraic Enum Types).

```kern
enum Message {
    Data: i32,
    Closed,
};
```

### 10.2 Builtin Optional and Result Carriers

Kern provides builtin carrier type families for optional values and
result-carrying values:

  * optional: `?T`
  * result: `T!E`

These are canonical language forms, not library aliases that happen to enjoy
special treatment.

They also do not receive hidden representation privileges. In particular,
builtin `?T` / `T!E` are not special nullable-pointer or ABI escape hatches;
they are builtin enum families and should be reasoned about the same way as
ordinary enums with the same shape.

```kern
let present = ?i32.{ Some: 7 };
let absent = ?i32.None;

let ok = i32!&[u8].{ Ok: 7 };
let err = i32!&[u8].{ Err: "bad" };
```

Kern also provides a direct propagation operator:

  * on `?T`, `value.?` extracts `Some` or returns `None`
  * on `T!E`, `value.?` extracts `Ok` or returns `Err`

### 10.3 Elided Initialization Syntax

Where the target type context is strictly explicit (e.g., function returns, arguments, explicit variable declarations), **any type** (including Enum, Arrays, and Structs) can be initialized using the elided literal syntax `.{ ... }`.

```kern
fn safe_divide(a: i32, b: i32) i32!i32 {
    if (b == 0) return .{ Err: -1 }; 
    return .{ Ok: a / b };
}
```

### 10.4 Pattern Matching (`match`)

Pattern matching is the only way to access the payload of a `enum` variant. Bindings within a match arm can be made mutable.

```kern
match (value) {
    .{ Some: mut val } => {
        val += 1; 
        printf("%d\n\0", val);
    },
    .None => printf("Nothing\n\0"),
}
```

Constructor syntax keeps the same surface form in pattern position. The
compiler interprets `Type.{ ... }` as an enum payload pattern or a struct
destructuring pattern after type checking the target type, and nested fields are
recursively parsed as patterns:

```kern
Tree.{ Branch: Node.{ left: Leaf.Empty, right } } => use(right),
Tree.{ Branch: Node.{ left: Leaf.Empty, right: right } } => use(right),
```

Every match arm is checked through a common binding model. User-defined pattern
values use the `Pattern[T]` protocol over the matched value type:

```kern
trait Pattern[T] {
    type Bind;
    fn apply(value: T) ?Bind;
}
```

`Bind` is the binding environment produced by a successful pattern. A pattern
with no bindings has `Bind = void`. A pattern such as `.{ Some: value }`
produces a structural binding environment equivalent to:

```kern
struct { value: T }
```

The compiler derives this environment from structural pattern syntax. User code
does not spell it at the match site.

Any ordinary expression in value-pattern position may be used as an opaque user
pattern when its type implements `Pattern[T]` for the scrutinee type:

```kern
struct IsEven {};

impl IsEven : Pattern[i32] {
    type Bind = struct { value: i32 };

    fn apply(value: i32) ?Bind {
        if ((value % 2) == 0) {
            return .{ Some: .{ value: value } };
        }
        return .None;
    }
}

match (n) {
    IsEven.{} => value,
    _ => 0,
}
```

Only `Bind = void` and `Bind = struct { ... }` are currently valid binding
shapes. Struct fields become arm-local bindings. User `Pattern` values are
opaque to static coverage analysis: they can match at runtime, but they do not
prove exhaustiveness or make later arms unreachable.

When an arm has multiple alternative patterns, every alternative must produce
the same `Bind` shape: the same field names, field types, and mutability. Field
order is canonicalized by name, so the source order of nested bindings does not
matter. This keeps the arm body in one coherent scope:

```kern
.{ Int: n }, .{ Float: n } => use(n), // valid: both bind `struct { n: T }`
.{ Int: n }, .{ Float: other } => bad, // invalid: different binding names
```

Exact value patterns and closed scalar range patterns are no-binding patterns.
They participate in the same binding model with `Bind = void`; compiler-known
forms also participate in exhaustiveness and unreachable-pattern analysis.

### 10.5 Refutable `let` and `let else`

Kern treats `let else` as a first-class control-flow construct, not as a
truncated `match`.

Its job is specific: one pattern is the privileged success path, and if that
pattern matches, its bindings continue into the surrounding scope.

```kern
let .{ Some: value } = maybe_value else return 0;
use_value(value);
```

This is distinct from `match`:

  * `match` branches between peers and produces a value
  * `let else` privileges one success pattern and only routes failures away

Refutable `let` patterns must use `else`. Irrefutable `let` patterns must not.

```kern
let .{ Ok: handle } = open(path) else return 1;
```

The short form stays valid:

```kern
let PAT = EXPR else DIVERGING_EXPR;
```

It is language sugar for a failure-only `else` block:

```kern
let PAT = EXPR else {
    _ => DIVERGING_EXPR,
};
```

Kern also supports a structured failure block:

```kern
let .{ Ready: value } = state else {
    .{ Pending } => return 1,
    .{ Failed: err } => return err.code,
    _ => return 9,
};
```

After `else`, a plain expression is still allowed. In particular:

  * `else { return 0; }` is an ordinary block expression
  * `else { PAT => EXPR, ... }` is a failure-arm block

Kern does not reinterpret an ordinary block as a hidden `match`.

The semantics are:

  * `PAT` is tested first against `EXPR`
  * if `PAT` matches, its bindings are introduced into the outer scope after
    the statement
  * if `PAT` does not match, control enters the `else` block
  * each `else` arm matches against the original value, but only within the
    remaining failure space not already accepted by the main `let` pattern
  * bindings introduced by an `else` arm are local to that arm
  * every `else` arm must diverge via `return`, `break`, `continue`, panic, or
    another diverging expression

Coverage in the `else` block is checked against the remaining failure space:

  * `_` means "all remaining failures"
  * if `_` is omitted, the listed `else` arms must exhaust every failure case
    left over after the main `let` pattern
  * an `else` arm that matches no remaining failure space is invalid

This design keeps `let else` narrow and intentional. It is for "bind the good
case here, handle failures immediately", not for replacing `match`. Multi-way
failure handling uses the block form.

## 11. Closures and Anonymous Functions

Kern explicitly separates the physical state of a closure from its dynamic invocation interface. A closure in Kern is not a magical opaque type; it is fundamentally an anonymous structure combined with a function.

### 11.1 Syntax and Capture Assignments

Closures use the `[captures](args) ReturnType { ... }` syntax. 
Capturing must be explicit and follows **Pure Value Semantics**. You define bindings in the capture list using `=`. If the target binding name matches a local variable in scope, you can use the capture elision shorthand. Unlike struct initialization, which requires strict `field: value` pairs, closure capture lists uniquely permit this safe shorthand.

```kern
let a = 120i32;
let mut counter = 0i32;

// Explicit binding (`ptr = counter..&`) and elided capture binding (`a` stands for `a = a`)
let closure = [a, ptr = counter..&](b: i32) i32 {
    ptr.* += 1;
    return a + b;
};
```

### 11.2 The Dual-Type Nature of Closures

Understanding closures in Kern requires distinguishing between two distinct types:

1. **The Anonymous Closure State**: When you write `[a]() { ... }`, it evaluates to a value of a compiler-generated, highly specific anonymous struct type (e.g., `__Lambda_1`). You cannot directly write the name of this type in code (though it can be queried via `@typeOf`). By default, it lives on the stack.
2. **The Closure Fat Pointer (`&Fn` / `&mut Fn`)**: This is the universal, dynamic interface for executing a closure. It is a primitive fat pointer with a hardcoded layout: `{ data_ptr: &void, code_ptr: &void }`. 
    * `&Fn(Args) Ret`: An immutable closure pointer (read-only access to captured state).
    * `&mut Fn(Args) Ret`: A mutable closure pointer (can mutate captured state).

### 11.3 Boundary Natural Conversion and Decay

Kern seamlessly bridges the Anonymous Closure State and the Closure Fat Pointer through **Boundary Natural Conversion (BNC)** (See Section 2.5).

When an Anonymous Closure State is passed to a context explicitly expecting a closure pointer (like a function argument or return type), the compiler automatically packages the anonymous struct's address and the generated code pointer into a `&Fn` or `&mut Fn` fat pointer. 

Furthermore, if the capture list is strictly empty `[]`:
* The resulting Anonymous Closure State has a size of `0` (`@sizeOf` yields 0).
* **BNC Decay Rule**: It naturally boundary-converts into a standard, stateless C-ABI function pointer: `&fn(Args) Ret`.

```kern
// Naturally decays to '&fn(i32, i32) bool' via BNC
arr.sort([](a: i32, b: i32) bool {
    return a < b;
});
```

### 11.4 Explicit Escape, Heap Allocation, and State Projection

Because closures evaluate to standard structs on the stack, escaping a closure requires explicitly allocating memory for its anonymous type and manually assembling the `&Fn` fat pointer.

Kern strictly preserves **abstraction consistency**. Fat pointers (`&Fn`) are primitive types, not standard structs. Therefore, Kern explicitly forbids abstraction leaks like accessing internal fields directly (e.g., `cb.data`). To retrieve the original data pointer for memory deallocation, use the compiler-owned member intrinsic `.@statePtr()`.

```kern
// 1. Stack-allocated closure state (Anonymous Type)
let closure = [a](b: i32) i32 { return a + b; };

// 2. Explicitly allocate heap memory using @typeOf
let size = @sizeOf[@typeOf(closure)]();
let raw = malloc(size) as &mut @typeOf(closure);
raw.* = closure;

// 3. Explicitly cast the state pointer to a Closure Fat Pointer
let heap_cb = raw as &mut Fn(i32) i32;

// --- Later, when memory needs to be freed ---

// 4. Extract the anonymous state pointer explicitly.
let ptr_to_free = heap_cb.@statePtr() as &mut u8;
free(ptr_to_free, size); 
```

## 12\. Inline Assembly (`@asm`)

To maintain Kern's philosophy of "explicit over implicit", inline assembly does not use format strings with hidden index bindings. Instead, it leverages Kern's elided struct literal syntax (`.{ ... }`) to create a strict, named mapping between CPU registers and Kern variables.

### 12.1 Syntax, Register Binding, and Compile-Time Validation

The parameters passed to `@asm` (such as the `asm` template string, `clobbers`,
and `volatile` flag) are **not runtime structures**. They are compiler-owned
metadata resolved and validated at compile time, then consumed by later
lowering/codegen stages rather than materialized as ordinary runtime values.

The `asm` field itself must be exactly one string literal. For multi-line
assembly, use Kern's multiline string syntax rather than an array of strings.

```kern
pub fn outb_and_read(port: u16, data: u8) u8 {
    let mut status: u8 = undef;

    @asm(.{
        asm:
            \\out dx, al
            \\in al, dx
        ,
        outputs: .{ al: status..& },   // Binds register to mutable pointer
        inputs: .{ dx: port, al: data },
        clobbers: .{ "memory" },      // Compile-time known
        volatile: true                // Compile-time known
    });

    return status;
}
```

## 13\. AST Attributes and Metadata (`#[...]` and `#![...]`)

Kern completely rejects traditional C-style preprocessor macros, substituting them with an **Attribute Mini-Language**. Attributes are strictly parsed by the frontend and natively understood by the compiler backend to control memory layout, linkage, and optimization.

### 13.1 Scope: Outer vs. Inner Attributes

  * **Outer Attributes (`#[...]`)**: Attached to the immediately following AST node (e.g., a function, struct, or variable declaration).
  * **Inner Attributes (`#![...]`)**: Applies to the entire enclosing lexical scope (usually the file). If placed at the top of an `mod.kn` file, the attribute applies to the entire module.

### 13.2 Mutually Exclusive Content

Kern strictly enforces single-responsibility for attribute brackets. The content inside the brackets `[...]` must be **either** a condition evaluator **or** a list of metadata tags.

#### 1\. Conditional Compilation (`if(...)`)

Uses a strict boolean evaluator at compile-time. If the condition evaluates to `false`, the target node (or file) is entirely pruned before semantic analysis. It supports logical operators (`and`, `or`, `!`) and checking custom compiler flags (`--define key=value`).

```kern
#![if(os == "bare_metal")]
#[if(!debug_mode)]
```

#### 2\. Metadata Tags

A comma-separated list of tags attached to the AST for compiler side-effects. Metadata tags are grouped by their specific impact on the generated binary:

**A. Linkage & FFI Control**

  * `export_name("...")`: Overrides the mangled name with a specific string for the linker.
  * `link_section("...")`: Forces a global variable or function into a specific ELF/Mach-O/COFF section (crucial for OS bootloaders, e.g., `#[link_section(".multiboot")]`).
  * `retain`: Forces a function or global to remain reachable even when no Kern code references it directly. This is orthogonal to symbol export and section placement.

**B. Memory Layout**

  * `packed`: Removes all padding between struct/union fields. The size becomes exactly the sum of its fields, at the cost of potential unaligned memory access penalties.
  * `align(N)`: Forces the alignment of a struct or static variable to `N` bytes (e.g., `#[align(4096)]` for page tables).

**C. Optimization & Control Flow**

  * `cold`: Marks a function as rarely executed, moving it out of the hot instruction cache and optimizing branching.
  * `naked`: Instructs the compiler to omit the standard function prologue and epilogue. Strictly used for hardware interrupt handlers and contextual context-switching alongside `@asm`.
  * `inline` / `noinline`: `inline` requests forced inlining, while `noinline` forbids it for a specific function.
  * `target_feature("...")`: Attaches explicit backend CPU feature requirements to a function. The payload is a comma-separated feature list such as `#[target_feature("avx2,fma")]`.

-----

## 14\. Compiler Intrinsics (`@...`)

Intrinsics are special functions implemented directly within the Kern compiler backend (e.g., LLVM). They are prefixed with `@` to strictly separate them from user-defined functions. They are used for operations that alter data representation, query compile-time information, or emit specialized CPU instructions.

### 14.1 Compile-Time Type Information

These intrinsics evaluate completely at compile-time and incur zero runtime overhead.

  * `@typeOf(expr) -> Type`: Evaluates to the exact compile-time type of the provided expression. This is strictly a type-context intrinsic (it returns a type representation, not a value). It is crucial for manipulating anonymous types, such as allocating memory for closures (`@sizeOf[@typeOf(closure)]()`).
  * `@sizeOf[T]() -> usize`: Returns the memory footprint (size in bytes) of type `T`.
  * `@alignOf[T]() -> usize`: Returns the ABI-required alignment (in bytes) of type `T`.

### 14.1.1 Member Intrinsics

Member intrinsics use receiver syntax with an `@`-prefixed projection name:
`value.@name()`. They are compiler-owned primitives and are not ordinary
methods. This keeps low-level representation access explicit without reserving
plain method names such as `len` or `ptr`.

  * `array.@len() -> usize`, `slice.@len() -> usize`: returns the element count.
  * `array.@ptr() -> &T`, `slice.@ptr() -> &T` / `&mut T`: returns the data pointer.
  * `range.@start()`, `range.@end()`: returns the present bound of a range value.
  * `trait_object.@dataPtr() -> &void` / `&mut void`: returns the dynamic receiver data pointer.
  * `trait_object.@vtablePtr() -> &void`: returns the vtable pointer.
  * `closure.@statePtr() -> &void` / `&mut void`: returns the captured state pointer.
  * `closure.@entryPtr() -> &void`: returns the closure entry pointer.

Libraries may wrap these primitives with ordinary methods for fluent user code,
for example `slice.len()`, `slice.ptr()`, `range.start()`, `writer.data_ptr()`,
and `callback.state_ptr()`. The compiler only owns the `.@name()` primitive
spelling; plain method names remain normal library API.

### 14.2 Hardware & Execution Control

  * `@unreachable() -> !`: Emits an unreachable instruction. Informs the optimizer that a control flow path is physically impossible, allowing it to eliminate dead branches (often used in exhaustiveness fallback).
  * `@trap() -> !`: Emits an illegal instruction (`llvm.trap`) to deliberately crash/halt the program securely.
  * `@fence(order)`: Emits an explicit memory fence with programmer-specified ordering. `order` must be a compile-time constant and may be `Acquire`, `Release`, `AcqRel`, or `SeqCst`.
  * `@breakpoint()`: Triggers a hardware breakpoint (`llvm.debugtrap`) for system debuggers.

*(Note: Kern does not provide `@volatileLoad` or `@volatileStore` intrinsics. Instead, Kern treats volatility as a first-class type qualifier (`^T` and `^mut T`). Hardware register accesses are performed via standard dereferencing `ptr.*` on a volatile pointer, yielding perfectly predictable code without intrinsic clutter.)*

### 14.3 Bitwise Math & Memory Operations

Mapped directly to single-cycle CPU instructions and highly optimized backend primitives where available:

  * `@popCount[T: Integer](val: T) -> T`: Returns the number of set bits (1s).
  * `@clz[T: Integer](val: T) -> T`: Count leading zeros.
  * `@ctz[T: Integer](val: T) -> T`: Count trailing zeros.
  * `@bswap[T: Integer](val: T) -> T`: Reverses the byte order of an integer value (useful for endianness conversions).
  * `@memcpy(dest: &mut u8, src: &u8, len: usize) void`: Performs a highly-optimized bulk memory copy.
  * `@memmove(dest: &mut u8, src: &u8, len: usize) void`: Performs an overlap-safe bulk memory move.
  * `@memset(dest: &mut u8, val: u8, len: usize) void`: Performs a highly-optimized bulk memory fill.

The `Integer` bound here is a marker-style family constraint. It expresses that these intrinsics operate on integer types as a category. It does **not** mean `Integer` is the source of arithmetic or bitwise operator semantics. Operator syntax remains governed by the builtin capability traits described in Section 6.4.1.

### 14.4 Atomic Operations and Memory Ordering

Kern exposes lock-free atomic operations through dedicated compiler intrinsics rather than inline assembly. This preserves optimizer visibility while still lowering directly to LLVM atomic IR with zero runtime abstraction overhead.

Atomic operations require an explicit compile-time memory ordering constant. The compiler consumes the following stable integer ABI:

```kern
Relaxed = 0
Acquire = 1
Release = 2
AcqRel  = 3
SeqCst  = 4
```

These numeric values are part of Kern's intrinsic ABI contract. The compiler maps them to the backend's actual atomic ordering semantics; source code does not depend on LLVM's internal enum numbering.

The base library provides named wrappers in `base.sync`:

```kern
use base.sync.{MemOrder, atomic, ACQUIRE, RELEASE};

let load_order = ACQUIRE;
let store_order = RELEASE;

let mut counter = atomic[usize](0);
counter..&.store[RELEASE](1);
let current = counter.&.load[ACQUIRE]();
```

`base.sync.MemOrder` is an `extern enum: u8`. The safe library wrappers take
that enum as their const generic ordering parameter. Low-level compiler
intrinsics remain freestanding and take raw `u8` ABI codes; `MemOrder` values
can cross that boundary directly because their backing type is exactly `u8`.

Supported atomic value types are:

  * Native integers: `i8`..`i128`, `u8`..`u128`, `isize`, `usize`
  * Normal raw pointers: `&T`, `&mut T`

Rejected types include `bool`, floating-point types, volatile pointers (`^T`, `^mut T`), slices, arrays, trait objects, closure fat pointers, and any other non-thin-pointer aggregate.

Kern is freestanding and does not permit LLVM to lower oversized atomics into runtime helper calls such as `__atomic_*`. The compiler therefore rejects atomic widths larger than the target's lock-free limit at compile time.

  * `@atomicLoad[T](ptr: &T, order: u8) -> T`
    `order` must be `Relaxed`, `Acquire`, or `SeqCst`.
  * `@atomicStore[T](ptr: &mut T, val: T, order: u8) void`
    `order` must be `Relaxed`, `Release`, or `SeqCst`.
  * `@atomicCas[T](ptr: &mut T, expected: T, desired: T, succ: u8, fail: u8) -> struct { success: bool, value: T }`
    This is a strong compare-and-exchange. `fail` must be `Relaxed`, `Acquire`, or `SeqCst`, and it cannot be stronger than `succ`.
  * `@atomicCasWeak[T](ptr: &mut T, expected: T, desired: T, succ: u8, fail: u8) -> struct { success: bool, value: T }`
    This is a weak compare-and-exchange and may fail spuriously. `fail` must be `Relaxed`, `Acquire`, or `SeqCst`, and it cannot be stronger than `succ`.
  * `@atomicXchg[T](ptr: &mut T, val: T, order: u8) -> T`
    Supports integer and normal raw-pointer payloads.
  * `@atomicRmwAdd[T](ptr: &mut T, val: T, order: u8) -> T`
  * `@atomicRmwSub[T](ptr: &mut T, val: T, order: u8) -> T`
  * `@atomicRmwAnd[T](ptr: &mut T, val: T, order: u8) -> T`
  * `@atomicRmwNand[T](ptr: &mut T, val: T, order: u8) -> T`
  * `@atomicRmwOr[T](ptr: &mut T, val: T, order: u8) -> T`
  * `@atomicRmwXor[T](ptr: &mut T, val: T, order: u8) -> T`
  * `@atomicRmwMax[T](ptr: &mut T, val: T, order: u8) -> T`
  * `@atomicRmwMin[T](ptr: &mut T, val: T, order: u8) -> T`
  * `@atomicRmwUMax[T](ptr: &mut T, val: T, order: u8) -> T`
  * `@atomicRmwUMin[T](ptr: &mut T, val: T, order: u8) -> T`
    These read-modify-write intrinsics are integer-only. Their `order` must be one of `Relaxed`, `Acquire`, `Release`, `AcqRel`, or `SeqCst`.
  * `@fence(order: u8) void`
    `order` must be `Acquire`, `Release`, `AcqRel`, or `SeqCst`.

For both compare-and-exchange intrinsics, the operand evaluation order is fixed: `ptr`, then `expected`, then `desired`, from left to right. This matters when `expected` or `desired` contains side effects.

Atomic synchronization is for shared memory, not MMIO. Device registers should continue to use Kern's volatile pointer types and ordinary dereferencing rules.

### 14.5 SIMD Intrinsics

Kern keeps SIMD as a builtin type family first, and reserves `@...` intrinsics only for operations that do not map cleanly onto ordinary expression syntax.

  * `@simdAny(mask: boolxN) -> bool`
    Returns `true` when any lane in `mask` is `true`.
  * `@simdAll(mask: boolxN) -> bool`
    Returns `true` only when every lane in `mask` is `true`.
  * `@simdBitmask(mask: boolxN) -> usize`
    Packs the mask into scalar bits so that lane `i` becomes bit `i` of the returned `usize`. This requires `N <= bit_width(usize)` on the current target.
  * `@simdSelect(mask: boolxN, on_true: TxN, on_false: TxN) -> TxN`
    Performs lane-wise selection. Lane `i` comes from `on_true.[i]` when `mask.[i]` is `true`, otherwise from `on_false.[i]`.
  * `@simdShuffle(lhs: TxN, rhs: TxN, indices: [N]u32) -> TxN`
    Produces a new vector by selecting lanes from the concatenated pair `lhs ++ rhs`. Index `0` addresses `lhs.[0]`, while index `N` addresses `rhs.[0]`.
  * `@simdSwizzle(value: TxN, indices: [N]u32) -> TxN`
    Unary lane permutation shorthand for the common case where every selected lane must come from `value` itself. Every index must be a compile-time constant in `0..N-1`.
  * `@simdReverse(value: TxN) -> TxN`
    Returns the same vector with its lane order reversed.
  * `@simdRotateLeft(value: TxN, amount: usize) -> TxN`
    Rotates lanes toward lower indices. `amount` must be a compile-time constant.
  * `@simdRotateRight(value: TxN, amount: usize) -> TxN`
    Rotates lanes toward higher indices. `amount` must be a compile-time constant.
  * `@simdInterleaveLo(lhs: TxN, rhs: TxN) -> TxN`
    Interleaves the lower half of `lhs` and `rhs` lane-by-lane. This requires an even lane count.
  * `@simdInterleaveHi(lhs: TxN, rhs: TxN) -> TxN`
    Interleaves the upper half of `lhs` and `rhs` lane-by-lane. This requires an even lane count.
  * `@simdZipLo(lhs: TxN, rhs: TxN) -> TxN`
    Alias for `@simdInterleaveLo`.
  * `@simdZipHi(lhs: TxN, rhs: TxN) -> TxN`
    Alias for `@simdInterleaveHi`.
  * `@simdConcatLo(lhs: TxN, rhs: TxN) -> TxN`
    Concatenates the lower half of `lhs` with the lower half of `rhs`. This requires an even lane count.
  * `@simdConcatHi(lhs: TxN, rhs: TxN) -> TxN`
    Concatenates the upper half of `lhs` with the upper half of `rhs`. This requires an even lane count.
  * `@simdDeinterleaveLo(lhs: TxN, rhs: TxN) -> TxN`
    Collects even-numbered lanes from `lhs`, then even-numbered lanes from `rhs`. This requires an even lane count.
  * `@simdDeinterleaveHi(lhs: TxN, rhs: TxN) -> TxN`
    Collects odd-numbered lanes from `lhs`, then odd-numbered lanes from `rhs`. This requires an even lane count.
  * `@simdUnzipLo(lhs: TxN, rhs: TxN) -> TxN`
    Alias for `@simdDeinterleaveLo`.
  * `@simdUnzipHi(lhs: TxN, rhs: TxN) -> TxN`
    Alias for `@simdDeinterleaveHi`.
  * `@simdLowHalf[TxM](value: TxN) -> TxM`
    Extracts the lower half of a vector. `N` must be exactly `2 * M`, and the lane element type must stay the same.
  * `@simdHighHalf[TxM](value: TxN) -> TxM`
    Extracts the upper half of a vector. `N` must be exactly `2 * M`, and the lane element type must stay the same.
  * `@simdWithLowHalf[TxN](base: TxN, half: TxM) -> TxN`
    Replaces the lower half of `base` with `half`. `N` must be exactly `2 * M`, and the lane element type must stay the same.
  * `@simdWithHighHalf[TxN](base: TxN, half: TxM) -> TxN`
    Replaces the upper half of `base` with `half`. `N` must be exactly `2 * M`, and the lane element type must stay the same.
  * `@simdReduceAdd(value: TxN) -> T`
    Horizontally adds all lanes and returns the scalar result.
  * `@simdReduceMul(value: TxN) -> T`
    Horizontally multiplies all lanes and returns the scalar result.
  * `@simdReduceAnd(value: IxN) -> I`
    Bitwise-AND reduction for integer or mask vectors.
  * `@simdReduceOr(value: IxN) -> I`
    Bitwise-OR reduction for integer or mask vectors.
  * `@simdReduceXor(value: IxN) -> I`
    Bitwise-XOR reduction for integer or mask vectors.
  * `@simdReduceMin(value: TxN) -> T`
    Returns the minimum lane for integer or floating-point vectors.
  * `@simdReduceMax(value: TxN) -> T`
    Returns the maximum lane for integer or floating-point vectors.
  * `@simdAbs(value: TxN) -> TxN`
    Lane-wise absolute value for signed integer or floating-point vectors. Signed integer lanes use two's-complement wrapping semantics, so the most-negative lane stays unchanged.
  * `@simdMin(lhs: TxN, rhs: TxN) -> TxN`
    Lane-wise minimum for integer or floating-point vectors. Each lane compares `lhs.[i]` with `rhs.[i]`; if `lhs.[i] < rhs.[i]`, the result lane is `lhs.[i]`, otherwise it is `rhs.[i]`.
  * `@simdMax(lhs: TxN, rhs: TxN) -> TxN`
    Lane-wise maximum for integer or floating-point vectors. Each lane compares `lhs.[i]` with `rhs.[i]`; if `lhs.[i] > rhs.[i]`, the result lane is `lhs.[i]`, otherwise it is `rhs.[i]`.
  * `@simdClamp(value: TxN, lo: TxN, hi: TxN) -> TxN`
    Lane-wise clamp for integer or floating-point vectors. Semantically this is `@simdMin(@simdMax(value, lo), hi)` on each lane.
  * `@simdSqrt(value: FxN) -> FxN`
    Lane-wise square root for floating-point vectors.
  * `@simdFloor(value: FxN) -> FxN`
    Lane-wise floor for floating-point vectors.
  * `@simdCeil(value: FxN) -> FxN`
    Lane-wise ceil for floating-point vectors.
  * `@simdTrunc(value: FxN) -> FxN`
    Lane-wise truncation toward zero for floating-point vectors.
  * `@simdRound(value: FxN) -> FxN`
    Lane-wise rounding to the nearest integral value, with halfway cases rounded away from zero.
  * `@simdSplat[TxN](value: T) -> TxN`
    Replicates one scalar lane value into every lane of the result vector.
  * `@simdCast[UxN](value: TxN) -> UxN`
    Performs lane-wise numeric conversion. The source and result vectors must have the same lane count. Source lanes may be integer, floating-point, or `bool`; result lanes may be integer or floating-point.
  * `@simdBitcast[UxM](value: TxN) -> UxM`
    Reinterprets the vector bits without changing them. The source and result vectors must have the same total size in bytes.
  * `@simdLoad[TxN](ptr: &T, align: usize) -> TxN`
    Loads a vector from contiguous scalar memory. `align` must be a compile-time non-zero power of two and is an explicit alignment promise made by the source program.
  * `@simdStore[TxN](ptr: &mut T, value: TxN, align: usize) void`
    Stores a vector to contiguous scalar memory. `align` follows the same rule and promise model as `@simdLoad`.
  * `@simdMaskedLoad[TxN](ptr: &T, mask: boolxN, or_else: TxN, align: usize) -> TxN`
    For lane `i`, loads from `ptr[i]` when `mask.[i]` is `true`, otherwise yields `or_else.[i]`. Masked-off lanes do not access memory.
  * `@simdMaskedStore[TxN](ptr: &mut T, mask: boolxN, value: TxN, align: usize) void`
    For lane `i`, stores `value.[i]` to `ptr[i]` only when `mask.[i]` is `true`. Masked-off lanes do not access memory.
  * `@simdGather[TxN](ptr: &T, indices: &usize) -> TxN`
    Loads lane `i` from `ptr[indices[i]]`. The `indices` pointer must reference at least `N` `usize` elements. Both pointers obey Kern's ordinary raw-pointer validity and alignment rules.
  * `@simdScatter[TxN](ptr: &mut T, indices: &usize, value: TxN) void`
    Stores lane `i` to `ptr[indices[i]]`. Scatter applies stores from lane `0` through lane `N - 1`, so duplicate indices are allowed and later lanes overwrite earlier lanes.
  * `@simdMaskedGather[TxN](ptr: &T, indices: &usize, mask: boolxN, or_else: TxN) -> TxN`
    For lane `i`, loads from `ptr[indices[i]]` when `mask.[i]` is `true`, otherwise yields `or_else.[i]`. Masked-off lanes do not access either `indices[i]` or `ptr[indices[i]]`.
  * `@simdMaskedScatter[TxN](ptr: &mut T, indices: &usize, mask: boolxN, value: TxN) void`
    For lane `i`, stores to `ptr[indices[i]]` only when `mask.[i]` is `true`. Scatter still applies active stores in lane order `0` through `N - 1`.

These are value intrinsics, not control-flow forms. Their operands are all evaluated normally before the intrinsic is applied.

The rearrangement helpers above are specified purely in terms of lane order. They lower to fixed `@simdShuffle` masks rather than backend-specific bespoke nodes.

```kern
let a = f32x4.{ 1.0, 2.0, 3.0, 4.0 };
let b = f32x4.{ 5.0, 1.0, 3.0, 0.0 };
let mask = a < b; // boolx4
let mags = @simdAbs(f32x4.{ -1.0, 2.0, -0.0, -4.0 });
let rev = @simdReverse(a);
let rot = @simdRotateLeft(a, 1);
let inter = @simdInterleaveLo(a, b);
let cat = @simdConcatLo(a, b);
let de = @simdDeinterleaveLo(inter, @simdInterleaveHi(a, b));
let pair_min = @simdMin(i32x4.{ 9, 2, -4, 8 }, i32x4.{ 3, 7, -5, 8 });
let pair_max = @simdMax(a, b);
let clipped = @simdClamp(a, f32x4.{ 0.0, 1.5, 1.0, 0.0 }, f32x4.{ 3.0, 3.0, 3.0, 3.0 });
let roots = @simdSqrt(f32x4.{ 1.0, 4.0, 9.0, 16.0 });
let lowered = @simdFloor(f32x4.{ 1.9, -1.2, 2.0, -0.0 });
let raised = @simdCeil(f32x4.{ 1.1, -1.8, 2.0, -0.0 });
let chopped = @simdTrunc(f32x4.{ 1.9, -1.8, 2.0, -0.0 });
let ones = @simdSplat[i32x4](1);
let as_float = @simdCast[f32x4](ones);
let bits = @simdBitcast[u32x4](as_float);

if (@simdAny(mask)) {
    let mixed = @simdSelect(mask, a, b);
    let last = mixed.[3];
}

let data = [8]f32.{ 1.0, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0 };
let picks = [4]usize.{ 7, 0, 5, 2 };
let left = @simdLoad[f32x4](data.[0]..&, 4);
let right = @simdLoad[f32x4](data.[4]..&, 4);
let mixed = @simdShuffle(left, right, [4]u32.{ 0, 5, 2, 7 });
let swizzled = @simdSwizzle(left, [4]u32.{ 3, 0, 2, 1 });
let total = @simdReduceAdd(mixed);
@simdStore(data.[0]..&, mixed, 4);
let partial = @simdMaskedLoad[f32x4](data.[0]..&, boolx4.{ true, false, true, false }, f32x4.{ 0.0, 0.0, 0.0, 0.0 }, 4);
@simdMaskedStore(data.[0]..&, boolx4.{ true, false, true, false }, partial, 4);
let gathered = @simdGather[f32x4](data.[0]..&, picks.[0].&);
@simdScatter(data.[0]..&, picks.[0].&, gathered);
let masked_gather = @simdMaskedGather[f32x4](data.[0]..&, picks.[0].&, boolx4.{ true, false, true, false }, f32x4.{ -1.0, -1.0, -1.0, -1.0 });
@simdMaskedScatter(data.[0]..&, picks.[0].&, boolx4.{ true, false, true, false }, masked_gather);
let halves = @simdLowHalf[f32x2](swizzled);
let restored = @simdWithHighHalf[f32x4](mixed, halves);
```

Like `@sizeOf` and `@trap`, these intrinsics are compiler-owned language mechanisms and remain available in freestanding code.

The existing bit intrinsics also extend lane-wise to SIMD integer vectors:

  * `@popCount(IxN) -> IxN`
  * `@clz(IxN) -> IxN`
  * `@ctz(IxN) -> IxN`
  * `@bswap(IxN) -> IxN`

Each lane is processed independently, and the result has the same SIMD type as the operand.

For floating-point `@simdMin` and `@simdMax`, Kern uses the ordered comparisons above directly. This means unordered lanes such as `NaN` fall through to the `rhs` lane.

`@simdClamp` inherits the same ordered-comparison rule because it is defined in terms of `@simdMax` followed by `@simdMin`.

`@simdSqrt`, `@simdFloor`, `@simdCeil`, `@simdTrunc`, and `@simdRound` are floating-point-only. They are lane-wise value operations and do not imply any control-flow or mask semantics.
