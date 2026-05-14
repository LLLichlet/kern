# 05. Modules, Packages, And Library Layers

English | [简体中文](../zh/05-模块包与库分层.md)

Earlier chapters focused on syntax. Real projects split source across files,
and the build system needs to know entry points, dependencies, and runtime
choices. This chapter separates a few layers that are easy to mix up:

- A module is a Kern language namespace and source-organization unit.
- A package is a build unit described by `Craft.toml`; it can contain a library, binaries, examples, and tests.
- A workspace is a shared root for multiple packages, dependency declarations, lockfiles, and build state.
- The official libraries are layered freestanding-first and wired through runtime `bundle` choices.

## A Small Package

A normal executable package might look like:

```text
hello/
  Craft.toml
  src/
    main.kn
    math.kn
```

`craft init` produces something close to:

```toml
[package]
name = "hello"
version = "0.1.0"
kern = "0.7.6"

[[bin]]
name = "hello"
root = "src/main.kn"
```

`[[bin]]` declares a binary target, and `root = "src/main.kn"` says that
`src/main.kn` is the target's root module.

Most hosted applications do not need an explicit `[runtime]` table. For binary,
example, and test targets, `craft` defaults to toolchain runtime startup, no
implicit libc link, and the `std` bundle. Write runtime configuration only when
you are building a kernel, a freestanding program, a custom startup entry, or a
package that needs a different library bundle.

`src/main.kn` can declare child modules:

```kern
mod math;

use std.io;

fn main() i32 {
    let value = math.square(7);
    "square = {}".fmt(.{value}).println();
    return 0;
}
```

`src/math.kn` is brought into the module tree by `mod math;`:

```kern
pub/ fn square(value: i32) i32 {
    return value * value;
}
```

`pub/` means package-visible. `main.kn` and `math.kn` are in the same package,
so `main` can call `math.square`; code outside the package cannot treat it as a
public API.

## `mod` Declares Modules

Kern's module tree is declared explicitly. Files do not enter compilation just
because they exist on disk. A module must be declared with `mod`.

`mod name;` declares a child module under the current module. The module
declaration itself also has visibility: private by default, `pub/ mod name;`
for package-internal access, and `pub mod name;` for public package API.

The compiler finds module files by fixed rules:

- `name.kn` is a file module.
- `name/mod.kn` is the entry file for a directory module.
- `mod name { ... }` is an inline module and does not need an entry file.

So `mod.kn` is not an arbitrary name; it is the entry point for a directory
module. It usually declares further child modules and exposes that directory's
API.

```text
src/
  main.kn
  parse/
    mod.kn
    token.kn
```

`src/main.kn`:

```kern
mod parse;

fn main() i32 {
    return parse.run("123");
}
```

`src/parse/mod.kn`:

```kern
mod token;

pub/ fn run(text: &[u8]) i32 {
    return token.count_digits(text);
}
```

`src/parse/token.kn`:

```kern
pub.. fn count_digits(text: &[u8]) i32 {
    let mut count = 0;
    for (byte: text.iter()) {
        if (byte >= b'0' and byte <= b'9') {
            count += 1;
        }
    }
    return count;
}
```

`pub..` means visible to the parent module tree. `parse/mod.kn` can call
`token.count_digits`, but it does not become public package API.

Inline modules are useful when the child namespace is small enough to keep near
its parent:

```kern
mod api {
    pub fn answer() i32 {
        return detail.value();
    }

    mod detail {
        pub fn value() i32 {
            return 42;
        }
    }
}
```

Inline modules are still real module nodes. If an inline module declares a
file-backed child, that child is resolved below the inline module's logical
directory. For example, `mod api { mod detail; }` looks for `api/detail.kn` or
`api/detail/mod.kn`.

`mod` is not textual inclusion, and Kern does not need headers or forward
declarations. The compiler collects the module tree and resolves declarations
in multiple phases.

## `use` Imports Names

Kern has no automatic prelude. Import the modules or names you use:

```kern
use std.io;
use base.coll.{List, String, list, string};
use base.mem.alloc.gpa;
use std.mem.page;
```

`use std.io;` brings the module name `io` into the current scope.
`use base.coll.{List, String, list, string};` imports several names from one
module.

