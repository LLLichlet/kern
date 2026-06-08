# 08. impl、trait 与泛型约束

[English](../en/08-impl-traits-and-generics.md) | 简体中文

Kern 里的方法、接口、泛型算法和运算符重载都建立在 `impl` 与 trait 系统上。前面章节已经用过 `.println()`、`.fmt()`、`.iter()`、`.should()`，这些不是特殊语法魔法，而是普通方法和 trait 组合出来的编程风格。

这一章先讲能开始写代码的模型，再讲关联类型、supertrait、内置 trait 这些更 Kern 的部分。

## `impl` 写在具体类型上

`impl` 给一个具体类型附加方法：

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

方法体里有隐式的 `self`。调用时：

```kern
let pair = Pair.{ x: 4, y: 5 };
let total = pair.sum();
```

Kern 按值建模，指针也是值，切片也是值。所以 `impl` 可以写在值类型上，也可以写在指针类型上：

```kern
impl &mut Pair {
    pub fn move_by(dx: i32, dy: i32) void {
        self.x += dx;
        self.y += dy;
    }
}
```

如果方法需要修改对象，通常实现到 `&mut T` 上；如果方法只是读取，可以实现到 `T` 或 `&T` 上，取决于 API 想表达的是值语义还是引用语义。

字段访问和方法查找的规则不一样。字段访问表达的是一条访问路径，所以 `self.x` 在 `impl &mut Pair` 里可以到达 `Pair` 的字段。方法查找则按 receiver 的具体类型进行：`impl Pair` 里的方法属于 `Pair`，不会自动变成 `&Pair` 或 `&mut Pair` 的方法。如果确实要从指针调用值方法，需要显式解引用，例如 `self.*.sum()`。

给 Rust 用户的提醒：Kern 没有 Rust 那种方法 receiver 的自动 deref/autoref 调整。`Pair`、`&Pair`、`&mut Pair` 都是独立的具体类型；标准库也会按需要分别给这些类型写 impl。

## 先理解泛型参数

泛型参数写在方括号里：

```kern
fn identity[T](value: T) T {
    return value;
}
```

`[T]` 的意思只是“这个函数引入一个类型参数 `T`”。它没有说明 `T` 能做什么。下面这个函数不能只因为 `T` 是泛型就使用 `==`：

```kern
fn same[T](left: T, right: T) bool {
    return left == right;
}
```

在 Kern 里，泛型代码必须把自己需要的能力写进 `where`：

```kern
fn same[T](left: T, right: T) bool
    where T: Eq[T],
{
    return left == right;
}
```

`where T: Eq[T]` 读作：类型 `T` 必须实现“可以和另一个 `T` 比较相等”的能力。

`where` 不只出现在函数上，也能出现在 struct、type alias、trait 和 impl 上。比如一个 map 类型可以把 key 的要求放在类型声明处：

```kern
struct Map[K, V]
    where K: Eq[K] + Hash[K],
{
    len: usize,
    buckets: &[V],
}
```

这不是装饰性说明，而是类型成立所需的前提。

## trait 描述能力

trait 定义一组方法契约：

```kern
trait Score {
    fn score() i32;
};
```

类型通过 `impl Type : Trait` 实现它：

```kern
impl Pair : Score {
    pub fn score() i32 {
        return self.sum();
    }
}
```

然后泛型函数可以要求这个能力：

```kern
fn choose_better[T](left: T, right: T) T
    where T: Score,
{
    if left.score() >= right.score() return left;
    return right;
}
```

这和 OOP 里的“某个类继承某个基类”不是同一件事。`Pair` 本身没有被放进一个 class hierarchy；它只是有一个 impl 证明：`Pair` 满足 `Score` 这个接口契约。

## `where` 约束的是具体类型

Kern 的类型系统不会把 `T`、`&T`、`&mut T` 混为一个东西。它们是三个具体类型，因此可以分别有不同的 trait 约束：

```kern
fn write_value[T](writer: &mut Write, value: T) void
    where &T: Formatable,
{
    value.&.write_to(writer);
}
```

这里约束的是 `&T: Formatable`，不是 `T: Formatable`。这不是“借用形态上的附加说明”，而是一个完整的类型约束：`&T` 这个类型必须实现 `Formatable`。

标准库里经常能看到这种写法：

```kern
impl[T] &List[T] : Formatable
    where &T: Formatable,
{
    pub fn write_to(writer: &mut Write) void {
        _ = writer.write("<List>");
    }
}
```

