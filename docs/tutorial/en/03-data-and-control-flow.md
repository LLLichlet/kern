# 03. Data And Control Flow

English | [简体中文](../zh/03-数据与控制流.md)

## Structs

```kern
struct Point {
    x: i32,
    y: i32,
};

fn translate(point: Point, dx: i32, dy: i32) Point {
    return Point.{
        x: point.x + dx,
        y: point.y + dy,
    };
}
```

Construct a struct with the full type name:

```kern
let p = Point.{ x: 1, y: 2 };
```

When the target type is provided by context, such as a return type or argument
type, the type name can be omitted:

```kern
return .{ x: point.x + dx, y: point.y + dy };
```

This works because `translate` already declares `Point` as its return type.
Kern reads `.{ ... }` as an initializer body whose target type comes from the
surrounding context. Prefer the full `Point.{ ... }` form while learning, and
use the elided form only when the target type is close and obvious.

Normal `struct` layout may be reordered by the compiler for compactness. Use
`extern struct` when you need C ABI layout, hardware register descriptions, or
source-order field layout:

```kern
extern struct Header {
    tag: u8,
    value: u64,
};
```

## Enums And `match`

Kern uses `enum` for mutually exclusive states. A simple enum can contain only
names:

```kern
enum Color: u8 {
    Red = 0,
    Green = 1,
    Blue = 2,
};
```

For this kind of enum, `Color: u8` sets the integer backing type. Explicit
discriminants such as `Red = 0` choose concrete values; omitted values increase
in order. Without a backing type, simple enums default to `u32`.

Enums can also carry data:

```kern
enum ParseResult {
    Number: i32,
    Missing,
};
```

Construct enum values with the full form:

```kern
let ok = ParseResult.{ Number: 42 };
let missing = ParseResult.Missing;
```

Or omit the type when context supplies it:

```kern
fn parse_value() ParseResult {
    return .{ Number: 42 };
}
```

`match` chooses a branch based on the shape of a value:

```kern
fn code_score(code: u8) i32 {
    return match code {
        0 => 10,
        1 => 20,
        _ => -1,
    };
}
```

`_` means "all other cases." Each arm has a pattern on the left and an
expression on the right:

```kern
match value {
    pattern => expr,
    pattern => expr,
}
```

Arms can contain multiple patterns and range patterns:

```kern
fn class_of(byte: u8) i32 {
    return match byte {
        b'0' ..= b'9' => 1,
        b'a' ..= b'z', b'A' ..= b'Z' => 2,
        b'_', b'-' => 3,
        _ => 0,
    };
}
```

`a ... b` is a left-closed, right-open range. `a ..= b` includes the right
endpoint. Comma-separated patterns on one arm share the same result expression.

Compiler-known value patterns include scalar literals, enum variants,
structural data patterns, and closed scalar ranges. User-defined matching logic
is expressed by values that implement `Pattern[T]`; `Eq[...]` only enables
`==`, not `match` arms:

```kern
struct IsCommand { name: &[u8] };

impl IsCommand : Pattern[&[u8]] {
    type Bind = void;

    fn apply(value: &[u8]) ?Bind {
        if value == self.name {
            return .{ Some: {} };
        }
        return .None;
    }
}

fn command_id(command: &[u8]) i32 {
    return match command {
        IsCommand.{ name: "run" } => 1,
        IsCommand.{ name: "test" } => 2,
        _ => 0,
    };
}
```

For open-ended value domains, keep a `_` arm unless every possible value is
covered by structural or scalar patterns.

`Pattern[T]` can also bind data for the arm body. The binding shape is the
associated `Bind` type. `void` means no bindings; `struct { ... }` means each
field becomes an arm-local name:

```kern
struct Prefix {
    byte: u8,
};

impl Prefix : Pattern[&[u8]] {
    type Bind = struct { rest: &[u8] };

    fn apply(value: &[u8]) ?Bind {
        if value.@len() == 0 or value.[0] != self.byte {
            return .None;
        }
        return .{ Some: .{ rest: value.&[1...] } };
    }
}

fn after_dash(text: &[u8]) ?&[u8] {
    return match text {
        Prefix.{ byte: b'-' } => .{ Some: rest },
        _ => .None,
    };
}
```

When one arm lists several patterns, all of them must produce the same binding
shape: the same field names, field types, and binding mutability. This is why
`1, 2, 3 => ...` is valid: each pattern has `Bind = void`. For binding
patterns, keep the names aligned:

```kern
match value {
    .{ A: item }, .{ B: item } => use(item),
    // .{ A: left }, .{ B: right } => ...   // different bind names
}
```

For enums, patterns can test the variant and unpack payloads:

```kern
fn describe(result: ParseResult) void {
    match result {
        .{ Number: value } => "number = {}".fmt(.{value}).println(),
        .Missing => "missing".println(),
    }
}
```

`.{ Number: value }` means: if the value is `Number`, bind its `i32` payload to
the local name `value`. Bindings may be mutable:

```kern
ParseResult.{ Number: mut value } => {
    value += 1;
    "next = {}".fmt(.{value}).println();
}
```

`match` is exhaustive. If an enum later gains a new variant, the compiler can
point out every branch that still needs to handle it.

