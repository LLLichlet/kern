# 10. Attributes, Intrinsics, And Low-Level Operations

English | [简体中文](../zh/10-属性intrinsic与低层操作.md)

Kern has no C-preprocessor-style macro system. Code that needs to affect
compilation, layout, linking, code generation, or target-machine operations
usually uses three forms:

- `#[...]` / `#![...]`: attributes attached to syntax nodes or lexical scopes.
- `@...`: compiler intrinsics implemented by the compiler.
- `^T` / `^mut T`: address / volatile pointers for MMIO expressed through ordinary dereference syntax.

This chapter is not a complete index. It teaches the common low-level forms you
will see in source code.

## Outer And Inner Attributes

Outer attributes apply to the next declaration:

```kern
#[export_name("_start")]
fn kmain() void {
    while true {}
    @unreachable();
}
```

Inner attributes apply to the current lexical scope, often at the start of a
file or module:

```kern
#![if os == "linux"]
```

Attribute content is either a conditional-compilation expression or metadata
tags. Do not mix both categories in one `#[...]`.

## Conditional Compilation

Conditional attributes prune code before semantic analysis:

```kern
#[if os == "linux"]
mod linux;

#[if os == "windows"]
mod windows;
```

Conditions use Kern boolean syntax: `and`, `or`, and `!`. They can also read
compiler configuration definitions:

```kern
#[if os == "linux" or os == "darwin" and !libc]
mod posix_no_libc;
```

The standard library's hosted OS shims use this pattern heavily. Pruned code
does not participate in later semantic checking.

## Common Metadata

Linking and FFI:

- `export_name("...")`: set the exported symbol name.
- `link_section("...")`: place a function or static in a specific section.
- `retain`: keep an item even when no Kern code directly references it.

Layout:

- `packed`: remove field padding, with possible unaligned-access costs.
- `align(N)`: set type or static-data alignment.

Optimization and code generation:

- `inline` / `noinline`: request or forbid inlining.
- `cold`: mark a cold path.
- `naked`: omit function prologue/epilogue, usually for very low-level entry or interrupt glue.
- `target_feature("...")`: attach CPU feature requirements, such as `#[target_feature("avx2,fma")]`.

Attributes are compiler-understood metadata, not runtime objects. Attribute
arguments that must be compile-time constants are validated by the frontend.

## Type Information Intrinsics

These intrinsics evaluate at compile time:

```kern
let size = @sizeOf[Pair]();
let align = @alignOf[Pair]();
let same_type_size = @sizeOf[@typeOf(pair)]();
```

- `@sizeOf[T]()`: size of type `T`, in bytes.
- `@alignOf[T]()`: ABI alignment of type `T`, in bytes.
- `@typeOf(expr)`: the exact compile-time type of an expression.

`@typeOf` matters especially for anonymous structs and closures, whose types do
not have source names.

## `@trap`, `@unreachable`, And Breakpoints

Common execution-control intrinsics:

```kern
@trap();
@unreachable();
@breakpoint();
```

- `@trap()` deliberately triggers a trap; use it for unrecoverable errors, test failures, or branches with no recovery path.
- `@unreachable()` promises the compiler that this path is unreachable, often after control has already transferred away.
- `@breakpoint()` triggers a debugger breakpoint.

`@trap()` means "stop if execution reaches here." `@unreachable()` is a promise
to optimization and code generation. Only write it when the path really cannot
continue.

```kern
fn exit_now(code: i32) ! {
    raw_exit(code);
    @unreachable();
}

fn expect_nonzero(value: i32) i32 {
    if value == 0 {
        @trap();
    }
    return value;
}
```

It is normal for source code to use `@trap()` more often than `@unreachable()`:
many error paths really mean "terminate now," not "this point is physically
impossible."

## Bit And Memory Intrinsics

Integer bit operations:

```kern
let bits = @popCount[u8](0b1011);
let swapped = @bswap[u16](0x1234);
let leading = @clz[u32](1);
let trailing = @ctz[u32](8);
```

