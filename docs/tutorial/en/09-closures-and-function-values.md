# 09. Closures And Function Values

English | [简体中文](../zh/09-闭包与函数值.md)

Kern closures are not hidden runtime objects. A closure has two layers: a
concrete anonymous state value, and a callable entry point. Understanding those
layers makes it clear when a closure can be passed, when it escapes, and when
it must be allocated explicitly.

## Function Pointers

A function value without captured state can be represented as a normal function
pointer:

```kern
fn apply_operation(op: &fn(i32, i32) i32, a: i32, b: i32) i32 {
    return op(a, b);
}
```

An empty-capture closure can naturally pass to it:

```kern
let sum = apply_operation([](a: i32, b: i32) i32 {
    return a + b;
}, 10, 20);
```

`&fn(i32, i32) i32` is a thin pointer. It stores only a code address and carries
no closure state.

## Captures Are Explicit

Closure syntax is:

```kern
[captures](args) ReturnType {
    return value;
}
```

Capture names are explicit:

```kern
let base = i32.{100};

let add_base = [base](value: i32) i32 {
    return base + value;
};
```

To capture a pointer, name that binding in the capture list:

```kern
let mut counter = i32.{0};

let bump = [ptr = counter..&]() void {
    ptr.* += 1;
};
```

`ptr = counter..&` captures a writable pointer to `counter`. The closure body
modifies `ptr.*`, not a copied integer.

The right-hand side of a capture item is an ordinary expression, not a separate
capture sub-language. It can be address-of, field access, a function call, or
even a block expression:

```kern
let mut counter = i32.{0};

let bump = [ptr = {
    let p = counter..&;
    p
}]() void {
    ptr.* += 1;
};
```

For everyday code, reading `&T` / `&mut T` as a read view / write view is often
enough. Semantically, though, a pointer is still an ordinary value in Kern: the
closure captures that value, then later uses it to access target storage.

## `&Fn` And `&mut Fn`

Closures with state are called through closure fat pointers:

```kern
fn process_with_context(cb: &Fn(i32) i32, value: i32) i32 {
    return cb(value);
}

let base = i32.{100};
let result = process_with_context([base](value: i32) i32 {
    return base + value;
}, 23);
```

`&Fn(Args) Ret` is a read-only call interface. `&mut Fn(Args) Ret` allows the
call to mutate captured state:

```kern
fn repeat_twice(cb: &mut Fn() void) void {
    cb();
    cb();
}

let mut counter = i32.{0};
repeat_twice([ptr = counter..&]() void {
    ptr.* += 1;
});
```

When a function parameter explicitly expects `&Fn` or `&mut Fn`, Kern can
naturally package stack closure state into a fat pointer at that boundary.

## Closure State Is A Value

Writing a closure expression produces a concrete anonymous state value:

```kern
let left = i32.{10};
let right = i32.{20};

let closure = [left, right](x: i32) i32 {
    return (left + right) * x;
};
```

You cannot write this anonymous type's name directly, but you can query it with
`@typeOf(closure)`. By default, it lives like an ordinary local value in the
current scope.

If no context performs natural conversion, construct a closure fat pointer
explicitly:

```kern
let cb = &Fn(i32) i32.{ closure.& };
let out = cb(2);
```

The constructor receives a pointer to closure state. The fat pointer is not the
state itself; it is the dynamic interface: state pointer plus call entry.

## Escaping Closures Need Explicit Allocation

Do not return an `&Fn` pointing at stack closure state. If the closure escapes
the current function, store its state somewhere with a longer lifetime, such as
allocator-backed memory.

```kern
use base.mem.{Allocator, Layout, layout_of};

struct StoredClosure {
    callback: &Fn(i32) i32,
    layout: Layout,
};

fn create_heap_closure(alloc: &mut Allocator, factor: i32) StoredClosure {
    let stack_closure = [factor](value: i32) i32 {
        return factor * value;
    };

    let layout = layout_of[@typeOf(stack_closure)]();
    let raw = match (alloc.alloc(layout)) {
        .{ Some: storage } => storage as &mut @typeOf(stack_closure),
        .None => @trap(),
    };

    raw.* = stack_closure;
    return .{
        callback: &Fn(i32) i32.{ raw },
        layout,
    };
}
```

When freeing, do not pretend the closure fat pointer is an ordinary struct with
fields. Use `#` to extract its state pointer:

```kern
defer alloc.free((#stored.callback) as &mut u8, stored.layout);
```

`#` is the language operation for extracting fat-pointer state or metadata. It
also applies to slices; earlier chapters used `#slice` for length.

## Common Uses

Closures are common in Kern for:

- `Option.map`, `Result.map`, and `and_then` value transformations.
- `List.retain`, `Map.for_each`, and other container traversal APIs.
- File-tree, environment-variable, and visitor APIs.
- Local tests and one-off strategy functions.

Read these for examples:

- [`examples/test_closure.rn`](../../../examples/test_closure.rn)
- [`examples/closure_heap_escape.rn`](../../../examples/closure_heap_escape.rn)
- [`library/base/option.rn`](../../../library/base/option.rn)
- [`library/base/result.rn`](../../../library/base/result.rn)
- [`library/std/fs/dir.rn`](../../../library/std/fs/dir.rn)
