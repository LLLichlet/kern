---
title: "Enums And Patterns"
summary: "Use `enum`, builtin `?T` and `T!E` carriers, contextual `. { ... }` construction, propagation operators, and braced pattern matching in the current Kern style."
order: 7
---

Enums are one of Kern's main state-modeling tools.

That includes:

- ordinary named enums
- payload-carrying tagged unions
- builtin carrier families such as `?T` and `T!E`

The key rule is that state stays explicit in the type system, and payload access
stays explicit in patterns.

## A Validated Example

The following package was built and run successfully while writing this guide:

```kern
use std.io;

type Message = enum {
    Data: i32,
    Closed,
};

fn decode(msg: Message) i32 {
    return match (msg) {
        .{ Data: value } => value,
        .Closed => 0,
    };
}

fn maybe_bump(value: ?i32) ?i32 {
    let inner = value.?;
    return ?i32.{ Some: inner + 1 };
}

fn try_bump(value: i32!i32) i32!i32 {
    let inner = value.!;
    return i32!i32.{ Ok: inner + 10 };
}

fn main() i32 {
    let contextual = decode(.{ Data: 9 });
    let none_mark = match (maybe_bump(?i32.None)) {
        .None => i32.{1},
        .{ Some: _ } => i32.{0},
    };
    let result_value = match (try_bump(i32!i32.{ Ok: 5 })) {
        .{ Ok: value } => value,
        .{ Err: err } => err,
    };

    io.println("contextual={} none={} result={}", .{
        contextual,
        none_mark,
        result_value,
    });

    return if (contextual == 9 and none_mark == 1 and result_value == 15) 0 else 1;
}
```

The validated run printed:

```text
contextual=9 none=1 result=15
```

## What This Shows

### Payload-Carrying Enums Use `enum`

Kern uses `enum` both for plain sets and for tagged unions with payloads.

This declaration:

```kern
type Message = enum {
    Data: i32,
    Closed,
};
```

defines one payload variant and one payload-less variant.

### Pattern Matching Uses Braced Payload Destructuring

Payload access is done through explicit patterns such as:

```kern
.{ Data: value }
.{ Some: inner }
.{ Ok: value }
```

Current Kern intentionally rejects older payload-pattern spellings such as
`.Some: value`.

That is worth teaching directly because the current compiler expects braced
destructuring syntax for payload variants.

### Payload-Less Variants Use Direct Variant Syntax

Payload-less variants stay direct:

```kern
.Closed
.None
```

Do not wrap them in `.{ ... }`.

Current Kern rejects legacy payload-less constructor spellings like:

```kern
?i32.{ None }
```

### Builtin `?T` And `T!E` Are Real Enum Families

Kern's optional and result carriers are builtin language forms:

- `?T`
- `T!E`

But they should still be reasoned about like enums, not like magical null or
exception channels.

That is why they:

- construct with `Some` / `None` and `Ok` / `Err`
- match with the same pattern rules
- propagate through dedicated operators instead of hidden control flow

### Propagation Is Explicit

This chapter's example used:

```kern
let inner = value.?;
let inner = value.!;
```

Those operators are Kern's direct propagation syntax:

- `.?` propagates `None`
- `.!` propagates `Err`

That is still explicit control flow.
It is just compact explicit control flow.

### Contextual Construction Works When The Type Is Already Known

This call:

```kern
decode(.{ Data: 9 })
```

works because the callee already fixes the target type to `Message`.

That is the right way to think about elided enum construction:

- it works where the surrounding context already nails down the target enum type
- it is not a hidden global inference system

## Practical Takeaway

Keep these rules in mind:

- enums are Kern's normal tagged-state mechanism
- payload variants use braced destructuring patterns
- payload-less variants use direct variant syntax
- `?T` and `T!E` are builtin enum families, not hidden nullable/exception hacks
- `.?` and `.!` are explicit propagation operators

If you model state this way, `match` remains readable and the compiler's
exhaustiveness and diagnostic rules stay on your side.