Patterns can nest:

```kern
enum Bit {
    Zero,
    One,
};

enum Leaf {
    Empty,
    Full: Bit,
};

fn leaf_score(leaf: Leaf) i32 {
    return match leaf {
        .Empty => 0,
        .{ Full: .Zero } => 1,
        .{ Full: .One } => 2,
    };
}
```

The omitted type names are recovered from the surrounding `match` target.

## Option: `?T`

`?T` is the builtin enum family for "value or no value." It is similar in shape
to:

```kern
enum Option[T] {
    Some: T,
    None,
};
```

But `?T` is a direct language type form, like `i32`, `&T`, or `[N]T`. It is not
a nullable pointer and does not have hidden ABI privileges.

```kern
fn first_digit(text: &[u8]) ?u8 {
    let mut i = 0;
    while i < text.@len() {
        let byte = text.[i];
        if byte >= b'0' and byte <= b'9' {
            return .{ Some: byte };
        }
        i += 1;
    }
    return .None;
}
```

Use `match` to unpack it:

```kern
match first_digit("abc3") {
    .{ Some: byte } => "digit = {}".fmt(.{byte}).println(),
    .None => "no digit".println(),
}
```

If you only care whether a value exists, use `is_some()` / `is_none()`:

```kern
if first_digit("abc3").is_some() {
    "found a digit".println();
}
```

Use `map` to transform the payload while preserving `None`:

```kern
let digit_value = first_digit("abc3").map([](byte: u8) u8 {
    return byte - b'0';
});
```

Use `ok_or` to turn absence into a typed error:

```kern
enum ParseError {
    MissingDigit,
};

let digit = first_digit("abc").ok_or(ParseError.MissingDigit);
```

`digit` has type `u8!ParseError`.

## Result: `T!E`

`T!E` means "successful `T` or error `E`." Kern has no exceptions; failure is a
plain value and ordinary control flow.

```kern
enum Error {
    Empty,
};

fn first(text: &[u8]) u8!Error {
    if text.@len() == 0 {
        return .{ Err: .Empty };
    }
    return .{ Ok: text.[0] };
}
```

You can handle results with `match`:

```kern
match first("kern") {
    .{ Ok: byte } => "first = {}".fmt(.{byte}).println(),
    .{ Err: .Empty } => "error: empty input".println(),
}
```

Use `match` when the branches contain real work. Use shorter forms when the
flow is simply "continue on success, return on failure."

`let else` expresses "this one pattern may continue; every other case must
leave this path":

```kern
let .{ Ok: byte } = first(text) else return .{ Err: .Empty };
```

If the expression is `Ok`, the payload is bound as `byte`. Otherwise, the
`else` expression runs; here it returns from the current function.

When failures need separate handling, use a failure-arm block:

```kern
fn first_or_error(text: &[u8]) u8!Error {
    let .{ Ok: byte } = first(text)
        else {
            .{ Err: err } => return .{ Err: err },
        };

    return .{ Ok: byte };
}
```

`.?` is the direct propagation operator. On `T!E`, it extracts `Ok` in a
function that also returns `T!E`; if the value is `Err`, it returns that error
from the current function:

```kern
fn first_or_error(text: &[u8]) u8!Error {
    let byte = first(text).?;
    return .{ Ok: byte };
}
```

On `?T`, the same operator extracts `Some`, or returns `None` from the current
function.

```kern
fn first_digit_plus_one(text: &[u8]) ?u8 {
    let digit = first_digit(text).?;
    return .{ Some: digit + 1 };
}
```

## Anonymous Aggregates

Kern supports anonymous structs, unions, and enums. They are useful for local
data organization, structural boundaries, and lightweight duck typing:

```kern
fn sum_pair(pair: struct { y: i32, x: i32 }) i32 {
    return pair.x + pair.y;
}

fn read_word(word: union { bytes: [4]u8, int: i32 }) i32 {
    return word.int;
}
```

Anonymous structs are structural types. At explicit boundaries, a named struct
can naturally convert to a compatible anonymous struct:

```kern
use std.io;

struct Pair {
    x: i32,
    y: i32,
};

fn sum_pair(pair: struct { y: i32, x: i32 }) i32 {
    return pair.x + pair.y;
}

fn main() i32 {
    let pair = Pair.{ x: 4, y: 5 };
    let total = sum_pair(pair);
    "total = {}".fmt(.{total}).println();
    return 0;
}
```

Named types are better for domain concepts and public API identity. Anonymous
structs are better for local boundaries that mean "I only need these fields."

Anonymous unions express multiple views over shared storage:

```kern
let word = union { bytes: [4]u8, int: i32 }.{ int: 11 };
let value = read_word(word);
```

Anonymous enums express local state sets:

```kern
fn classify(state: enum: u32 { Off = 0, On = 1, Error: i32 }) i32 {
    return match state {
        .Off => 0,
        .On => 1,
        .{ Error: code } => code,
    };
}
```

For fixed C ABI layout, anonymous structs can also be written as
`extern struct { ... }`. See
[`examples/anonymous_aggregates.kn`](../../../examples/anonymous_aggregates.kn)
for a fuller layout example.