这一点和 Rust 读者的直觉也不完全一样。Kern 不把约束理解成“某个形态自动继承了底层类型的能力”；它只认最终写出来的具体类型。需要 `T` 的能力就写 `T: Trait`，需要 `&T` 的能力就写 `&T: Trait`，需要 `&mut T` 的能力就写 `&mut T: Trait`。

## trait object 是显式的动态接口值

上面的 `where T: Score` 是静态泛型约束：编译器在具体类型上选择 impl。另一种情况是运行时动态分发，也就是 trait object。

如果要把不同具体类型通过同一个动态接口传递，可以构造 trait object：

```kern
let mut sink = io.stderr();
let writer = sink..& as &mut Write;
```

`&mut Write` 是一个 fat pointer。它携带“指向具体对象的指针”和“指向 `Write` vtable 的指针”。显式打包使用 `as` 从兼容指针转换；在调用和赋值边界，如果上下文期望 `&mut Write`，也可以自然完成同样的打包。

这和“为某个类型实现 trait”不是一回事：

- `impl &mut File : Write`：说明 `&mut File` 这个具体类型满足接口。
- `file..& as &mut Write`：把一个具体对象包装成动态接口值。

Kern 要求 trait object 从指针构造，避免把未知大小的动态对象当成普通栈值。

fat pointer 是一类统一概念。`&[u8]` 是 slice fat pointer，携带数据指针和长度；`&Write` / `&mut Write` 是 trait-object fat pointer，携带数据指针和 vtable；`&Fn(...) Ret` 是 closure fat pointer，携带状态指针和调用入口。它们的用途不同，但都不是单纯的 thin pointer。需要拿到这类值携带的状态或长度时，使用语言定义的操作，例如 `slice.@len()` 或 `callback.@statePtr()`。这类表示层投影使用显式的 `.@name()` 写法；普通方法仍然是库抽象。

trait object 可以从指针形态构造，包括指向普通对象的指针，也包括指向 slice 这类 fat value 的指针。关键规则仍然是同一条：构造 trait object 时传入的是某个具体值的指针，而不是把 trait 当成基类来继承。

## supertrait

trait 可以要求另一个 trait：

```kern
trait Read {
    fn read(buffer: &mut [u8]) usize;
};

trait BufReader: Read {
    fn fill() void;
};
```

`BufReader: Read` 表示任何 `BufReader` 也必须满足 `Read` 的契约。实现 `BufReader` 时，相关 `Read` 义务也要能成立。

supertrait 不是 OOP 的对象继承。它表达的是接口契约之间的依赖。动态接口值也可以沿 supertrait 做 upcast：

```kern
let reader = file.& as &BufReader;
let base = reader as &Read;
```

如果函数需要 `&Read`，传入 `&BufReader` 时也可以发生边界自然转换。这个转换只改写 fat pointer 的 vtable metadata，不移动底层对象。

标准库里的 `Ord` 就是一个简单例子：

```kern
trait Comparable[T] {
    fn cmp(other: T) Ordering;
};

trait Ord[T]: Eq[T] + Comparable[T] {};
```

`Ord[T]` 表示这个类型既能做 equality，也能给出 ordering comparison。

## associated type

有些 trait 不只需要方法，还需要给出“和这个实现绑定在一起的类型”。这就是 associated type。

先看一个普通 trait：

```kern
trait Score {
    fn score() i32;
};
```

`score` 的返回值固定是 `i32`。但有些接口的返回类型取决于实现者。例如“两个值相加的结果类型”不一定总是左操作数类型：

```kern
trait AddLike[Rhs] {
    type Out;
    fn add_like(rhs: Rhs) Out;
};
```

`type Out;` 的意思是：每个 `AddLike[Rhs]` 的实现都必须说明自己的输出类型。

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

这给了泛型代码一种很强的表达能力：它可以不提前知道输出类型叫什么，但仍然把这个类型精确写出来。

```kern
fn add_generic[T](left: T, right: T) T.AddLike[T].Out
    where T: AddLike[T],
{
    return left.add_like(right);
}
```

`T.AddLike[T].Out` 读作：在 `T: AddLike[T]` 这个实现里声明的 `Out`。Kern 要求通过显式 trait 路径投影关联类型，而不是写成 `T.Out`。这样当同一个类型实现多个 trait，并且多个 trait 都有 `Out` 时，读者和编译器都知道你要的是哪一个。