These also apply lane-wise to integer SIMD vectors.

Bulk memory operations:

```kern
@memcpy(dest, src, len);
@memmove(dest, src, len);
@memset(dest, 0, len);
```

These map directly to backend capabilities. The caller is responsible for
pointer validity, length, overlap rules, and lifetimes; Kern does not insert
hidden checks here.

## `^T` And Volatile Pointers

Kern models MMIO and fixed-address access with explicit pointer types:

- `^T`: read-only address / volatile pointer.
- `^mut T`: writable address / volatile pointer.

They are still ordinary values: they can be stored, passed, compared, converted
to integer addresses, and converted back. Their special property is that `.*`
deref emits volatile load/store.

```kern
const UART_DR = 0x1000_0000usize;

fn read_data() u32 {
    let reg = UART_DR as ^u32;
    return reg.*;
}

fn write_data(value: u32) void {
    let reg = UART_DR as ^mut u32;
    reg.* = value;
}
```

Volatility is part of the pointer family, and access still uses ordinary
dereference syntax.

`&T` / `&mut T` are for ordinary object memory. `^T` / `^mut T` are for device
registers and fixed addresses:

```kern
let obj = addr as &mut u32;   // ordinary object memory
let reg = addr as ^mut u32;   // volatile address access
```

Atomics are for ordinary shared memory, not MMIO. Device registers should use
`^T` / `^mut T` and ordinary dereference.

## Atomics

Atomic operations are compiler intrinsics. The `base.sync` library wraps them
with a better everyday API:

```kern
use base.sync.{ACQUIRE, RELEASE, SEQ_CST, atomic};

let mut counter = atomic[usize](0);
counter..&.store[RELEASE](1);
let current = counter.&.load[ACQUIRE]();
```

Low-level intrinsics require explicit memory ordering:

```kern
let mut raw_counter = 1usize;
let value = @atomicLoad[usize](raw_counter.&, SEQ_CST);
```

Ordering is a compile-time constant. Prefer the typed `base.sync` wrappers for
ordinary code. `base.sync.MemOrder` is an `extern enum: u8`, so its values can
cross directly into raw intrinsic ordering operands.

Common intrinsic shapes:

```kern
@atomicLoad[T](ptr, order);
@atomicStore[T](ptr, value, order);
@atomicXchg[T](ptr, value, order);
@atomicCas[T](ptr, expected, desired, success_order, failure_order);
@atomicRmwAdd[T](ptr, value, order);
@fence(order);
```

Atomic synchronization is for small scalar values and ordinary thin pointers in
shared memory. Floating point, `^T` / `^mut T`, slices, trait objects, and
closure fat pointers are not this low-level atomic payload family.

## SIMD Builtin Values

Kern has builtin SIMD types such as `i32x4`, `f32x4`, `u8x16`, and `boolx4`.
They are not aliases for `[N]T`; the lane count is part of the type spelling.

```kern
let a = i32x4.{ 1, 2, 3, 4 };
let b = i32x4.{ 4, 3, 2, 1 };

let sum = a + b;
let mask = a < b;
```

Arithmetic, bitwise operations, and comparisons are lane-wise. Comparison
results are `boolxN`, which cannot be used directly as scalar `if` conditions.
Reduce them explicitly:

```kern
if @simdAny(mask) {
    @breakpoint();
}
```

Lane access uses `.[]`, similar to arrays, but SIMD values do not participate
in slice semantics:

```kern
let mut v = f32x4.{ 1.0, 2.0, 3.0, 4.0 };
let second = v.[1];
v.[2] = 9.0;
```

For the current fixed-width model, lane indexes must be compile-time constants
and in range.

## A Common SIMD Workflow

Typical SIMD code loads contiguous memory into a vector, computes lane-wise,
then reduces or stores back:

