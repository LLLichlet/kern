---
title: "Maps And Ordered Trees"
summary: "Use `Map[K, V]` for hash-based lookup and `Tree[K, V]` when you need ordered traversal, floor, and ceil queries."
order: 13
---

After slices, lists, and strings, the next practical container question is
usually associative lookup.

Kern currently exposes two main owned associative containers in `base.coll`:

- `Map[K, V]` for hash-based lookup
- `Tree[K, V]` for ordered lookup and sorted traversal

Choosing between them is mostly about whether key order matters.

## A Validated Example

The following package was built and run successfully in a temporary directory
while writing this guide:

```kern
use std.io;
use base.coll.{Map, Tree, String};
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;

    let table = Map[i32, i32].{}..&;
    defer table.deinit(gpa);
    if (!table.insert(gpa, 7, 70) or !table.insert(gpa, 3, 30) or !table.insert(gpa, 9, 90)) {
        return 1;
    }
    if (!table.insert(gpa, 7, 77)) {
        return 2;
    }

    let hash_hit = match (table.get(7)) {
        .{ Some: value } => value,
        .None => return 3,
    };

    let ordered = Tree[i32, i32].{}..&;
    defer ordered.deinit(gpa);
    if (!ordered.insert(gpa, 7, 70)
        or !ordered.insert(gpa, 3, 30)
        or !ordered.insert(gpa, 9, 90)
        or !ordered.insert(gpa, 5, 50))
    {
        return 4;
    }

    let trace = String.{}..&;
    defer trace.deinit(gpa);
    ordered.for_each(.[trace, gpa](key: i32, _: i32) void {
        let _ = trace.push_char(gpa, (key as u8) + b'0');
    });

    let floor = match (ordered.floor(6)) {
        .{ Some: value } => value,
        .None => return 5,
    };
    let ceil = match (ordered.ceil(6)) {
        .{ Some: value } => value,
        .None => return 6,
    };

    io.println("map={} tree={} floor={} ceil={}", .{
        hash_hit,
        trace.as_str(),
        floor,
        ceil,
    });

    if (hash_hit != 77) {
        return 7;
    }
    if (!trace.eq("3579")) {
        return 8;
    }
    if (floor != 50 or ceil != 70) {
        return 9;
    }

    return 0;
}
```

The validated run printed:

```text
map=77 tree=3579 floor=50 ceil=70
```

## What This Shows

### `Map[K, V]` Is For Hash-Based Lookup

This block:

```kern
let table = Map[i32, i32].{}..&;
table.insert(gpa, 7, 70);
table.insert(gpa, 7, 77);
```

shows the intended hash-map behavior:

- insertion uses an allocator
- reinserting an existing key replaces its value
- lookup is by key, not by order

In the example, looking up `7` returns the replaced value `77`.

### `Tree[K, V]` Keeps Keys Ordered

This block:

```kern
let ordered = Tree[i32, i32].{}..&;
ordered.insert(gpa, 7, 70);
ordered.insert(gpa, 3, 30);
ordered.insert(gpa, 9, 90);
ordered.insert(gpa, 5, 50);
```

builds an ordered map.

Then:

```kern
ordered.for_each(...)
```

visits entries in sorted key order, producing the trace:

```text
3579
```

That is the main distinction from `Map`: iteration order is part of the data
structure contract.

### Ordered Queries Are First-Class On `Tree`

These calls:

```kern
ordered.floor(6)
ordered.ceil(6)
```

are exactly the kind of operations that justify an ordered tree.

For the sample data:

- the floor of `6` is `50` at key `5`
- the ceil of `6` is `70` at key `7`

If your algorithm needs nearest-key queries or sorted traversal, `Tree` is
usually the better fit.

### Both Containers Still Follow Kern's Ownership Model

Neither container hides allocation policy.

They are both owned values:

```kern
Map[K, V].{}..&
Tree[K, V].{}..&
```

and mutating them still requires an allocator:

```kern
insert(gpa, ...)
```

So the collection model stays consistent with the earlier `List` and `String`
chapters.

## Choosing Between Them

Use `Map[K, V]` when:

- you mainly need key lookup and updates
- key order does not matter
- you want ordinary hash-table behavior

Use `Tree[K, V]` when:

- you need sorted traversal
- you need `first`, `last`, `floor`, or `ceil`
- key order is part of the algorithm

## Trait Requirements Still Matter

The key constraints are different on purpose:

- `Map[K, V]` needs `Eq[K]` and `Hash[K]`
- `Tree[K, V]` needs `Ord[K]`

That means the container choice is also a statement about how your key type is
meant to behave.

## Practical Takeaway

Think about the split this way:

- `Map` is the normal hash-based associative container
- `Tree` is the ordered associative container
- both are explicit owned library values
- the right choice depends on whether key order is semantically important

That keeps Kern's associative-container story simple and predictable.