associated type 还可以出现在约束里：

```kern
fn add_to_i32[T](left: T, right: T) i32
    where T: AddLike[T, Out = i32],
{
    return left.add_like(right);
}
```

这里 `Out = i32` 是对关联类型的约束：不仅要求 `T` 实现 `AddLike[T]`，还要求这个实现的输出类型正好是 `i32`。

这也是 Kern 泛型系统能表达复杂静态关系的基础之一。它不是运行时反射，也不是继承树查询，而是编译期证明：某个具体类型满足某个 trait，并且这个 trait 实现里的关联类型满足指定关系。

## 内置 trait 与运算符

`+`、`==`、`<` 等运算符也由语言拥有的内置 capability trait 描述。常用类别包括：

- `Eq[Rhs]`：`==` 和 `!=`。
- `Lt[Rhs]`、`Le[Rhs]`、`Gt[Rhs]`、`Ge[Rhs]`：比较。
- `Add[Rhs]`、`Sub[Rhs]`、`Mul[Rhs]`、`Div[Rhs]`、`Rem[Rhs]`：算术。
- `BitAnd[Rhs]`、`BitOr[Rhs]`、`BitXor[Rhs]`、`Shl[Rhs]`、`Shr[Rhs]`：位运算和移位。
- `Neg`、`BitNot`、`Not`：一元值运算。

这些 trait 是语言语义的一部分，不依赖 `std` 或某个特殊 core 包。这样 freestanding 代码也能表达泛型运算约束。

内置 trait 分成两类：

- capability trait：描述“能做什么操作”，例如 `Eq[T]`、`Add[T, Out = T]`、`Neg`。
- marker trait：描述“属于什么类型族”，例如 `Integer`、`SignedInteger`、`UnsignedInteger`、`Float`。

marker trait 不是能力 trait。`Integer` 表示这是整数类型族，但不等于“这个类型可以执行你要写的所有运算”；`Float` 表示浮点类型族，但不自动推出 `Add`、`Lt` 或格式化能力。泛型代码要约束自己真正使用的操作：

```kern
fn add[T](left: T, right: T) T
    where T: Integer + Add[T, Out = T],
{
    return left + right;
}
```

如果只需要分类，就用 marker trait；如果要写运算符，就写对应 capability trait。这个边界比很多语言更显式，但也让底层和 freestanding 泛型代码更容易审计。

## 不能重载的语法

Kern 有意限制重载边界。下面这些语法保留给语言本身：

- `and`、`or`：短路控制流。
- `=` 和复合赋值：存储修改。
- `.&`、`..&`、`.*`：取址和解引用。
- `#`：fat pointer 或容器的元数据/状态提取。

这些语法带有控制流或内存语义，不应该变成任意用户代码。Kern 支持的是“值计算”层面的重载，而不是 C++ 式的全语法重载。

## 开始写泛型代码的顺序

第一次写 Kern 泛型时，可以按这个顺序思考：

1. 先写非泛型版本，让数据流跑通。
2. 把会变化的类型抽成 `[T]`、`[K, V]` 或 `[N: usize]`。
3. 每使用一个操作，就补一个 `where` 约束。
4. 如果约束的是引用能力，明确写 `&T` 或 `&mut T`。
5. 如果接口需要表达“实现者决定的类型”，再引入 associated type。
6. 如果要运行时动态分发，再构造 trait object。

不要一开始就把所有东西都做成 trait object。Kern 的默认泛型风格是静态、显式、按具体类型证明能力；trait object 是需要动态接口值时再使用的工具。

## 读标准库时看哪里

想理解 Kern 的 trait 风格，可以先看这些文件：

- [`library/base/io/traits.kn`](../../../library/base/io/traits.kn)：`Read`、`Write`、`Formatable`。
- [`library/std/io/mod.kn`](../../../library/std/io/mod.kn)：`Printable` 和 `println`。
- [`library/base/cmp/mod.kn`](../../../library/base/cmp/mod.kn)：`Comparable`、`Ord`。
- [`library/base/hash/mod.kn`](../../../library/base/hash/mod.kn)：`Hash`。
- [`library/base/coll/ranges.kn`](../../../library/base/coll/ranges.kn)：marker trait 和 capability trait 如何一起约束数值泛型。
- [`library/base/coll/slice/query.kn`](../../../library/base/coll/slice/query.kn)：slice 上的泛型方法和 trait 实现。
