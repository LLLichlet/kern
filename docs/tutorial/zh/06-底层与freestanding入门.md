# 06. 底层与 freestanding 入门

[English](../en/06-freestanding-and-runtime.md) | 简体中文

Kern 的语言层本身不假设进程、libc、堆分配器、命令行参数或操作系统。普通命令行程序当然可以使用 `std` 和默认 startup，但这些是项目选择，不是语言语义的一部分。

这一章要建立的模型是：hosted 和 freestanding 不是两个语言方言，而是同一套语言在不同 runtime 策略下的使用方式。

- hosted：程序运行在操作系统进程环境里，通常使用 `std` 和工具链 startup。
- freestanding：项目自己拥有启动入口、链接方式、内存布局和外部环境边界。
- libc：可选的外部 ABI 和生态接口，不是 Kern 标准库成立的前提。

## 先从默认值开始

上一章说过，普通 binary、example 和 test target 不写 `[runtime]` 时，`craft` 默认等价于：

```toml
[runtime]
entry = "rt"
libc = false
bundle = "std"
```

也就是说，普通项目一般不需要为了“使用标准库”而写 runtime 配置。默认策略已经是：

- 用 `rt` 提供启动 glue，并要求根模块里有合法 `main`。
- 不隐式链接 libc。
- 接入官方 `std` bundle，也就是 `base`、`std` 等 root alias。

这也是 Kern 和很多 C/C++ 项目的一个关键差异：hosted 不等于“依赖 libc”。Kern 的 hosted 能力通过内部 `std.host` 实现接入，`std` 建立在 `base` 上；libc 只是需要兼容 C ABI、链接外部 C 库或使用平台 C runtime 时才显式选择。

## 三条 runtime 轴

当前工具链把 runtime 策略拆成三条轴。它们彼此独立，不要把它们混成一个“模式”。

```toml
[runtime]
entry = "rt"
libc = false
bundle = "std"
```

- `entry`：谁拥有程序入口契约。
- `libc`：是否链接 libc。
- `bundle`：接入哪些官方库 root alias。

`entry` 常见取值：

- `none`：不生成或要求程序入口契约，项目自己导出入口符号。
- `rt`：使用 Kern 工具链提供的 runtime startup。
- `crt`：让平台 C runtime 拥有最早的进程 startup。

`bundle` 常见取值：

- `none`：不接入官方库 root alias。
- `base`：接入 freestanding 基础库。
- `std`：接入普通 hosted 项目常用的 `base`、`std`。

`bundle` 只是 root alias wiring，不是 prelude。写了 `bundle = "std"` 之后，源码里仍然要显式 `use std.io;`、`use base.mem.alloc.gpa;`。

## hosted `main` 契约

当 `entry != "none"` 时，根模块里的 `main` 是特殊程序入口。当前合法形式是：

```kern
fn main() i32
```

或：

```kern
fn main(argc: i32, argv: &&u8) i32
```

这个 `argv` 是底层 C 风格进程 ABI。普通代码一般不直接处理它，而是使用 `std.proc` 里提供的高层包装。

写简单命令行程序时，通常在 `main` 一开始包装这个底层 ABI。`argv[0]` 是程序路径或程序名，所以用户传入的业务参数一般从 `args.skip(1)` 开始：

```kern
use std.io;
use std.proc;

fn main(argc: i32, argv: &&u8) i32 {
    let args = proc.args(argc, argv);

    for item in args.skip(1).enumerate() {
        io.print("arg ");
        io.println(item.index);
        io.println(item.value);
    }

    return 0;
}
```

`main` 的规则很窄：

- 必须在 target 根模块里。
- 不能是 `extern`。
- 不能是泛型函数。
- 必须返回 `i32`。

这条契约只负责程序入口。它不隐式分配堆内存，不构造高级参数对象，也不把 `std` 名字塞进当前作用域。

## 最小 freestanding 包

如果项目要自己拥有启动入口，就把 `entry` 关掉：

```toml
[package]
name = "kernel"
version = "0.1.0"
kern = "0.8"

[runtime]
entry = "none"
libc = false
bundle = "base"

[[bin]]
name = "kernel"
root = "src/main.kn"
```

这表示：

- 没有工具链 startup 来找 `main`。
- 不链接 libc。
- 只接入 `base` 这一层官方库。
- 最终入口符号由项目自己导出。

`src/main.kn` 可以导出 `_start`：

```kern
#[export_name("_start")]
fn kmain() void {
    while true {}
    @unreachable();
}
```

