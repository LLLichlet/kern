# 10. 属性、intrinsic 与低层操作

[English](../en/10-attributes-intrinsics-and-operators.md) | 简体中文

Kern 没有 C 预处理器式宏系统。需要影响编译、布局、链接、代码生成或直接使用目标机器能力时，通常会遇到三类语法：

- `#[...]` / `#![...]`：属性，附着在源码节点或词法作用域上。
- `@...`：编译器 intrinsic，由编译器直接实现。
- `^T` / `^mut T`：address / volatile 指针，用普通解引用表达 MMIO 访问。

这一章不是完整索引，而是把底层代码里最常用、最容易误用的部分按实际写法串起来。

## 外层属性与内层属性

外层属性作用在紧跟着的声明上：

```kern
#[export_name("_start")]
fn kmain() void {
    while (true) {}
    @unreachable();
}
```

内层属性作用在当前词法作用域，常见于文件或模块开头：

```kern
#![if(os == "linux")]
```

属性内容分两类：条件编译表达式和 metadata tags。不要把两类内容混在同一个 `#[...]` 里。

## 条件编译

条件属性会在语义分析前裁剪代码：

```kern
#[if(os == "linux")]
mod linux;

#[if(os == "windows")]
mod windows;
```

条件表达式使用 Kern 自己的布尔运算语法：`and`、`or`、`!`。也可以读取编译配置里的定义：

```kern
#[if((os == "linux" or os == "darwin") and !libc)]
mod posix_no_libc;
```

标准库的 hosted OS shim 大量使用这种模式。条件编译的核心效果是“裁剪源码节点”，不是运行时 `if`，因此被裁掉的分支不参与后续语义检查。

## 常见 metadata

链接和 FFI 常见属性：

- `export_name("...")`：指定导出符号名。
- `link_section("...")`：把函数或静态数据放进指定 section。
- `retain`：即使 Kern 代码没有直接引用，也要求保留。

布局常见属性：

- `packed`：移除字段 padding，可能带来非对齐访问代价。
- `align(N)`：指定类型或静态数据对齐。

优化和代码生成常见属性：

- `inline` / `noinline`：要求或禁止内联。
- `cold`：标记冷路径。
- `naked`：省略函数 prologue/epilogue，通常只用于极低层入口或中断 glue。
- `target_feature("...")`：给函数附加 CPU feature 要求，例如 `#[target_feature("avx2,fma")]`。

属性是编译器可理解的 metadata，不是运行时对象。属性参数里需要编译期常量时，编译器会在前端直接验证。

## 类型信息 intrinsic

这些 intrinsic 在编译期求值：

```kern
let size = @sizeOf[Pair]();
let align = @alignOf[Pair]();
let same_type_size = @sizeOf[@typeOf(pair)]();
```

- `@sizeOf[T]()`：类型大小，单位是 byte。
- `@alignOf[T]()`：类型 ABI 对齐，单位是 byte。
- `@typeOf(expr)`：表达式的精确编译期类型。

`@typeOf` 对匿名结构体和闭包尤其重要，因为这些类型没有源码名字。

## `@trap`、`@unreachable` 与调试断点

常见执行控制 intrinsic：

```kern
@trap();
@unreachable();
@breakpoint();
```

- `@trap()` 主动触发 trap，适合不可恢复错误、测试失败、暂时没有恢复路径的分支。
- `@unreachable()` 告诉编译器这里真的不可达，常用于已经转移控制权的路径之后。
- `@breakpoint()` 触发调试断点。

`@trap()` 是“走到这里就停”。`@unreachable()` 是给优化器和代码生成看的承诺，只有路径真的不会继续执行时才写。

```kern
fn exit_now(code: i32) ! {
    raw_exit(code);
    @unreachable();
}

fn expect_nonzero(value: i32) i32 {
    if (value == 0) {
        @trap();
    }
    return value;
}
```

实际源码中 `@trap()` 出现得更多是正常的：很多错误路径确实需要立即终止，而不是向优化器声明“控制流物理不可能到达这里”。

## 位运算和内存 intrinsic

整数位操作：

```kern
let bits = @popCount[u8](0b1011);
let swapped = @bswap[u16](0x1234);
let leading = @clz[u32](1);
let trailing = @ctz[u32](8);
```