```kern
fn sum4(ptr: &f32) f32 {
    let values = @simdLoad[f32x4](ptr, 4);
    return @simdReduceAdd(values);
}

fn add4(ptr: &mut f32, delta: f32) void {
    let values = @simdLoad[f32x4](ptr, 4);
    let out = values + @simdSplat[f32x4](delta);
    @simdStore(ptr, out, 4);
}
```

The alignment argument to `@simdLoad` / `@simdStore` is an explicit promise. It
must be a compile-time non-zero power of two; it is not a runtime check.

Masks are central to everyday SIMD. This scans a 16-byte chunk for the first
non-space byte:

```kern
fn first_non_space(chunk: &u8) usize {
    let bytes = @simdLoad[u8x16](chunk, 1);
    let spaces =
        (bytes == @simdSplat[u8x16](b' ')) |
        (bytes == @simdSplat[u8x16](b'\n')) |
        (bytes == @simdSplat[u8x16](b'\r')) |
        (bytes == @simdSplat[u8x16](b'\t'));

    let non_spaces = @simdBitmask(!spaces);
    if non_spaces == 0 {
        return 16usize;
    }
    return @ctz(non_spaces);
}
```

Important pieces:

- `|` and `&` combine `boolxN` masks lane-wise.
- `!mask` flips a mask lane-wise.
- `@simdBitmask` packs a `boolxN` into a `usize` bitset; lane `i` becomes bit `i`.
- `@ctz` finds the first set bit.

Selection and rearrangement do not require inline assembly:

```kern
let lo = f32x4.{ 1.0, 2.0, 3.0, 4.0 };
let hi = f32x4.{ 10.0, 20.0, 30.0, 40.0 };
let mask = boolx4.{ true, false, true, false };

let picked = @simdSelect(mask, lo, hi);
let mixed = @simdShuffle(lo, hi, [4]u32.{ 0, 5, 2, 7 });
let reversed = @simdReverse(lo);
```

`@simdSelect` picks lanes by mask. `@simdShuffle` selects from the concatenated
view `lhs ++ rhs`: `0` is `lhs.[0]`, `4` is `rhs.[0]`.

Use gather/scatter for non-contiguous memory:

```kern
let indices = [4]usize.{ 7, 0, 5, 2 };
let values = @simdGather[f32x4](base, indices.[0].&);
@simdScatter(out, indices.[0].&, values);
```

Masked load/store/gather/scatter do not access memory for masked-off lanes,
which is useful for tails and sparse selection.

## `@asm`

Inline assembly uses structured parameters instead of hidden string placeholder
bindings:

```kern
fn outb(port: u16, value: u8) void {
    @asm(.{
        asm: "out dx, al",
        inputs: .{
            dx: port,
            al: value,
        },
        volatile: true
    });
}
```

Outputs bind registers to writable pointers:

```kern
fn syscall1_raw(sys_num: usize, arg1: usize) isize {
    let mut ret: isize = undef;

    @asm(.{
        asm: "syscall",
        outputs: .{ rax: ret..& },
        inputs: .{
            rax: sys_num,
            rdi: arg1,
        },
        clobbers: .{ "rcx", "r11", "memory" },
        volatile: true
    });

    return ret;
}
```

Use Kern multiline strings for multiline assembly. The `asm` field itself must
still be one string literal:

```kern
@asm(.{
    asm:
        \\nop
        \\nop
    ,
    volatile: true,
});
```

`asm`, `inputs`, `outputs`, `clobbers`, and `volatile` are compiler-consumed
metadata, not ordinary runtime structs. The template, clobber list, and
`volatile` flag must be known at compile time.

Use `@asm` for CPU instructions, special registers, system-call entry, and
startup glue. Prefer intrinsics for ordinary memory copying, atomics, SIMD,
byte order, and bit operations because the compiler can understand and
optimize them.

Continue with the attributes, inline assembly, and compiler intrinsics sections
of [`design.md`](../../design.md). The standard library files
[`library/std/host/os/linux.kn`](../../../library/std/host/os/linux.kn) and
[`library/base/sync/mod.kn`](../../../library/base/sync/mod.kn) are also useful
references.
