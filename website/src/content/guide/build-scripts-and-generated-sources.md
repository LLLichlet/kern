---
title: "`build.rn` And Generated Sources"
summary: "Use `build.rn` as the post-lock build orchestration phase that can generate or stage source files and then bind them as the unit's real source root."
order: 32
---

`build.rn` is where execution-time build orchestration belongs.

Unlike `craft.rn`, it runs after resolution and can shape how one already
selected unit is built.

## A Validated Example

The following package was checked and run successfully while writing this
guide:

```toml
[package]
name = "buildrn-guide"
version = "0.1.0"
kern = "0.7.1"
publish = false

[runtime]
entry = "rt"
libc = false
bundle = "std"

[[bin]]
name = "buildrn-guide"
root = "src/placeholder.rn"
```

```kern
// build.rn
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    let path = b.copy_package_file("templates/main.rn", "src/main.rn");
    b.set_source_root(path);
}
```

```kern
// templates/main.rn
use std.io;

fn main() i32 {
    io.println("built from template", .{});
    return 0;
}
```

The validated run printed:

```text
built from template
```

## What `check --verbose` Showed

During validation, `craft check --verbose` reported:

```text
scripts    workspace craft no, package craft 0, build.rn 1
  -- generated
  generate buildrn-guide:buildrn-guide [bin,target] -> .../src/main.rn<=copy:templates/main.rn
```

That is exactly the behavior this guide is meant to show:

- `build.rn` produced a generated/staged source
- that generated source replaced the unit's original source root
- the generated action stayed visible in `craft` output instead of being hidden

## Path Semantics Matter

`build.rn` is the execution-oriented phase, so its path model is deliberately
filesystem-facing.

The current rules are:

- `b.workspace.root` is an absolute normalized workspace path
- `b.package.root` is an absolute normalized package path
- for a workspace member package, those two paths differ
- for the root package, they are the same path
- `b.paths.build_root`, `b.paths.generated_root`, `b.paths.artifact_root`, `b.paths.object`, and `b.paths.artifact` are also absolute execution paths

That is different from `craft.rn`, where the roots are display-oriented and
workspace-relative.

## Relative Paths Are Package-Relative

The most important practical rule is that `build.rn` path operations are based
on the current package root.

For example:

- `b.set_source_root("src/main.rn")` resolves from the package root
- `b.link_search("native")` resolves from the package root during execution
- `b.copy_package_file(...)` reads from the package root
- `b.copy_package_file_to_artifact(...)` also reads from the package root

Absolute paths stay absolute, but relative paths are intentionally local to the
current package instead of the workspace root.

## The Function Shape

Current `build.rn` must use the builder API:

```kern
use craft.builder;

pub fn build(b: *mut builder.Builder) void { ... }
```

That is the required entrypoint shape for the current toolchain.

## Why The Placeholder Root Still Worked

The manifest pointed at:

```toml
root = "src/placeholder.rn"
```

but the actual executable came from the copied template.

That worked because:

```kern
let path = b.copy_package_file("templates/main.rn", "src/main.rn");
b.set_source_root(path);
```

bound the generated output as the unit's real source root for compilation.

## Linking Native Libraries From `build.rn`

`build.rn` is also the place where post-lock native link inputs belong.

While writing this guide, the current toolchain successfully ran a workspace
member package whose `build.rn` linked a local static library through a
package-relative search path:

```kern
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    b.link_search("native");
    b.link_system_lib("demo");
}
```

The library archive lived at:

```text
app/native/libdemo.a
```

and the validated run printed:

```text
workspace-member-native=42
```

This proves two useful things at once:

- `link_search("native")` is relative to the member package root, not the workspace root
- native link wiring belongs naturally in `build.rn`

That makes `build.rn` the right phase for things like:

- local static libraries
- generated native artifacts
- host/target/profile-specific linker adjustments

## Practical Mental Model

Think of `build.rn` as:

- after `Craft.lock`
- after package resolution
- about build-time orchestration and staged outputs
- about execution-local filesystem and linker work

If the behavior depends on how a chosen unit should be built, staged, linked,
or generated, `build.rn` is the candidate phase.

That is the sharp boundary:

- `craft.rn` changes planning before the graph is fixed
- `build.rn` changes execution after the graph is fixed
