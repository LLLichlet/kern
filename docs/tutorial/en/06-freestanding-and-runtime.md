# 06. Freestanding And Runtime Basics

English | [简体中文](../zh/06-底层与freestanding入门.md)

The Kern language layer itself does not assume processes, libc, heap
allocators, command-line arguments, or an operating system. Normal command-line
programs can use `std` and default startup, but that is a project choice, not
language semantics.

The model for this chapter: hosted and freestanding are not two dialects. They
are the same language used with different runtime strategies.

- hosted: the program runs in an OS process environment, usually using `std` and toolchain startup.
- freestanding: the project owns startup, linking, memory layout, and external environment boundaries.
- libc: an optional external ABI/ecosystem interface, not the foundation of Kern's standard library.

## Start With Defaults

For ordinary binary, example, and test targets, omitting `[runtime]` is
equivalent to:

```toml
[runtime]
entry = "rt"
libc = false
bundle = "std"
```

That means ordinary projects usually do not need runtime configuration just to
use the standard library:

- `rt` provides startup glue and expects a valid root-module `main`.
- libc is not linked implicitly.
- the `std` bundle wires official root aliases such as `base` and `std`.

Hosted does not mean "depends on libc." Kern's hosted capabilities go through
internal `std.host` implementations, and `std` is built on `base`. Libc is
selected explicitly only when you need C ABI compatibility, external C
libraries, or platform C runtime ownership.

## Three Runtime Axes

Current tooling splits runtime policy into three independent axes:

```toml
[runtime]
entry = "rt"
libc = false
bundle = "std"
```

- `entry`: who owns the program-entry contract.
- `libc`: whether libc is linked.
- `bundle`: which official library root aliases are wired.

Common `entry` values:

- `none`: no generated or required program-entry contract; the project exports its own entry symbol.
- `rt`: use Kern toolchain runtime startup.
- `crt`: let the platform C runtime own the earliest process startup.

Common `bundle` values:

- `none`: wire no official library root aliases.
- `base`: wire the freestanding base library.
- `std`: wire the common hosted `base` and `std` aliases.

`bundle` is root-alias wiring, not a prelude. Even with `bundle = "std"`, source
files still write `use std.io;` or `use base.mem.alloc.gpa;`.

## Hosted `main`

When `entry != "none"`, the root module's `main` is the special program entry.
Current legal forms are:

```kern
fn main() i32
```

or:

```kern
fn main(argc: i32, argv: &&u8) i32
```

`argv` is the low-level C-style process ABI. Ordinary code usually uses
higher-level wrappers from `std.proc`.

Rules for `main` are intentionally narrow:

- it must live in the target root module;
- it must not be `extern`;
- it must not be generic;
- it must return `i32`.

This contract only owns program entry. It does not allocate a heap, construct
high-level argument objects, or inject `std` names into scope.

## Minimal Freestanding Package

When a project owns startup, turn off runtime entry:

```toml
[package]
name = "kernel"
version = "0.1.0"
kern = "0.7.5"

[runtime]
entry = "none"
libc = false
bundle = "base"

[[bin]]
name = "kernel"
root = "src/main.rn"
```

This says:

- no toolchain startup looks for `main`;
- libc is not linked;
- only the `base` official layer is wired;
- the final entry symbol is exported by the project.

`src/main.rn` can export `_start`:

```kern
#[export_name("_start")]
fn kmain() void {
    while (true) {}
    @unreachable();
}
```

`kmain` is only the Kern source name. `#[export_name("_start")]` controls the
final exported symbol. The linker, bootloader, or platform ABI cares about
`_start`.

`entry = "none"` does not mean "no libraries." It only means the toolchain does
not own startup. You can still use freestanding `base` facilities such as
integers, slices, comparison, layout queries, allocator traits, and collections.
Anything that needs an OS boundary must be supplied by the project.

## The Same Shape With `kernc`

Normal projects should use `craft`, but the lower-level flags make the layers
clear:

```sh
kernc \
  --runtime-entry none \
  --runtime-libc no \
  --library-bundle base \
  --entry-symbol _start \
  src/main.rn \
  -o kernel.bin
```

Two entry concepts are involved:

- `--runtime-entry none`: do not enable Kern's `main`/startup contract.
- `--entry-symbol _start`: tell the final linked artifact which symbol is the entry.

The first is Kern runtime semantics. The second is link-layer selection.

## Linker Scripts Are Project Policy

Kernels, boot stages, firmware images, and bare-metal programs often need to
control sections, load addresses, and entry symbols. With `kernc`, pass linker
arguments explicitly:

```sh
kernc \
  --runtime-entry none \
  --runtime-libc no \
  --library-bundle base \
  --entry-symbol _start \
  --link-arg -T \
  --link-arg kernel.ld \
  src/main.rn \
  -o kernel.bin
```

In a `craft` package, prefer putting this policy in `build.rn`:

```kern
use craft.builder;

pub fn build(b: &mut builder.Builder) void {
    b.link_arg_path("-T", "link/kernel.ld");
}
```

`link_arg_path` records the path as a real link input. The linking policy stays
in the repository instead of command-line history.

For a fuller bootable image, `build.rn` can copy the kernel artifact, copy
resources, or call tools exposed by build dependencies. The repository's
[`examples/limine-smoke`](../../../examples/limine-smoke) is a small
freestanding example: it uses `entry = "none"` and `bundle = "base"`, exports
`_start`, and uses `build.rn` to assemble a Limine ISO.

## `build.rn` And `craft.rn`

Low-level projects often need extra build logic. Use this boundary:

- `Craft.toml`: package, targets, dependencies, resources, and runtime policy.
- `craft.rn`: optional pre-resolution planning script; affects the lockfile and is not needed for ordinary projects.
- `build.rn`: optional post-lock build script; good for linker scripts, generated files, C support files, artifact copying, and tool invocation.

If you only need to add `kernel.ld`, use `build.rn`. If you only need to
declare Limine resources, use `[resources]`. Do not hide this policy in manual
shell commands.

## C And Hardware Boundaries

Freestanding code often touches ABI, registers, MMIO, and external symbols.
Kern keeps these boundaries explicit:

- `extern struct`: C ABI layout, useful for hardware tables, boot protocols, and C header mappings.
- `&void` / `&mut void`: opaque FFI boundaries.
- `^T` / `^mut T`: address / volatile pointers, useful for MMIO.
- `as`: explicit numeric and pointer/integer boundary conversion.
- `#[export_name("...")]`: exported symbol control.
- `@asm`: inline assembly for port I/O, special instructions, and architecture glue.

A simplified port-output function:

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

Types and syntax help express intent, but the hardware contract, ABI contract,
and memory layout are still the project's responsibility.

## Choosing A Strategy

When starting a Kern project:

- Normal command-line tool: omit `[runtime]`; let `craft` use `entry = "rt"`, `libc = false`, `bundle = "std"`.
- Needs C libraries: consider `libc = true` or linker configuration explicitly; do not treat libc as the prerequisite for `std`.
- Kernel, bootloader, bare-metal program: use `[runtime] entry = "none"`, usually with `libc = false`, `bundle = "base"`, and export your own entry.
- Fully custom library roots: use `bundle = "none"` and wire required module roots via package dependencies or `kernc --module-path`.

The goal of this chapter is not to build a complete kernel immediately. It is
to set the boundary: startup, linking, memory, OS access, and libc are
project policy. Kern does not hide those policies in language defaults.
