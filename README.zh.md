<p align="center">
  <img src="./assets/brand/kern-logo.svg" alt="Kern" width="320"><br>
  用于编写内核、固件和 freestanding 程序的系统编程语言。
</p>

<p align="center">
  <a href="./README.md">English</a> |
  简体中文
</p>

<p align="center">
  <a href="#安装">安装</a> |
  <a href="#快速开始">快速开始</a> |
  <a href="#示例">示例</a> |
  <a href="#文档">文档</a>
</p>

> 当前状态：v0.7.6，实验阶段。Kern 尚未进入 1.0；当新的设计已经足够清晰时，语言和工具链会主动清理旧语法与历史包袱。

Kern 面向底层系统开发。它允许你直接面对内存、链接入口和运行时边界，同时仍然使用模块、泛型、代数数据类型、trait 和穷尽模式匹配。Kern 也自带包管理与构建工具，并把 freestanding 目标作为一等场景处理。

Kern 不提供垃圾回收，不使用异常，也不会在你没有要求时分配内存。运行时策略需要写在项目配置和源码里；`std` 只是建立在 `base`、`rt` 和 hosted 内部实现之上的库层，而不是编译器强制附带的一部分。

## 安装

Linux 和 macOS：

```sh
curl -sSf https://raw.githubusercontent.com/kern-project/kern/main/install.sh | bash
```

Windows PowerShell：

```powershell
powershell -Command "Set-ExecutionPolicy Bypass -Scope Process -Force; Invoke-Expression (Invoke-WebRequest -Uri https://raw.githubusercontent.com/kern-project/kern/main/install.ps1 -UseBasicParsing).Content"
```

安装器会把 SDK 放到 Unix 的 `~/.kern` 或 Windows 的 `%USERPROFILE%\.kern`，并检查 `kernc`、`craft`、`kern-lsp` 是否可以正常启动。

如果你在使用 NixOS，或者通过 Nix 管理工具链，请优先阅读
[Nix.md](./Nix.md)，不要走这里的 shell 安装流程。

离线安装、源码构建、本地 SDK 归档和可复现性检查见 [Installing Kern](docs/install.md)。

## 快速开始

创建一个包：

```sh
mkdir hello
cd hello
craft init
```

`craft init` 会创建一个最小包，其中包含 `Craft.toml` 和 `src/main.kn`。编辑 `src/main.kn`：

```kern
use std.io;

fn main() i32 {
    "hello, {}!"
        .fmt(.{"kern"})
        .println();
    return 0;
}
```

然后再次运行：

```sh
craft run
```

常用命令：

```sh
craft check
craft build
craft run
craft test
craft clean
```

选择包、可执行目标、示例或发布配置：

```sh
craft build -p path/to/package
craft run -b my-tool
craft run --example smoke
craft build --profile release
```

`craft init` 从单包项目开始。多包仓库使用 `[workspace]` 根来列出成员，
用 `[workspace.package]` 集中共享元数据，并通过 `[workspace.exports]`
声明哪些成员包导出给外部用户。完整的 Craft 模型见 [docs/craft.md](docs/craft.md)。

## 单文件编译

如果绕过 `craft` 直接调用编译器，需要显式选择运行时入口和库组合：

```sh
kernc --runtime-entry rt --library-bundle std examples/hello_world.kn -o hello
./hello
```

只生成目标文件：

```sh
kernc -c --runtime-entry rt --library-bundle std examples/hello_world.kn -o hello.o
```

查看 LLVM IR：

```sh
kernc --emit-llvm --runtime-entry rt --library-bundle std examples/hello_world.kn
```

完整的编译驱动说明见 [docs/kernc.md](docs/kernc.md)。

## 语言速览

```kern
use std.io;

enum ParseResult {
    Number: i32,
    Missing,
};

fn describe(result: ParseResult) void {
    match (result) {
        .{ Number: value } => "number = {}".fmt(.{value}).println(),
        .Missing => "missing".println(),
    }
}

fn main() i32 {
    describe(.{ Number: 42 });
    return 0;
}
```

Kern 倾向于把效果和边界留在明处：