这里函数名叫 `kmain` 只是源码里的 Kern 名字；`#[export_name("_start")]` 控制最终导出的符号名。链接器、bootloader 或平台 ABI 关心的是 `_start`。

`entry = "none"` 不代表“不能用库”。它只表示工具链不接管 startup。你仍然可以使用 `base` 里的整数、切片、比较、布局、allocator trait、集合等 freestanding 设施；只是任何需要 OS 边界的能力都必须由项目提供。

## 用 `kernc` 表达同一件事

日常项目优先用 `craft`，但理解底层参数有助于看清每一层在做什么。上面的 freestanding 包用 `kernc` 可以写成：

```sh
kernc \
  --runtime-entry none \
  --runtime-libc no \
  --library-bundle base \
  --entry-symbol _start \
  src/main.kn \
  -o kernel.bin
```

这里有两个容易混淆的入口概念：

- `--runtime-entry none`：告诉编译器不要启用 `main`/startup 契约。
- `--entry-symbol _start`：告诉最终链接产物以哪个符号作为入口。

前者是 Kern runtime 语义，后者是链接层选择。

## 链接脚本属于项目策略

内核、boot stage、固件镜像和裸机程序通常需要控制段布局、加载地址和入口符号。直接用 `kernc` 时，可以显式传 linker 参数：

```sh
kernc \
  --runtime-entry none \
  --runtime-libc no \
  --library-bundle base \
  --entry-symbol _start \
  --link-arg -T \
  --link-arg kernel.ld \
  src/main.kn \
  -o kernel.bin
```

在 `craft` 包里，更推荐把这类策略放进 `build.kn`：

```kern
use craft.builder;

pub fn build(b: &mut builder.Builder) void {
    b.link_arg_path("-T", "link/kernel.ld");
}
```

`link_arg_path` 会把路径作为真实链接输入记录下来。这样项目的链接策略留在仓库里，而不是散落在命令行历史中。

如果要构建更完整的 bootable 镜像，`build.kn` 还可以拷贝 kernel artifact、拷贝资源、调用 build dependency 暴露的工具。仓库里的 [`examples/limine-smoke`](../../../examples/limine-smoke) 就是一个小型 freestanding 示例：它用 `entry = "none"`、`bundle = "base"`，自己导出 `_start`，再通过 `build.kn` 组织 Limine ISO。

## `build.kn` 的位置

底层项目很容易需要额外构建逻辑。先记住一个实用边界：

- `Craft.toml`：声明包、target、依赖、资源和 runtime 策略。
- `build.kn`：可选的 post-lock 构建脚本，适合链接脚本、生成文件、C 支持文件、拷贝 artifact、调用工具。

所以如果你只是要给内核加 `kernel.ld`，用 `build.kn`。如果你只是要声明 Limine 资源，用 `[resources]`。不要把这些策略藏进手写 shell 命令里。

## 与 C 和硬件边界

freestanding 代码经常会碰到 ABI、寄存器、MMIO 和外部符号。Kern 给这些边界保留显式语法：

- `extern struct`：按 C ABI 约束布局，适合硬件表、boot protocol、C 头文件映射。
- `&void` / `&mut void`：opaque FFI 边界。
- `^T` / `^mut T`：地址/volatile 指针，适合 MMIO。
- `as`：显式数值转换和指针/整数边界转换。
- `#[export_name("...")]`：控制导出符号名。
- `@asm`：内联汇编，适合端口 I/O、特殊指令和架构相关 glue。

一个简化的端口输出函数可以写成：

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

底层边界的原则是：类型和语法可以帮你表达意图，但硬件契约、ABI 契约和内存布局仍然由项目负责。

## 选择策略

刚开始写 Kern 时，可以按下面的顺序选：

- 普通命令行工具：不写 `[runtime]`，让 `craft` 使用默认 `entry = "rt"`、`libc = false`、`bundle = "std"`。
- 需要链接 C 库：显式考虑 `libc = true` 或相关 linker 配置，但不要把它当成使用 `std` 的前提。
- kernel、bootloader、裸机程序：写 `[runtime] entry = "none"`，通常配 `libc = false`、`bundle = "base"`，自己导出入口。
- 完全自定义库根：使用 `bundle = "none"`，并通过包依赖或 `kernc --module-path` 明确接入需要的模块根。

这一章的核心不是让你马上写完整 kernel，而是建立边界感：启动、链接、内存、OS 访问和 libc 都是项目策略；Kern 不把这些策略藏进语言默认行为里。
