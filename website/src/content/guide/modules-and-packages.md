---
title: "Modules And Packages"
summary: "Use explicit module trees inside one package, then step cleanly across package boundaries with `craft` dependencies."
order: 9
---

Kern keeps module shape and package boundaries explicit.

That is true both inside one package and across `craft` dependencies.

## A Validated Multi-Package Example

The following layout was built and run successfully while writing this guide:

```text
app/
  Craft.toml
  src/main.rn
  src/message/init.rn
  src/message/detail.rn
support/
  Craft.toml
  src/lib.rn
  src/math.rn
```

The dependency package exposes one public module:

```kern
// support/src/lib.rn
pub mod math;
```

```kern
// support/src/math.rn
pub fn double(value: i32) i32 {
    return value * 2;
}
```

The application package declares a local directory module and imports the
dependency root by package name:

```kern
// app/src/main.rn
use std.io;
use support.math;

mod message;

fn main() i32 {
    io.println("{} {}", .{
        message.greeting(),
        math.double(21),
    });
    return 0;
}
```

The local directory module uses `init.rn` plus a facade re-export:

```kern
// app/src/message/init.rn
mod detail;

pub use .detail.greeting;
```

```kern
// app/src/message/detail.rn
pub fn greeting() []u8 {
    return "ready";
}
```

For the validated run, this printed:

```text
ready 42
```

## The Important Rules

### Local Modules Are Explicit

Files do not silently become part of the build just because they exist on disk.

You still declare them in source:

```kern
mod message;
```

If `message` is a directory module, Kern looks for `message/init.rn`.

### Re-Exports Build The Public Surface

`pub use` is the normal way to expose a cleaner facade:

```kern
pub use .detail.greeting;
```

That lets you keep internal file layout separate from the API you want other
modules to consume.

### Package Roots Come From `craft`

In the validated example, this dependency entry:

```toml
[dependencies]
support = { path = "../support" }
```

made the package root available as:

```kern
use support.math;
```

That is the right mental model:

- `craft` resolves the package graph
- package names become import roots
- Kern module paths stay explicit after that

## Path Anchors To Remember

Kern uses different path anchors for different scopes:

- `use std.io;` for an external package root
- `use .detail;` for the current module
- `use ..helper;` for the parent module
- `use /pkg_level;` for the current package root

That split is stricter than languages that overload one "absolute import"
syntax for everything, but it keeps large codebases easier to audit.