Imports only change how this file can write names. They do not change module
visibility and do not re-export dependencies for other modules.

Relative paths are common:

```kern
use ..types.{Error, Path};
```

`..` means the parent module. `/` means the current package root:

```kern
use /parse;

fn run_from_root(text: &[u8]) i32 {
    return /parse.run(text);
}
```

Use root aliases such as `base.coll` and `std.fs` across official library
boundaries. Use `/` for paths inside the same package, and `..` when a child
module refers to its parent tree.

## Visibility

Remember these three visibility forms:

- `pub`: visible outside the package; real public API.
- `pub/`: visible within the package.
- `pub..`: visible to the parent module tree.

No visibility marker means local to the current module.

```kern
fn helper() i32 {
    return 1;
}

pub.. fn parse_local(text: &[u8]) i32 {
    return helper();
}

pub/ fn parse_package(text: &[u8]) i32 {
    return parse_local(text);
}

pub fn parse_public(text: &[u8]) i32 {
    return parse_package(text);
}
```

Do not make everything `pub`. Use `pub/` for package-internal helpers and
`pub..` for parent-subtree organization.

Struct fields have their own visibility. Fields of a named struct are private
by default:

```kern
pub struct Span {
    pub start: usize,
    pub end: usize,
};
```

You can expose a type while keeping fields private and providing constructors
or methods instead.

## Packages And Targets

`Craft.toml` describes how `craft` builds a package. A minimal executable
usually needs `[package]` and one `[[bin]]`; libraries, examples, and tests use
their own target declarations.

Library package:

```toml
[package]
name = "mylib"
version = "0.1.0"
kern = "0.7.6"

[lib]
root = "src/lib.kn"
```

The library target name is the package name. Library targets do not own runtime
startup because they are not final executables.

Executable target:

```toml
[[bin]]
name = "tool"
root = "src/main.kn"
```

Examples and tests:

```toml
[example]
roots = [
    "examples/smoke.kn",
    "examples/demo.kn",
]

[test]
roots = ["tests/smoke.kn"]
```

Each root becomes an independent target. The name comes from the file name.
`examples/basics.kn` becomes `--example basics`.

If `[test].roots` is absent, `craft test` discovers direct `tests/*.kn` files
in that package. In a workspace this default is applied per member package;
the workspace root is not a test package by itself. Write `[test].roots`
explicitly when you want a different set, including globs such as
`"integration/*.kn"`. Recursive globs are not test roots; nested files should
usually be modules declared by a root.

Inside a test root, mark test cases with `#[test]`:

```kern
use base.io;
use base.test.report;

#[test]
fn adds() i32 {
    let t = report(io.discard())..&;
    (1 + 1).should().eq(2).sum(@loc(), t);
    return t.finish();
}

#[test]
fn accepts_args(argc: i32, argv: &&u8) i32 {
    return 0;
}
```

A test function returns `i32`, just like `main`: `0` passes and any other value
fails. It may take no arguments or `(argc: i32, argv: &&u8)`. `#[if(test)]`
keeps helper modules or declarations only when compiling in test mode.

Run one repository example:

```sh
craft run -p examples --example basics
```

Build all examples:

```sh
craft build -p examples --examples
```

`craft` selects packages and targets, resolves manifests, maintains
`Craft.lock`, and schedules build actions. `kernc` performs the lower-level
compile/link action for explicit inputs.

## Packages, Modules, And Workspaces

The boundaries are:

- module: source organization in the language, controlled by `mod` and `use`.
- package: a build unit described by `Craft.toml`, containing one or more targets.
- workspace: a namespace root for packages, dependencies, lockfiles, `.craft/` build state, and exported member packages.

If you come from C/C++, `Craft.toml` is closer to build description, while
`mod` is source-structure declaration. Kern has no header inclusion model.

`craft init` creates a single-package project. A workspace is a different root
shape: it has `[workspace]`, not `[package]`, and the buildable packages live in
member directories.