- `let mut value` 表示这块存储可变。
- `&T`、`&mut T`、`^T` 和 `^mut T` 是显式指针值。
- `?T` 和 `T!E` 是内置枚举形式，不是隐式空引用或异常。
- `match` 必须穷尽。
- 不能悄悄丢弃函数返回值。

## 示例

仓库里保留 hosted 和 freestanding 场景下可以直接运行的示例：

- [examples](examples)：由 Craft 管理的入门示例。用 `craft build --project-path examples --examples` 构建全部示例，或用 `craft run --project-path examples --example hello_world` 运行单个示例。
- [examples/limine-smoke](examples/limine-smoke)：freestanding kernel 示例，通过 `craft` 构建可启动的 Limine ISO。
- [examples/limine-mkiso](examples/limine-mkiso)：Limine 示例使用的 hosted 构建工具。

从仓库根目录运行示例包：

```sh
craft build -p examples/limine-smoke
craft run -p examples/limine-mkiso -- --help
```

## 工具链

Kern 包含这些工具：

- `kernc`：编译、分析、目标文件生成和链接驱动。
- `craft`：包管理、锁文件同步和构建编排。
- `kern-lsp`：用于编辑器集成的语言服务器。
- `base`、`rt`、`std`：官方库分层。

需要发现包、解析依赖、运行构建脚本、生成文件，或选择测试/示例目标时，用 `craft`。需要精确控制某一次编译或链接动作时，再直接使用 `kernc`。

## 编辑器

第一方 VS Code 扩展位于 [editors/vscode](editors/vscode)。它提供 Kern 语言支持、语言图标、语义高亮、补全、悬浮信息、诊断、重命名和代码操作。

## 从源码构建

如果要在本地开发编译器：

```sh
git clone https://github.com/kern-project/kern.git
cd kern
cargo build --release
cargo test
```

构建结果会放在 `target/release/`，包括 `kernc`、`craft` 和 `kern-lsp`。

仓库维护命令正在迁移到 Rust 宿主工具。分组编译器集成测试优先使用：

```sh
cargo run -p kernworker -- ci kernc-tests --mode smoke
```

在 Windows 上从源码构建时，需要完整的 LLVM 21 开发环境，不能只安装面向使用者的 SDK。如果 `cargo build` 报告缺少 `libxml2.lib`、`libxml2s.lib` 等 LLVM 库，请参考 [Windows Distribution](docs/windows-distribution.md#local-development-build) 中的本地构建说明。

安装后的 SDK 目录结构、本地归档、离线安装和 Rust `kernup` 入口见
[Installing Kern](docs/install.md)。

## 文档

- [Documentation Map](docs/documentation-map.md)：当前文档集合的索引。
- [Installing Kern](docs/install.md)：SDK 安装、离线安装、源码构建、本地归档和可复现性检查。
- [Kern 教程](docs/tutorial/zh/README.md)：中文导览式入门教程，覆盖工具、语言基础、核心语义、库和 freestanding 入口。英文版见 [Kern Tutorial](docs/tutorial/README.md)。
- [Kern Language Design](docs/design.md)：当前语言语义和语法的参考文档。
- [Source Style Guide](docs/style.md)：仓库内 Kern 代码的风格约定。
- [`kernc` Compiler Guide](docs/kernc.md)：命令行、链接、LLVM 输出和集成细节。
- [`craft` Package And Build Guide](docs/craft.md)：包、锁文件、构建脚本、生成文件、资源和命令行为。
- [Runtime And Library Architecture](docs/runtime-architecture.md)：`base` / `rt` / `std` 分层和 freestanding 模型。
- [Unix Distribution](docs/unix-distribution.md) 与 [Windows Distribution](docs/windows-distribution.md)：平台相关的发布包策略和宿主基线说明。

## 贡献

欢迎提交问题报告、文档修正、测试和范围清晰的实现补丁。涉及语言设计变更或新语法时，请先开 issue，方便从 Kern 的 freestanding 场景和显式语义出发讨论方案。

## Star 走势

<a href="https://www.star-history.com/#kern-project/kern&Date">
  <img src="https://api.star-history.com/svg?repos=kern-project/kern&type=Date" alt="Star history chart">
</a>

## 许可证

Kern 使用 [MIT License](LICENSE)。
