<p align="center">
  <img src="./logo.svg" alt="Kern logo" width="120">
</p>

<h1 align="center">Kern</h1>

<p align="center">
  A systems programming language for kernels, firmware, and freestanding software.
</p>

<p align="center">
  <a href="#install">Install</a> |
  <a href="#quick-start">Quick Start</a> |
  <a href="#examples">Examples</a> |
  <a href="#documentation">Documentation</a>
</p>

> Status: v0.7.5, experimental. Kern is pre-1.0 and deliberately removes
> historical syntax or toolchain baggage when the current design becomes clear.

Kern is designed for low-level software that still wants modern language
structure: explicit modules, generics, algebraic data types, traits, exhaustive
pattern matching, and a package/build tool that understands freestanding
targets.

There is no garbage collector, no exceptions, no implicit allocation, and no
hidden runtime policy. The standard library is optional layering over `base`,
`sys`, and `rt`, not a compiler requirement.

## Install

Linux and macOS:

```sh
curl -sSf https://raw.githubusercontent.com/softfault/kern/main/install.sh | bash
```

Windows PowerShell:

```powershell
powershell -Command "Set-ExecutionPolicy Bypass -Scope Process -Force; Invoke-Expression (Invoke-WebRequest -Uri https://raw.githubusercontent.com/softfault/kern/main/install.ps1 -UseBasicParsing).Content"
```

The installer places the SDK under `~/.kern` on Unix and
`%USERPROFILE%\.kern` on Windows, then verifies that `kernc`, `craft`, and
`kern-lsp` start successfully.

For offline installs, release packaging details, and host baseline notes, see
[Unix Distribution](docs/unix-distribution.md) and
[Windows Distribution](docs/windows-distribution.md).

## Quick Start

Create a package:

```sh
mkdir hello
cd hello
craft init
craft run
```

`craft init` creates a minimal package with `Craft.toml` and `src/main.rn`.
Edit `src/main.rn`:

```kern
use std.io;

fn main() i32 {
    io.println("hello, {}!", .{"kern"});
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

## Single File

For direct compiler use, call `kernc` with explicit runtime and library choices:

```sh
kernc --runtime-entry rt --library-bundle std examples/hello_world.rn -o hello
./hello
```

Compile only:

```sh
kernc -c --runtime-entry rt --library-bundle std examples/hello_world.rn -o hello.o
```

Inspect LLVM IR:

```sh
kernc --emit-llvm --runtime-entry rt --library-bundle std examples/hello_world.rn
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
        .{ Number: value } => io.println("number = {}", .{value}),
        .Missing => io.println("missing", .{}),
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

The repository carries small programs and larger incubator packages:

- [examples](examples): Craft-managed first-learn examples. Build all of them
  with `craft build --project-path examples --examples`, or run one with
  `craft run --project-path examples --example hello_world`.
- [incubator/json](incubator/json): parser, renderer, tests, examples, and
  benchmarks.
- [incubator/bitio](incubator/bitio): bit-level reader/writer package.
- [incubator/bed](incubator/bed): terminal editor experiment.
- [incubator/limine-smoke](incubator/limine-smoke): freestanding kernel package
  that builds a bootable Limine ISO through `craft`.

Run an incubator package from the repository root:

```sh
craft test -p incubator/json
craft run -p incubator/json --example hello_compact
```

## Toolchain

Kern ships these tools:

- `kernc`: compiler, analysis, object emission, and linking driver.
- `craft`: package manager, automatic lockfile synchronizer, and build orchestrator.
- `kern-lsp`: language server for editor integration.
- `base`, `sys`, `rt`, `std`: official library layers.

Use `craft` when you want package discovery, dependency resolution, build
scripts, generated files, or test/example selection. Use `kernc` when you want
to drive a specific compile or link action directly.

## Editors

The first-party VS Code extension lives in [editors/vscode](editors/vscode).
It provides Kern language support and a `Kern Icons` file icon theme for `.rn`
files.

## Build From Source

For local compiler development:

```sh
git clone https://github.com/softfault/kern.git
cd kern
cargo build --release
cargo test
```

This builds `kernc`, `craft`, and `kern-lsp` under `target/release/`.

Windows source builds require a full LLVM 21 development prefix, not only the
installed end-user SDK. If `cargo build` reports missing LLVM libraries such as
`libxml2.lib` or `libxml2s.lib`, follow the Windows source-build setup in
[Windows Distribution](docs/windows-distribution.md#local-development-build).

If you want a local SDK install that remains usable after deleting the source
checkout, package and install a local archive instead of copying
`target/release` by hand. The packaging entry point is:

```sh
python -m ops release package --version v0.7.5 --target <host-target>
```

## Documentation

- [Documentation Map](docs/documentation-map.md): where each kind of
  documentation lives.
- [Kern Language Design](docs/design.md): current language semantics and syntax.
- [Source Style Guide](docs/style.md): repository-level Kern code style.
- [The `kernc` Compiler Guide](docs/kernc.md): CLI, linking, LLVM output, and
  integration details.
- [`craft` Package And Build Guide](docs/craft.md): packages, lockfiles, build
  scripts, generated files, resources, and command behavior.
- [Runtime And Library Architecture](docs/runtime-architecture.md): the
  `base`/`sys`/`rt`/`std` split and freestanding model.
- [Unix Distribution](docs/unix-distribution.md) and
  [Windows Distribution](docs/windows-distribution.md): release packaging and
  installer policy.

## Contributing

Bug reports, documentation fixes, tests, and focused implementation patches are
welcome. For language design changes or new syntax, open an issue first so the
proposal can be checked against Kern's freestanding, explicit semantics.

## Star History

<a href="https://www.star-history.com/#softfault/kern&Date">
  <img src="https://api.star-history.com/svg?repos=softfault/kern&type=Date" alt="Star History Chart">
</a>

## License

Kern is licensed under the [MIT License](LICENSE).
