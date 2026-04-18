---
title: "`build.rn` And Generated Sources"
summary: "Use `build.rn` as the post-lock build orchestration phase that can generate or stage source files and then bind them as the unit's real source root."
order: 18
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
kern = "0.7.0"
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

That is exactly the behavior a user-facing guide should teach:

- `build.rn` produced a generated/staged source
- that generated source replaced the unit's original source root
- the generated action stayed visible in `craft` output instead of being hidden

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

## Practical Mental Model

Think of `build.rn` as:

- after `Craft.lock`
- after package resolution
- about build-time orchestration and staged outputs

If the behavior depends on how a chosen unit should be built, staged, linked,
or generated, `build.rn` is the candidate phase.

That is the sharp boundary:

- `craft.rn` changes planning before the graph is fixed
- `build.rn` changes execution after the graph is fixed
