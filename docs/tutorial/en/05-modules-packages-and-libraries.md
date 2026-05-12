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
    main.rn
    math.rn
```

`craft init` produces something close to:

```toml
[package]
name = "hello"
version = "0.1.0"
kern = "0.7.5"

[[bin]]
name = "hello"
root = "src/main.rn"
```

`[[bin]]` declares a binary target, and `root = "src/main.rn"` says that
`src/main.rn` is the target's root module.

Most hosted applications do not need an explicit `[runtime]` table. For binary,
example, and test targets, `craft` defaults to toolchain runtime startup, no
implicit libc link, and the `std` bundle. Write runtime configuration only when
you are building a kernel, a freestanding program, a custom startup entry, or a
package that needs a different library bundle.

`src/main.rn` can declare child modules:

```kern
mod math;

use std.io;

fn main() i32 {
    let value = math.square(7);
    "square = {}".fmt(.{value}).println();
    return 0;
}
```

`src/math.rn` is brought into the module tree by `mod math;`:

```kern
pub/ fn square(value: i32) i32 {
    return value * value;
}
```

`pub/` means package-visible. `main.rn` and `math.rn` are in the same package,
so `main` can call `math.square`; code outside the package cannot treat it as a
public API.

## `mod` Declares Modules

Kern's module tree is declared explicitly. Files do not enter compilation just
because they exist on disk. A module must be declared with `mod`.

`mod name;` declares a child module under the current module. The module
declaration itself also has visibility: private by default, `pub/ mod name;`
for package-internal access, and `pub mod name;` for public package API.

The compiler finds module files by fixed rules:

- `name.rn` is a file module.
- `name/init.rn` is the entry file for a directory module.

So `init.rn` is not an arbitrary name; it is the entry point for a directory
module. It usually declares further child modules and exposes that directory's
API.

```text
src/
  main.rn
  parse/
    init.rn
    token.rn
```

`src/main.rn`:

```kern
mod parse;

fn main() i32 {
    return parse.run("123");
}
```

`src/parse/init.rn`:

```kern
mod token;

pub/ fn run(text: &[u8]) i32 {
    return token.count_digits(text);
}
```

`src/parse/token.rn`:

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

`pub..` means visible to the parent module tree. `parse/init.rn` can call
`token.count_digits`, but it does not become public package API.

`mod` is not textual inclusion, and Kern does not need headers or forward
declarations. The compiler collects the module tree and resolves declarations
in multiple phases.

## `use` Imports Names

Kern has no automatic prelude. Import the modules or names you use:

```kern
use std.io;
use base.coll.{List, list, range};
use base.mem.alloc.gpa;
use std.mem.page;
```

`use std.io;` brings the module name `io` into the current scope.
`use base.coll.{List, list, range};` imports several names from one module.

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
kern = "0.7.5"

[lib]
root = "src/lib.rn"
```

The library target name is the package name. Library targets do not own runtime
startup because they are not final executables.

Executable target:

```toml
[[bin]]
name = "tool"
root = "src/main.rn"
```

Examples and tests:

```toml
[example]
roots = [
    "examples/smoke.rn",
    "examples/demo.rn",
]

[test]
roots = ["tests/smoke.rn"]
```

Each root becomes an independent target. The name comes from the file name.
`examples/basics.rn` becomes `--example basics`.

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
kern = "0.7.5"
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
publish = true

[lib]
root = "src/lib.rn"
```

`[workspace.package]` centralizes shared metadata. `version` and `kern` become
member defaults when a member omits them. `description`, `license`, `authors`,
`readme`, and `repository` are shared publish defaults. `homepage` and
`documentation` are accepted shared metadata fields. `publish` is not
inherited: each member says whether it is released.

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
prov = { path = "prov" }

[dependencies]
base = { workspace = true }
prov = { workspace = true }
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
- `prov`: provider contracts. Shared OS-facing data contracts for hosted and freestanding providers.
- `rt`: startup entry and minimal runtime glue.
- `std`: user-facing higher-level library built on `base` and `prov`, with hosted implementation under `std.host`.

`bundle = "std"` makes official root aliases such as `base`, `prov`, and `std`
available in normal hosted projects. `bundle = "base"` is better for
freestanding or lower-level packages. `bundle = "none"` wires none of these
official aliases.

Kern does not treat libc as the foundation of the language or standard
library. Normal targets default to `libc = false`; hosted `std` capabilities
come through `prov` contracts and `std.host`. Libc is an optional external ABI
choice. See [`runtime-architecture.md`](../../runtime-architecture.md) for the
full background.

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

- [`library/base/option.rn`](../../../library/base/option.rn): common `?T` methods.
- [`library/base/result.rn`](../../../library/base/result.rn): common `T!E` methods.
- [`library/base/coll/iter.rn`](../../../library/base/coll/iter.rn): iterator trait and the model behind `for`.
- [`library/base/coll/list_impl/init.rn`](../../../library/base/coll/list_impl/init.rn): explicitly allocated growable list.
- [`library/base/coll/string_impl/init.rn`](../../../library/base/coll/string_impl/init.rn): how `String` is built on `List[u8]`.
- [`library/std/io/init.rn`](../../../library/std/io/init.rn): `println`, `Printable`, and standard streams.
- [`library/std/fs/`](../../../library/std/fs/): filesystem APIs through `prov` contracts and `std.host`.
