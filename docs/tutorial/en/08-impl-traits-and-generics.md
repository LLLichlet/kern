# 08. Impls, Traits, And Generic Bounds

English | [简体中文](../zh/08-impl-trait与泛型约束.md)

Methods, interfaces, generic algorithms, and operator overloading in Kern are
built on `impl` and traits. Earlier chapters already used `.println()`,
`.fmt()`, `.iter()`, and `.should()`; these are ordinary methods and traits, not
special syntax magic.

This chapter starts with the model you need to write code, then moves into
associated types, supertraits, and builtin traits.

## `impl` Is Written On Concrete Types

`impl` attaches methods to a concrete type:

```kern
struct Pair {
    x: i32,
    y: i32,
};

impl Pair {
    pub fn sum() i32 {
        return self.x + self.y;
    }
}
```

Method bodies have an implicit `self`:

```kern
let pair = Pair.{ x: 4, y: 5 };
let total = pair.sum();
```

Kern models by values: a pointer is a value, and a slice is a value. Therefore
`impl` can be written on value types and pointer types:

```kern
impl &mut Pair {
    pub fn move_by(dx: i32, dy: i32) void {
        self.x += dx;
        self.y += dy;
    }
}
```

Methods that mutate the object usually live on `&mut T`. Read-only methods can
live on `T` or `&T`, depending on whether the API wants value semantics or
pointer semantics.

Field access and method lookup are different rules. Field access is an access
path, so `self.x` inside `impl &mut Pair` can reach fields of `Pair`. Method
lookup is by the concrete receiver type: methods in `impl Pair` do not
automatically become methods of `&Pair` or `&mut Pair`. If you need to call a
value method from a pointer receiver, explicitly dereference, for example
`self.*.sum()`.

Note for Rust users: Kern does not have Rust-style receiver autoref/autoderef.
`Pair`, `&Pair`, and `&mut Pair` are independent concrete types, and the
standard library writes impls for each shape as needed.

## Generic Parameters

Generic parameters are introduced in square brackets:

```kern
fn identity[T](value: T) T {
    return value;
}
```

`[T]` only means "this function introduces a type parameter named `T`." It does
not say what `T` can do. This function cannot use `==` merely because `T` is
generic:

```kern
fn same[T](left: T, right: T) bool {
    return left == right;
}
```

Generic code must state the capabilities it uses in `where`:

```kern
fn same[T](left: T, right: T) bool
    where T: Eq[T],
{
    return left == right;
}
```

`where T: Eq[T]` means: type `T` must implement equality comparison with
another `T`.

`where` can appear on functions, structs, type aliases, traits, and impls. For
example:

```kern
struct Map[K, V]
    where K: Eq[K] + Hash[K],
{
    len: usize,
    buckets: &[V],
}
```

This is not decorative documentation; it is a precondition for the type.

## Traits Describe Capabilities

A trait defines a method contract:

```kern
trait Score {
    fn score() i32;
};
```

A type implements it with `impl Type : Trait`:

```kern
impl Pair : Score {
    pub fn score() i32 {
        return self.sum();
    }
}
```

Then generic functions can require that capability:

```kern
fn choose_better[T](left: T, right: T) T
    where T: Score,
{
    if left.score() >= right.score() return left;
    return right;
}
```

This is not class inheritance. `Pair` is not placed in a class hierarchy. There
is an impl proving that `Pair` satisfies the `Score` interface contract.

## `where` Bounds Concrete Types

Kern does not collapse `T`, `&T`, and `&mut T` into one thing. They are three
concrete types and may have different trait bounds:

```kern
fn write_value[T](writer: &mut Write, value: T) void
    where &T: Formatable,
{
    value.&.write_to(writer);
}
```

The bound is `&T: Formatable`, not `T: Formatable`. It means the concrete type
`&T` must implement `Formatable`.

You will see this pattern in the standard library:

```kern
impl[T] &List[T] : Formatable
    where &T: Formatable,
{
    pub fn write_to(writer: &mut Write) void {
        _ = writer.write("<List>");
    }
}
```

Kern only reasons about the final concrete type you write. If you need a
capability of `T`, write `T: Trait`. If you need a capability of `&T`, write
`&T: Trait`. If you need one for `&mut T`, write `&mut T: Trait`.

## Trait Objects Are Explicit Dynamic Interface Values

`where T: Score` is a static generic constraint: the compiler chooses an impl
for a concrete type. Runtime dynamic dispatch uses trait objects.

To pass different concrete types through one dynamic interface, construct a
trait object:

```kern
let mut sink = io.stderr();
let writer = sink..& as &mut Write;
```

`&mut Write` is a fat pointer containing a pointer to the concrete object and a
pointer to the `Write` vtable. Explicit packaging uses `as` from a compatible
pointer. At call and assignment boundaries, an expected `&mut Write` type can
also perform the same packaging naturally.

This is distinct from implementing a trait:

- `impl &mut File : Write`: proves the concrete type `&mut File` satisfies the interface.
- `file..& as &mut Write`: packages a concrete object as a dynamic interface value.

Kern requires trait objects to be built from pointers, avoiding any suggestion
that an unknown-size dynamic object is an ordinary stack value.