这些 intrinsic 也可以用于整数 SIMD 向量，语义是逐 lane 处理。

内存块操作：

```kern
@memcpy(dest, src, len);
@memmove(dest, src, len);
@memset(dest, 0, len);
```

这些操作直接映射到编译器后端能力。调用者负责保证指针、长度、重叠关系和生命周期符合契约；Kern 不会在这里插入隐藏检查。

## `^T` 与 volatile 指针

Kern 把 MMIO / 固定地址访问建模为一类显式指针：

- `^T`：只读 address / volatile 指针。
- `^mut T`：可写 address / volatile 指针。

它们仍然是普通值，可以保存、传递、比较、显式转换成整数地址，也可以从整数地址显式转换回来。特殊之处在于：对 `^T` / `^mut T` 的 `.*` 解引用会生成 volatile load / store。

```kern
const UART_DR = 0x1000_0000usize;

fn read_data() u32 {
    let reg = UART_DR as ^u32;
    return reg.*;
}

fn write_data(value: u32) void {
    let reg = UART_DR as ^mut u32;
    reg.* = value;
}
```

volatile 是指针家族的一部分，访问仍然使用普通解引用语法。

`&T` / `&mut T` 适合普通对象内存，`^T` / `^mut T` 适合设备寄存器和固定地址。两者都可以显式 `as` 到 `usize`，也可以从 `usize` 显式转换回来，但读代码时含义不同：

```kern
let obj = addr as &mut u32;   // 普通对象内存
let reg = addr as ^mut u32;   // volatile 地址访问
```

原子同步用于普通共享内存，不用于 MMIO。设备寄存器应使用 `^T` / `^mut T` 和普通解引用。

## 原子操作

Kern 的原子能力由 intrinsic 提供，标准库 `base.sync` 在上面封装了更好用的 API。日常代码优先使用 `base.sync`：

```kern
use base.sync.{ACQUIRE, RELEASE, SEQ_CST, atomic};

let mut counter = atomic[usize](0);
counter..&.store[RELEASE](1);
let current = counter.&.load[ACQUIRE]();
```

底层 intrinsic 要求显式 memory ordering：

```kern
let mut raw_counter = 1usize;
let value = @atomicLoad[usize](raw_counter.&, SEQ_CST);
```

ordering 是编译期常量。教程代码里不建议直接散写数字，优先使用 `base.sync` 的命名常量。

常见 intrinsic 形状：

```kern
@atomicLoad[T](ptr, order);
@atomicStore[T](ptr, value, order);
@atomicXchg[T](ptr, value, order);
@atomicCas[T](ptr, expected, desired, success_order, failure_order);
@atomicRmwAdd[T](ptr, value, order);
@fence(order);
```

原子同步面向普通共享内存里的小型标量值和普通 thin pointer。浮点、`^T` / `^mut T`、切片、trait object、闭包 fat pointer 等不属于这组低层原子 payload；设备寄存器访问仍应使用 volatile 指针。

## SIMD 是内置值类型

Kern 有内置 SIMD 类型，例如 `i32x4`、`f32x4`、`u8x16`、`boolx4`。它们不是 `[N]T` 的别名，lane 数是类型拼写的一部分。

```kern
let a = i32x4.{ 1, 2, 3, 4 };
let b = i32x4.{ 4, 3, 2, 1 };

let sum = a + b;
let mask = a < b;
```

算术、位运算和比较都是逐 lane 运算。比较结果是 `boolxN`，不能直接放进 `if`，需要显式归约：

```kern
if (@simdAny(mask)) {
    @breakpoint();
}
```

lane 访问使用和数组相近的 `.[]` 语法，但 SIMD 不参与切片语义：

```kern
let mut v = f32x4.{ 1.0, 2.0, 3.0, 4.0 };
let second = v.[1];
v.[2] = 9.0;
```

当前固定宽度模型下，lane 下标必须是编译期常量并且在范围内。

## SIMD 常用工作流

最常见的 SIMD 代码不是先手写汇编，而是把连续内存加载成向量，逐 lane 计算，再归约或写回：

