---
title: "Option And Result Helpers"
summary: "Use `?T` and `T!E` helper methods such as `and_then`, `ok_or`, `map`, and `map_or` to keep fallible code explicit without expanding every step into a full `match`."
order: 8
---

The earlier enum chapter covered the shape of Kern's builtin carrier families:

- `?T`
- `T!E`

But day-to-day code also relies on their helper methods.

Those helpers do not turn Kern into an exception language.
They are still explicit value transformations over ordinary carriers.

## A Validated Example

The following package was built and run successfully in a temporary directory
while writing this guide:

```kern
use std.io;

fn parse_digit(byte: u8) ?i32 {
    if (byte >= b'0' and byte <= b'9') {
        return .{ Some: (byte - b'0') as i32 };
    }
    return .None;
}

fn pair_value(text: []u8) ?i32 {
    let first = text.first().and_then(parse_digit).?;
    let second = text.[1 .. #text].first().and_then(parse_digit).?;
    return .{ Some: first * 10 + second };
}

fn pair_result(text: []u8) i32![]u8 {
    let value = pair_value(text).ok_or("need two digits").!;
    return .{ Ok: value };
}

fn main() i32 {
    let maybe = parse_digit(b'7').map_or(i32.{-1}, .[](value: i32) i32 {
        return value + 1;
    });

    let value = match (pair_result("42").map(.[](value: i32) i32 {
        return value + 1;
    })) {
        .{ Ok: ok } => ok,
        .{ Err: _ } => return 1,
    };

    let err_len = match (pair_result("4x")) {
        .{ Ok: _ } => return 2,
        .{ Err: msg } => (#msg) as i32,
    };

    io.println("maybe={} value={} err_len={}", .{
        maybe,
        value,
        err_len,
    });

    return if (maybe == 8 and value == 43 and err_len == 15) 0 else 3;
}
```

The validated run printed:

```text
maybe=8 value=43 err_len=15
```

## What This Shows

### `and_then` Chains Another Carrier-Producing Step

This line:

```kern
text.first().and_then(parse_digit)
```

starts from `text.first(): ?u8` and then calls `parse_digit: fn(u8) ?i32`.

That is what `and_then` is for:

- if the carrier is empty or failed, stop there
- otherwise run the next carrier-producing step

No hidden control flow is introduced.
The program is still just transforming carrier values.

### `.?` And `.!` Still Mean Propagation

These lines:

```kern
...and_then(parse_digit).?;
...ok_or("need two digits").!;
```

show the intended split:

- helper methods shape the carrier
- `.?` and `.!` propagate the current failure state

That keeps the code compact without hiding the fact that control flow may exit
early.

### `ok_or` Converts Optional Data Into Result Data

This line:

```kern
pair_value(text).ok_or("need two digits")
```

turns:

- `?i32`

into:

- `i32![]u8`

That is often the right boundary when:

- an earlier stage only knows "present or absent"
- a later stage needs a real typed error payload

### `map_or` Handles Fallback And Success In One Expression

This line:

```kern
parse_digit(b'7').map_or(i32.{-1}, ...)
```

means:

- return the mapped value when `Some`
- otherwise use the provided fallback

That is useful when you want one scalar output instead of another carrier.

### `map` Transforms Success While Keeping The Error Type

This line:

```kern
pair_result("42").map(.[](value: i32) i32 { ... })
```

changes the success payload from `42` to `43` without rewriting the error side.

That is the purpose of `map` on `T!E`:

- keep `Err(E)` unchanged
- transform only `Ok(T)`

## When To Reach For Full `match`

Helper methods are useful, but they are not mandatory style rules.

Use a full `match` when:

- branch behavior is no longer symmetric
- you need distinct side effects on each arm
- the helper chain is becoming harder to read than two explicit branches

Kern's design does not treat helper chains as morally better than `match`.
They are just another explicit tool.

## Practical Takeaway

Use these helpers for their specific jobs:

- `and_then` for chaining another carrier-producing step
- `ok_or` when optional data must become a result with a typed error
- `map_or` when you want one plain output with a fallback
- `map` when you only want to transform the success payload

Then use `.?` and `.!` when you intentionally want early propagation from the
current carrier.