Fat pointers are a family of values. `&[u8]` is a slice fat pointer carrying a
data pointer and length. `&Write` / `&mut Write` is a trait-object fat pointer
carrying a data pointer and vtable. `&Fn(...) Ret` is a closure fat pointer
carrying a state pointer and call entry. Use language-defined operations such
as `slice.@len()` or `callback.@statePtr()` to extract their metadata or state
when needed. These representation projections use the explicit `.@name()`
spelling; plain methods remain library abstractions.

## Supertraits

Traits can require other traits:

```kern
trait Read {
    fn read(buffer: &mut [u8]) usize;
};

trait BufReader: Read {
    fn fill() void;
};
```

`BufReader: Read` means every `BufReader` must also satisfy `Read`. This is an
interface-contract dependency, not object inheritance.

Dynamic interface values can upcast along supertraits:

```kern
let reader = file.& as &BufReader;
let base = reader as &Read;
```

If a function needs `&Read`, passing `&BufReader` can also use boundary natural
conversion. The conversion rewrites fat-pointer vtable metadata; it does not
move the underlying object.

The standard library's `Ord` is a compact example:

```kern
trait Comparable[T] {
    fn cmp(other: T) Ordering;
};

trait Ord[T]: Eq[T] + Comparable[T] {};
```

## Associated Types

Some traits need a type bound to each implementation. This is an associated
type.

```kern
trait AddLike[Rhs] {
    type Out;
    fn add_like(rhs: Rhs) Out;
};
```

Each implementation of `AddLike[Rhs]` must state its output type:

```kern
impl Pair : AddLike[Pair] {
    type Out = Pair;

    pub fn add_like(rhs: Pair) Out {
        return .{
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        };
    }
}
```

Generic code can refer to that exact output type:

```kern
fn add_generic[T](left: T, right: T) T.AddLike[T].Out
    where T: AddLike[T],
{
    return left.add_like(right);
}
```

`T.AddLike[T].Out` means: the `Out` associated type declared by the
`T: AddLike[T]` implementation. Kern uses explicit trait-path projection
instead of `T.Out`, so it remains clear which trait's associated type is meant.

Associated types can appear in bounds:

```kern
fn add_to_i32[T](left: T, right: T) i32
    where T: AddLike[T, Out = i32],
{
    return left.add_like(right);
}
```

`Out = i32` requires not only `T: AddLike[T]`, but also that this
implementation's output type is exactly `i32`.

## Builtin Traits And Operators

Operators such as `+`, `==`, and `<` are described by language-owned builtin
capability traits:

- `Eq[Rhs]`: `==` and `!=`.
- `Lt[Rhs]`, `Le[Rhs]`, `Gt[Rhs]`, `Ge[Rhs]`: comparisons.
- `Add[Rhs]`, `Sub[Rhs]`, `Mul[Rhs]`, `Div[Rhs]`, `Rem[Rhs]`: arithmetic.
- `BitAnd[Rhs]`, `BitOr[Rhs]`, `BitXor[Rhs]`, `Shl[Rhs]`, `Shr[Rhs]`: bitwise and shift operators.
- `Neg`, `BitNot`, `Not`: unary value operators.

These traits are part of the language semantics and do not depend on `std` or a
special core package.

Builtin traits split into two categories:

- capability traits: operations a type supports, such as `Eq[T]`, `Add[T, Out = T]`, or `Neg`;
- marker traits: type-family classification, such as `Integer`, `SignedInteger`, `UnsignedInteger`, and `Float`.

Marker traits are not capability traits. `Integer` says a type belongs to the
integer family; it does not automatically prove every operation you might write.
Generic code should bound exactly what it uses:

```kern
fn add[T](left: T, right: T) T
    where T: Integer + Add[T, Out = T],
{
    return left + right;
}
```

Use marker traits for classification. Use capability traits for operators.
This explicit boundary makes low-level and freestanding generic code easier to
audit.

## Syntax That Is Not Overloadable

Kern intentionally limits overload boundaries. These remain language-owned:

- `and`, `or`: short-circuit control flow.
- `=` and compound assignment: storage mutation.
- `.&`, `..&`, `.*`: address-of and dereference.
- `#`: fat-pointer or container metadata/state extraction.

These forms carry control-flow or memory semantics and should not turn into
arbitrary user code. Kern overloads value computation, not every piece of
syntax.

## A Practical Order For Generic Code

When writing Kern generics:

1. Start with the non-generic version.
2. Extract varying types into `[T]`, `[K, V]`, or `[N: usize]`.
3. Add one `where` bound for each operation you use.
4. If the bound is about reference-shaped capability, write `&T` or `&mut T`.
5. Introduce associated types when the implementer chooses a type.
6. Construct trait objects only when you need runtime dynamic dispatch.

Do not start by turning everything into a trait object. Kern's default generic
style is static, explicit, and proved on concrete types. Trait objects are for
dynamic interface values.

## Where To Read Next

To study Kern's trait style, start with:

- [`library/base/io/traits.kn`](../../../library/base/io/traits.kn): `Read`, `Write`, `Formatable`.
- [`library/std/io/mod.kn`](../../../library/std/io/mod.kn): `Printable` and `println`.
- [`library/base/cmp/mod.kn`](../../../library/base/cmp/mod.kn): `Comparable`, `Ord`.
- [`library/base/hash/mod.kn`](../../../library/base/hash/mod.kn): `Hash`.
- [`library/base/coll/ranges.kn`](../../../library/base/coll/ranges.kn): marker and capability traits in numeric generics.
- [`library/base/coll/slice/query.kn`](../../../library/base/coll/slice/query.kn): generic slice methods and trait impls.
