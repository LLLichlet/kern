# 01. Quick Start

English | [简体中文](../zh/01-快速开始.md)

## Install The Toolchain

Linux and macOS:

```sh
curl -sSf https://raw.githubusercontent.com/kern-project/kern/main/install.sh | bash
```

Windows PowerShell:

```powershell
powershell -Command "Set-ExecutionPolicy Bypass -Scope Process -Force; Invoke-Expression (Invoke-WebRequest -Uri https://raw.githubusercontent.com/kern-project/kern/main/install.ps1 -UseBasicParsing).Content"
```

The installer places the SDK under `~/.kern` on Unix or `%USERPROFILE%\.kern`
on Windows, then checks that `kernc`, `craft`, and `kern-lsp` can start.
For offline installs, source builds, and local SDK archives, see
[Installing Kern](../../install.md).

## Create Your First Package

```sh
mkdir hello
cd hello
craft init
```

`craft init` creates a minimal package. In the current version, you normally
get these project files:

```text
Craft.toml
Craft.lock
.gitignore
src/main.rn
```

You may also see `.craft/`. This is `craft`'s local derived-state directory for
locks, caches, build outputs, and analysis state. It is not source code you
need to maintain by hand.

Replace `src/main.rn` with:

```kern
use std.io;

fn main() i32 {
    "Hello, Kern!".println();
    return 0;
}
```

Then run:

```sh
craft run
```

This program uses only three ideas:

- `use std.io;` imports output support from the standard library. Kern does not automatically inject standard-library names into the current file.
- `fn main() i32` defines the executable entry point. The returned `i32` is the process exit code; `0` usually means success.
- `.println()` writes byte text to standard output and appends a newline.

For formatted output:

```kern
use std.io;

fn main() i32 {
    let name = "Kern";
    "Hello, {}!".fmt(.{name}).println();
    return 0;
}
```

`.{name}` is a small aggregate value passed to the formatter. For now, read it
as "the group of arguments for the format string." Later chapters cover
structs, anonymous aggregates, and slices.

## Remember Three `craft` Commands

When you start writing Kern programs, these three commands are enough:

```sh
craft check
craft build
craft run
```

They differ as follows:

- `craft check`: parse, type-check, and semantically analyze the code, usually faster than a full build.
- `craft build`: build the current package's default target.
- `craft run`: build and run the current package's default executable target.

Tests, examples, release profiles, cache cleanup, and cross-package selection
are covered in the package chapter. If you are in a directory with
`Craft.toml`, `craft run` is usually the first command to try.

The repository's `examples/` directory is also a Craft project. From the Kern
repository root, run one example with:

```sh
craft run -p examples --example hello_world
```

Or build all examples:

```sh
craft build -p examples --examples
```

## `craft` And `kernc`

Use `craft` for normal projects.

`craft` reads `Craft.toml`, decides which package, target, dependencies, and
generated steps are involved, then calls the compiler to do the actual work.

`kernc` is the lower-level compile and link driver. Use it when you need exact
control over one `.rn` file, object files, LLVM IR, module paths, or linker
arguments. For example:

```sh
kernc --runtime-entry rt --library-bundle std examples/hello_world.rn -o hello
./hello
```

You can skip `kernc` at first. This tutorial uses `craft` by default, then
returns to direct compiler flags when it reaches freestanding and low-level
linking. See [`craft.md`](../../craft.md) and [`kernc.md`](../../kernc.md) for the
complete references.