```toml
[workspace]
name = "json-kern"
members = [
    "json",
    "json-test",
    "json-bench",
]

[workspace.exports]
json = { member = "json" }

[workspace.package]
version = "0.1.0"
kern = "0.7.6"
license = "MIT"
authors = ["Example <dev@example.com>"]
readme = "README.md"
repository = "https://example.com/json-kern.git"

[workspace.dependencies]
json = { path = "json" }
```

The member package still owns its package name and targets:

```toml
[package]
name = "json"

[lib]
root = "src/lib.kn"
```

`[workspace.package]` centralizes shared metadata. `version` and `kern` become
member defaults when a member omits them. `description`, `license`, `authors`,
`readme`, and `repository` are shared publish defaults. `homepage` and
`documentation` are accepted shared metadata fields. Publish intent is owned by
`Craft.publish`, which is generated by `craft publish`; a workspace publishes
the members listed in `[workspace.exports]`.

`[workspace.exports]` is the external namespace. A workspace can contain helper
packages that are not exported. External users see only declared exports.

## Dependencies

Package dependencies live in `[dependencies]`. A local path dependency:

```toml
[dependencies]
mkiso = { path = "../limine-mkiso", export = "limine-mkiso" }
```

For an external workspace dependency, the dependency key selects an export with
the same name:

```toml
[dependencies]
json = { git = "https://example.com/json-kern.git", tag = "v0.1.0" }
```

Use `export` when the local dependency name should differ from the exported
package name:

```toml
[dependencies]
kern_json = { git = "https://example.com/json-kern.git", tag = "v0.1.0", export = "json" }
```

Workspaces can reuse dependency declarations:

```toml
[workspace.dependencies]
base = { path = "base" }
json = { path = "json" }

[dependencies]
base = { workspace = true }
json = { workspace = true }
```

Adding a dependency to the package graph does not import names into source
files. The manifest answers "which packages does this package depend on?";
`use` answers "which names does this file use?"

## Official Library Layers

Kern's official libraries are freestanding-first. They are not one monolithic
standard library that the compiler must inject. `craft` wires common root
aliases according to the runtime `bundle`, so hosted programs can use `std`
directly while kernels and custom runtimes can keep only the layers they need.

Current layers:

- `base`: freestanding foundation. Comparisons, numbers, pointer/layout helpers, allocator traits, collections, strings, synchronization primitives, generic IO traits, and test assertions.
- `rt`: startup entry and minimal runtime glue.
- `std`: user-facing higher-level library built on `base`, with hosted implementation under `std.host`.

`bundle = "std"` makes official root aliases such as `base` and `std`
available in normal hosted projects. `bundle = "base"` is better for
freestanding or lower-level packages. `bundle = "none"` wires none of these
official aliases.

Kern does not treat libc as the foundation of the language or standard
library. Normal targets default to `libc = false`; hosted `std` capabilities
come through internal `std.host` implementations. Libc is an optional external
ABI choice. See [`runtime-architecture.md`](../../runtime-architecture.md) for
the full background.

## Runtime Choice

Normal command-line programs usually use:

```toml
[runtime]
entry = "rt"
libc = false
bundle = "std"
```

Freestanding packages are usually more explicit:

```toml
[runtime]
entry = "none"
libc = false
bundle = "base"
```

This means the package owns its startup entry, does not use runtime startup,
and only wires the base library layer. The next chapter covers freestanding and
runtime in more detail.

## Reading Standard Library Source

Kern's standard library is intentionally explicit and low-policy. The source is
good tutorial material:

- [`library/base/option.kn`](../../../library/base/option.kn): common `?T` methods.
- [`library/base/result.kn`](../../../library/base/result.kn): common `T!E` methods.
- [`library/base/coll/iter.kn`](../../../library/base/coll/iter.kn): iterator trait and the model behind `for`.
- [`library/base/coll/list_impl/mod.kn`](../../../library/base/coll/list_impl/mod.kn): explicitly allocated growable list.
- [`library/base/coll/string_impl/mod.kn`](../../../library/base/coll/string_impl/mod.kn): how `String` is built on `List[u8]`.
- [`library/std/io/mod.kn`](../../../library/std/io/mod.kn): `println`, `Printable`, and standard streams.
- [`library/std/fs/`](../../../library/std/fs/): filesystem APIs through `std.host`.
