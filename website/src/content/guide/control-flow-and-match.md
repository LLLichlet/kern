---
title: "Control Flow And Match"
summary: "Use `if`, `for`, and `match` in the current Kern style without hiding program state."
order: 6
---

Kern's control flow is deliberately unsurprising.

The language gives you familiar constructs such as `if`, `for`, and `match`,
but it pushes you toward explicit state and explicit branching rather than
clever implicit behavior.

## `if`

Use `if` when the branch is fundamentally a binary test:

```kern
fn classify(value: i32) Kind {
    if (value == 0) return .Zero;
    if (value > 0) return .{ Positive: value };
    return .{ Negative: value };
}
```

This reads as a direct sequence of decisions and returns.

## `for`

Kern's `for` loop is explicit enough to work well for low-level code:

```kern
for (; i < limit; i += i32.{1}) {
    total += i;
}
```

That makes it suitable both for ordinary loops and for machine-facing code
where the update step and termination condition should stay visible.

## `match`

`match` is where Kern's state-model story becomes most visible.

Given:

```kern
type Kind = enum {
    Zero,
    Positive: i32,
    Negative: i32,
};
```

you can branch over it explicitly:

```kern
match (classify(total)) {
    .Zero => io.println("zero", .{}),
    .{ Positive: value } => io.println("positive {}", .{value,}),
    .{ Negative: value } => io.println("negative {}", .{value,}),
};
```

This is a better fit for Kern than hiding state in status integers or relying
on side-channel conventions about what a value "really means".

## Why `match` Matters In Kern

Kern's design leans on enums plus `match` as one of its core mechanisms:

- state stays explicit in the type system
- branch structure stays visible in source
- the compiler can check exhaustiveness where appropriate

That combination is one of the main reasons Kern can aim for "high
abstraction, low policy" without sliding back toward hidden control flow.
