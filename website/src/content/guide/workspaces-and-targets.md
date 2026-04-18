---
title: "Workspaces And Targets"
summary: "Use a workspace root to coordinate multiple packages, share dependency declarations, and select `bin` / `example` / `test` targets through `craft`."
order: 28
---

Once a project is bigger than one package, you should stop thinking in terms of
"one manifest, one binary".

`craft` already has a real workspace model for that.

## A Validated Workspace Example

The following layout was built and exercised successfully while writing this
guide:

```text
Craft.toml
app/
  Craft.toml
  src/main.rn
  examples/inspect.rn
  tests/smoke.rn
util/
  Craft.toml
  src/lib.rn
```

The workspace root declares members and shared dependency wiring:

```toml
[workspace]
members = ["app", "util"]

[workspace.dependencies]
util = { path = "util" }
```

The library member is ordinary:

```toml
[package]
name = "util"
version = "0.1.0"
kern = "0.7.0"
publish = false

[lib]
root = "src/lib.rn"
```

```kern
pub fn twice(value: i32) i32 {
    return value * 2;
}
```

The application member inherits that dependency from the workspace root and
declares `bin`, `test`, and `example` targets explicitly:

```toml
[package]
name = "workspace-app"
version = "0.1.0"
kern = "0.7.0"
publish = false

[[bin]]
name = "workspace-app"
root = "src/main.rn"

[dependencies]
util = { workspace = true }

[test]
roots = ["tests/smoke.rn"]

[example]
roots = ["examples/inspect.rn"]
```

The validated commands were:

```bash
craft check --project-path /tmp/kern-site-workspace-zw0Ae2
craft build --examples --project-path /tmp/kern-site-workspace-zw0Ae2
craft run --project-path /tmp/kern-site-workspace-zw0Ae2
craft run --example inspect --project-path /tmp/kern-site-workspace-zw0Ae2
craft test --project-path /tmp/kern-site-workspace-zw0Ae2
```

The validated outputs were:

```text
app 42
example 18
```

and `craft test` completed successfully with one executed test target.

## What This Teaches

### The Workspace Root Owns Coordination

The root `Craft.toml` is not just a folder marker.

It owns:

- member discovery
- shared dependency declarations
- the workspace-level `Craft.lock`

That is why the guide uses `--project-path` pointing at the workspace root.

### Member Packages Still Declare Their Own Targets

Inside the member package, runnable and testable roots remain explicit:

- `[[bin]]` for named binaries
- `[example].roots` for example targets
- `[test].roots` for test targets

`craft` does not invent those roots from directory conventions alone.

### `workspace = true` Means "Inherit The Spec"

This dependency entry:

```toml
[dependencies]
util = { workspace = true }
```

does not invent a dependency by itself.

It means "take the base dependency spec from `[workspace.dependencies]` at the
workspace root".

In the validated example, that shared spec was:

```toml
[workspace.dependencies]
util = { path = "util" }
```

and the path was resolved relative to the workspace root.

### Workspace Roots And Package Roots Stay Separate

The workspace root coordinates the graph, but member package behavior still
stays package-local where it should.

For example, while writing this guide the current toolchain successfully ran a
workspace member package whose `build.rn` used:

```kern
b.link_search("native");
```

with the native archive stored under:

```text
app/native/libdemo.a
```

The validated run printed:

```text
workspace-member-native=42
```

That matters because it shows two independent scopes working together:

- the workspace root owns member discovery and shared dependency policy
- the member package root still owns its own package-local sources, staged files, and relative `build.rn` paths

### `craft run` Selects One Runnable Target

With exactly one binary target across the workspace, plain `craft run` worked.

The example target needed explicit selection:

```bash
craft run --example inspect --project-path ...
```

That reflects the current command model:

- plain `craft run` expects exactly one runnable binary
- examples are runnable too, but you select them explicitly with `--example`

If you need explicit binary selection, the current CLI also supports:

```bash
craft run --bin workspace-app --project-path ...
```

That keeps the command line honest once a workspace grows beyond the
"exactly one runnable binary" shape.

## Practical Takeaway

Use one workspace root when packages evolve together.

Keep dependency sharing at the root, keep target declarations inside each
member, and let `craft` stay honest about what is being built, run, or tested.
