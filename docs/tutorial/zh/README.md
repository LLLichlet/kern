# Kern 教程

[English](../README.md) | 简体中文

这是 Kern 官方中文入门教程。它面向第一次接触 Kern 的读者，目标不是替代
[`design.md`](../../design.md)、[`craft.md`](../../craft.md) 或
[`runtime-architecture.md`](../../runtime-architecture.md)，而是作为一条导览式学习路线：带你从工具、语法和库用法开始，逐步进入 Kern 的底层编程模型。

本教程以 Kern 0.7.5 的当前实现为准。Kern 仍处于 pre-1.0 阶段，语法和库接口会随着设计收敛而调整；遇到细节冲突时，以 `docs/` 下的参考文档和仓库里的可运行示例为准。

## 适合谁阅读

本教程面向已经具备编程基础、并希望进入 Kern 系统编程模型的读者。阅读时会默认你理解“编译”“链接”“栈/堆”“指针”“整数宽度”等基本概念；但不会假设你已经理解 Kern 的模块、runtime、标准库分层和错误处理风格。

Kern 的定位是面向内核、固件、freestanding 软件和需要低层控制的基础设施代码。它提供现代语言结构，例如模块、泛型、代数数据类型、trait、穷尽模式匹配和包构建工具，同时避免隐藏的运行时策略：没有垃圾回收、没有异常、没有隐式堆分配，也没有默认注入的预导入命名空间。

## 学习路线

1. [快速开始](./01-快速开始.md)：安装后创建包、运行程序、理解 `craft` 与 `kernc` 的分工。
2. [语言基础](./02-语言基础.md)：函数、绑定、类型、字符串、格式化输出、mutability。
3. [数据与控制流](./03-数据与控制流.md)：struct、enum、match、option/result、错误传播。
4. [内存、切片与集合](./04-内存切片与集合.md)：数组、切片、指针、显式分配、`List` 与 `String`。
5. [模块、包与库分层](./05-模块包与库分层.md)：`use`、官方库层、`Craft.toml`、示例项目结构。
6. [底层与 freestanding 入门](./06-底层与freestanding入门.md)：runtime entry、`base` bundle、自定义 `_start`、链接脚本入口。
7. [聚合类型与初始化](./07-聚合类型与初始化.md)：struct 默认字段、field pun、layout、anonymous struct、union 与 enum 初始化。
8. [impl、trait 与泛型约束](./08-impl-trait与泛型约束.md)：方法、trait object、associated type、内置 trait 与运算符边界。
9. [闭包与函数值](./09-闭包与函数值.md)：`&fn`、`&Fn`、捕获、逃逸闭包和 `#` 状态提取。
10. [属性、intrinsic 与低层操作](./10-属性intrinsic与低层操作.md)：`#[...]`、`@sizeOf`、原子、SIMD、`@asm`。
11. [下一步](./11-下一步.md)：继续阅读现有文档、标准库源码和示例项目的建议路径。

## 推荐实践

阅读教程时建议打开两个目录：

- [`examples/`](../../../examples)：小而可运行的入门示例。
- [`library/`](../../../library)：官方库源码，尤其是 `base`、`prov`、`std` 的边界。

教程里的代码优先使用 `craft` 运行。直接使用 `kernc` 的场景会明确说明，因为 `kernc` 是低层编译/链接驱动，而不是包管理器。
