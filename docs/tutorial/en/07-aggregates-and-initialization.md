# 07. Aggregates And Initialization

English | [简体中文](../zh/07-聚合类型与初始化.md)

Earlier chapters already used structs, anonymous structs, arrays, enums, and
unions. This chapter gathers the initialization rules for these aggregate
types, especially the struct-default-field pattern that appears often in Kern.

## Structs Can Provide Default Fields

Struct fields can have default values:

```kern
struct Config {
    port: u16 = 8080,
    retries: u8 = 3,
    verbose: bool = false,
};
```

Fields without defaults must be supplied. Fields with defaults may be omitted:

```kern
let default = Config.{};
let custom = Config.{ port: 9000 };
```

A field default is an expression, not just a literal. It is type-checked at the
struct declaration against the field type. When an initializer omits that field,
the compiler uses the default expression at that construction site.

```kern
struct Limits {
    max_open: usize = {
        static fallback = usize.{256};
        fallback
    },
};

let limits = Limits.{};
```

The expression follows ordinary Kern expression rules: it may be a block, and
the block may contain a local `static`. Do not read this as "compute once at
declaration time and cache into the field." Each construction site that omits
the field uses the default expression. If the default expression declares a
`static`, sharing comes from that explicit `static`, not from the field-default
mechanism itself.

This makes structs useful as default-argument providers. Kern does not need a
separate function-default-argument syntax for many APIs:

```kern
fn connect(options: Config) i32 {
    if (options.verbose) {
        return options.port as i32 + options.retries as i32;
    }
    return options.port as i32;
}

let code = connect(.{ port: 9000 });
```

Here `.{ port: 9000 }` is contextual initialization: `connect` already declares
the parameter type as `Config`. While learning, prefer the full
`Config.{ ... }` form until context is obvious.

## Field Puns

In typed struct initialization, if a local binding name matches a field name,
you can use a field pun:

```kern
struct Pair {
    x: i32,
    y: i32,
};

let x = i32.{4};
let y = i32.{5};

let pair = Pair.{ x, y };
```

This is equivalent to:

```kern
let pair = Pair.{ x: x, y: y };
```

Field puns are local shorthand. Across modules, API boundaries, or larger
field lists, `field: value` is often easier to read.

## `undef` Must Be Explicit

If a field has no default, omitting it is a compile error. If you intentionally
need uninitialized storage, write `undef` explicitly:

```kern
struct Packet {
    len: usize,
    data: [1500]u8,
};

let packet = Packet.{
    len: 0,
    data: [1500]u8.{undef},
};
```

This rule keeps "forgot to initialize a field" separate from "intentionally
left this storage uninitialized."

## Native Layout And `extern struct`

Normal Kern structs use native layout. The compiler may reorder physical fields
to reduce padding, so source field order is not an ABI promise.

```kern
struct NativeLayout {
    tag: u8,
    value: u64,
    flag: u16,
};
```

When a type crosses a C, hardware-table, boot-protocol, or assembly boundary,
use `extern struct`:

```kern
extern struct CLayout {
    tag: u8,
    value: u64,
    flag: u16,
};
```

This is an ABI contract, not a performance hint. Prefer native structs for
ordinary internal data; use `extern struct` for boundary data.

## Anonymous Structs

Anonymous structs are structural types, useful for lightweight grouping,
temporary return values, and closure state:

```kern
fn bounds() struct { start: usize, end: usize } {
    return .{ start: 2, end: 8 };
}
```

Anonymous structs are equivalent by field set and field type, not by a
declaration name. Named structs are nominal types even when fields match.

Anonymous structs can also be `extern` for inline C ABI boundaries:

```kern
extern {
    fn consume(header: &extern struct { tag: u8, len: u32 }) void;
}
```

## Union And Enum Initialization

Unions do not track an active field and do not have default fields. Choose a
field explicitly:

```kern
let word = union { bytes: [4]u8, int: i32 }.{ int: 11 };
```

Payload enum initialization explicitly chooses a variant:

```kern
let state = enum: u32 {
    Off = 0,
    On = 1,
    Error: i32,
}.{ Error: 13 };
```

Kern enums are strong independent types. External integers cannot be cast into
an enum with `as`. To turn hardware, file-format, or network-protocol integers
into enums, sanitize with `match`:

```kern
enum Color: u8 {
    Red = 0,
    Green,
    Blue,
};

fn color_from_byte(raw: u8) Color {
    return match (raw) {
        0 => Color.Red,
        1 => Color.Green,
        2 => Color.Blue,
        _ => Color.Red,
    };
}
```

## Type Information Intrinsics

Aggregate types often appear with type-information intrinsics:

```kern
let size = @sizeOf[CLayout]();
let align = @alignOf[CLayout]();
```

`@sizeOf[T]()` and `@alignOf[T]()` evaluate at compile time. They are common in
ABI checks, allocator layout, static assertions, and low-level memory
calculations.

Anonymous types such as closures also use `@typeOf(expr)`. Chapter 09 covers
closure state and `@typeOf` together.
