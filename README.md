<p align="center">
  <img src="./logo.svg" alt="Kern logo" width="120">
</p>

<h1 align="center">Kern</h1>

<p align="center">
  A systems programming language for kernels, firmware, and freestanding software.
</p>

<p align="center">
  English |
  <a href="./README.zh.md">简体中文</a>
</p>

<p align="center">
  <a href="#install">Install</a> |
  <a href="#quick-start">Quick Start</a> |
  <a href="#examples">Examples</a> |
  <a href="#documentation">Documentation</a>
</p>

> Status: v0.7.6, experimental. Kern is pre-1.0 and deliberately removes
> historical syntax or toolchain baggage when the current design becomes clear.

Kern is designed for low-level software that still wants modern language
structure: explicit modules, generics, algebraic data types, traits, exhaustive
pattern matching, and a package/build tool that understands freestanding
targets.

There is no garbage collector, no exceptions, no implicit allocation, and no
hidden runtime policy. The standard library is optional layering over `base`,
`rt`, and hosted internals, not a compiler requirement.

## Install

Linux and macOS:

```sh
curl -sSf https://raw.githubusercontent.com/kern-project/kern/main/install.sh | bash
```

Windows PowerShell:

```powershell
powershell -Command "Set-ExecutionPolicy Bypass -Scope Process -Force; Invoke-Expression (Invoke-WebRequest -Uri https://raw.githubusercontent.com/kern-project/kern/main/install.ps1 -UseBasicParsing).Content"
```

The installer places the SDK under `~/.kern` on Unix and
`%USERPROFILE%\.kern` on Windows, then verifies that `kernc`, `craft`, and
`kern-lsp` start successfully.

For offline installs, source builds, local SDK archives, and reproducibility
details, see [Installing Kern](docs/install.md).

## Quick Start

Create a package:

```sh
mkdir hello
cd hello
craft init
```

`craft init` creates a minimal package with `Craft.toml` and `src/main.kn`.
Edit `src/main.kn`:

```kern
use std.io;

fn main() i32 {
    "hello, {}!"
        .fmt(.{"kern"})
        .println();
    return 0;
}
```

Then run it again:

```sh
craft run
```

Common commands:

```sh
craft check
craft build
craft run
craft test
craft clean
```

Select a package, binary, example, or release profile:

```sh
craft build -p path/to/package
craft run -b my-tool
craft run --example smoke
craft build --profile release
```

`craft init` starts as a single-package project. Multi-package repositories use
a `[workspace]` root that names members, centralizes shared metadata with
`[workspace.package]`, and exports selected member packages through
`[workspace.exports]`. See [docs/craft.md](docs/craft.md) for the full Craft
model.

## Single File

For direct compiler use, call `kernc` with explicit runtime and library choices:

```sh
kernc --runtime-entry rt --library-bundle std examples/hello_world.kn -o hello
./hello
```

Compile only:

```sh
kernc -c --runtime-entry rt --library-bundle std examples/hello_world.kn -o hello.o
```

Inspect LLVM IR:

```sh
kernc --emit-llvm --runtime-entry rt --library-bundle std examples/hello_world.kn
```

For full compiler-driver usage, see [docs/kernc.md](docs/kernc.md).

## A Small Taste

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

Kern syntax keeps ownership of effects visible:

- `let mut value` makes storage mutable.
- `&T`, `&mut T`, `^T`, and `^mut T` are explicit pointer values.
- `?T` and `T!E` are built-in enum forms, not implicit nullable references or
  exceptions.
- `match` is exhaustive.
- Return values cannot be silently ignored.

## Examples

The repository carries runnable examples for both hosted and freestanding
programs:

- [examples](examples): Craft-managed first-learn examples. Build all of them
  with `craft build --project-path examples --examples`, or run one with
  `craft run --project-path examples --example hello_world`.
- [examples/limine-smoke](examples/limine-smoke): freestanding kernel example
  that builds a bootable Limine ISO through `craft`.
- [examples/limine-mkiso](examples/limine-mkiso): hosted build tool used by the
  Limine example.

Run an example package from the repository root:

```sh
craft build -p examples/limine-smoke
craft run -p examples/limine-mkiso -- --help
```

## Toolchain

Kern ships these tools:

- `kernc`: compiler, analysis, object emission, and linking driver.
- `craft`: package manager, automatic lockfile synchronizer, and build orchestrator.
- `kern-lsp`: language server for editor integration.
- `kernlib`: the official library workspace, containing `base`, `rt`, and `std`.

Use `craft` when you want package discovery, dependency resolution, build
scripts, generated files, or test/example selection. Use `kernc` when you want
to drive a specific compile or link action directly.

## Editors

The first-party VS Code extension lives in [editors/vscode](editors/vscode).
It provides Kern language support and a `Kern Icons` file icon theme for `.kn`
files.

## Build From Source

For local compiler development:

```sh
git clone https://github.com/kern-project/kern.git
cd kern
cargo build --release
cargo test
```

This builds `kernc`, `craft`, and `kern-lsp` under `target/release/`.
The official library workspace is checked in under `library/`. You can still
set `KERNLIB_PATH` to an external compatible library workspace when testing an
alternate library snapshot.

Repository maintenance commands are moving to Rust host tools. For grouped
compiler integration tests, prefer:

```sh
cargo run -p kernworker -- ci kernc-tests --mode smoke
```

Windows source builds require a full LLVM 21 development prefix, not only the
installed end-user SDK. If `cargo build` reports missing LLVM libraries such as
`libxml2.lib` or `libxml2s.lib`, follow the Windows source-build setup in
[Windows Distribution](docs/windows-distribution.md#local-development-build).

For installed SDK layout, local archives, offline installs, and the Rust
`kernup` entry point, see [Installing Kern](docs/install.md).

## Documentation

- [Documentation Map](docs/documentation-map.md): where each kind of
  documentation lives.
- [Installing Kern](docs/install.md): SDK installation, offline installs,
  source builds, local archive packaging, and reproducibility checks.
- [Kern Tutorial](docs/tutorial/README.md): introductory guided tour through
  tools, language basics, core semantics, libraries, and freestanding entry
  points. Also available in [Simplified Chinese](docs/tutorial/zh/README.md).
- [Kern Language Design](docs/design.md): current language semantics and syntax.
- [Source Style Guide](docs/style.md): repository-level Kern code style.
- [The `kernc` Compiler Guide](docs/kernc.md): CLI, linking, LLVM output, and
  integration details.
- [`craft` Package And Build Guide](docs/craft.md): packages, lockfiles, build
  scripts, generated files, resources, and command behavior.
- [Runtime And Library Architecture](docs/runtime-architecture.md): the
  `base`/`rt`/`std` split and freestanding model.
- [Unix Distribution](docs/unix-distribution.md) and
  [Windows Distribution](docs/windows-distribution.md): platform-specific
  release packaging policy and host baseline notes.

## Contributing

Bug reports, documentation fixes, tests, and focused implementation patches are
welcome. For language design changes or new syntax, open an issue first so the
proposal can be checked against Kern's freestanding, explicit semantics.

## Star History

<a href="https://www.star-history.com/#kern-project/kern&Date">
  <img src="https://api.star-history.com/svg?repos=kern-project/kern&type=Date" alt="Star History Chart">
</a>

## License

Kern is licensed under the [MIT License](LICENSE).