```kern
fn sum4(ptr: &f32) f32 {
    let values = @simdLoad[f32x4](ptr, 4);
    return @simdReduceAdd(values);
}

fn add4(ptr: &mut f32, delta: f32) void {
    let values = @simdLoad[f32x4](ptr, 4);
    let out = values + @simdSplat[f32x4](delta);
    @simdStore(ptr, out, 4);
}
```

`@simdLoad` / `@simdStore` 的第二个参数是显式 alignment 承诺，必须是编译期非零 2 的幂。它不是运行时检查。

mask 是 SIMD 日常开发的核心。比如扫描 16 个字节里第一个非空白字符：

```kern
fn first_non_space(chunk: &u8) usize {
    let bytes = @simdLoad[u8x16](chunk, 1);
    let spaces =
        (bytes == @simdSplat[u8x16](b' ')) |
        (bytes == @simdSplat[u8x16](b'\n')) |
        (bytes == @simdSplat[u8x16](b'\r')) |
        (bytes == @simdSplat[u8x16](b'\t'));

    let non_spaces = @simdBitmask(!spaces);
    if (non_spaces == 0) {
        return 16usize;
    }
    return @ctz(non_spaces);
}
```

这里有几个重要点：

- `|` 和 `&` 对 `boolxN` 是逐 lane mask 组合。
- `!mask` 是逐 lane 取反。
- `@simdBitmask` 把 `boolxN` 压成 `usize` 位图，lane `i` 对应 bit `i`。
- `@ctz` 可以找出第一个置位 lane。

选择和重排也不需要汇编：

```kern
let lo = f32x4.{ 1.0, 2.0, 3.0, 4.0 };
let hi = f32x4.{ 10.0, 20.0, 30.0, 40.0 };
let mask = boolx4.{ true, false, true, false };

let picked = @simdSelect(mask, lo, hi);
let mixed = @simdShuffle(lo, hi, [4]u32.{ 0, 5, 2, 7 });
let reversed = @simdReverse(lo);
```

`@simdSelect` 是按 mask 选 lane。`@simdShuffle` 从 `lhs ++ rhs` 这个拼接视图里取 lane：`0` 是 `lhs.[0]`，`4` 是 `rhs.[0]`。

非连续内存访问使用 gather / scatter：

```kern
let indices = [4]usize.{ 7, 0, 5, 2 };
let values = @simdGather[f32x4](base, indices.[0].&);
@simdScatter(out, indices.[0].&, values);
```

masked load / store / gather / scatter 的 masked-off lane 不访问对应内存，适合处理尾部元素或稀疏选择。

## `@asm`

内联汇编用结构化参数，不用字符串占位符隐式绑定变量：

```kern
fn outb(port: u16, value: u8) void {
    @asm(.{
        asm: "out dx, al",
        inputs: .{
            dx: port,
            al: value,
        },
        volatile: true
    });
}
```

带输出时，输出项绑定到可写指针：

```kern
fn syscall1_raw(sys_num: usize, arg1: usize) isize {
    let mut ret: isize = undef;

    @asm(.{
        asm: "syscall",
        outputs: .{ rax: ret..& },
        inputs: .{
            rax: sys_num,
            rdi: arg1,
        },
        clobbers: .{ "rcx", "r11", "memory" },
        volatile: true
    });

    return ret;
}
```

多行汇编使用 Kern 的多行字符串语法，`asm` 字段本身仍然必须是一个字符串字面量：

```kern
@asm(.{
    asm:
        \\nop
        \\nop
    ,
    volatile: true,
});
```

`asm`、`inputs`、`outputs`、`clobbers`、`volatile` 都是编译器消费的 metadata。它们不是普通 runtime struct。`volatile`、clobber 列表和模板字符串必须能在编译期确定。

`@asm` 的适用边界很窄：CPU 指令、特殊寄存器、系统调用入口、启动代码 glue。普通内存复制、原子、SIMD、字节序和位操作优先使用对应 intrinsic，因为编译器能理解这些操作并继续优化。

继续深入时，可以阅读 [`design.md`](../../design.md) 的 attributes、inline assembly 和 compiler intrinsics 章节。标准库里的 [`library/std/host/os/linux.rn`](../../../library/std/host/os/linux.rn)、[`library/base/sync/init.rn`](../../../library/base/sync/init.rn) 也很适合配合阅读。
