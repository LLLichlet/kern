---
title: "Testing And Runtime Messages"
summary: "Use `std.test` for executable assertions and `std.msg` for lightweight runtime diagnostics and fail-fast reporting."
order: 14
---

Kern keeps two related but different tools separate:

- `std.test` for assertion vocabulary
- `std.msg` for runtime-facing diagnostic output and fail-fast helpers

That split is intentional.
Tests are not the same thing as logging, and ad hoc diagnostics are not the
same thing as typed error handling.

## A Validated Example

The following package was built and run successfully in a temporary directory
while writing this guide:

```kern
use std.msg;
use std.test;

fn maybe(flag: bool) ?i32 {
    return if (flag) ?i32.{ Some: 7 } else ?i32.None;
}

fn parse(flag: bool) i32![]u8 {
    return if (flag) i32![]u8.{ Ok: 9 } else i32![]u8.{ Err: "bad" };
}

fn main() i32 {
    msg.log("boot {}", .{ 1, });
    msg.debug("trace {}", .{ "ok", });

    test.eq(test.expect_some(maybe(true)), i32.{7});
    test.expect_none(maybe(false));
    test.eq(test.expect_ok(parse(true)), i32.{9});
    test.eq(test.expect_err(parse(false)), "bad");
    test.not_eq(i32.{4}, i32.{5});
    test.assert(true, "should stay true", .{});

    return 0;
}
```

The validated run printed:

```text
log: boot 1
debug: trace ok
```

## What This Shows

### `std.msg` Is For Lightweight Human-Facing Diagnostics

These calls:

```kern
msg.log("boot {}", .{ 1, });
msg.debug("trace {}", .{ "ok", });
```

write formatted lines to standard error.

This is useful when you want:

- startup diagnostics
- temporary debugging output
- a small runtime-visible breadcrumb trail

without building a larger logging subsystem.

### `std.test` Gives You Assertion Vocabulary

These calls:

```kern
test.eq(...)
test.not_eq(...)
test.assert(...)
```

are the intended higher-level assertion interface.

They are especially natural inside real `test` targets, but the helpers are
ordinary library functions and can be used anywhere you intentionally want
fail-fast assertion behavior.

### Option And Result Assertions Are First-Class

These helpers:

```kern
test.expect_some(...)
test.expect_none(...)
test.expect_ok(...)
test.expect_err(...)
```

matter because they align with Kern's explicit carrier model.

Instead of unpacking carriers manually every time, you can assert the expected
shape directly and keep the success path readable.

### Assertion Failure Is Not Error Recovery

`std.test` helpers are for:

- tests
- invariants that should abort when violated

They are not a substitute for ordinary `?T` / `T!E` flow in production code.

If a condition is an expected recoverable outcome, model it as data.
If it is a hard failure for the current execution, assertion or panic helpers
can be appropriate.

### `std.msg` And `std.test` Solve Different Problems

Use this distinction:

- `std.msg` emits diagnostics and can trap through `panic` / `fail`
- `std.test` expresses test expectations and assertion-style checks

Keeping them separate makes code easier to read because the intent is visible
from the import itself.

## Practical Takeaway

Reach for:

- `std.msg.log` and `std.msg.debug` when you want lightweight runtime output
- `std.test.eq` / `not_eq` / `assert` for executable checks
- `std.test.expect_*` when you want to assert carrier shape directly

That gives Kern a small but clear testing and diagnostics story without hiding
failure policy under exceptions or global magic.
