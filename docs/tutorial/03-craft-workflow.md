# Chapter 3: Projects With `craft`

`craft` is the package manager and build orchestrator for Kern. It is already
useful, but it should be treated differently from `kernc`:

- `kernc` is the stable direct tool
- `craft` is the higher-level graph and workflow tool

That distinction will save you time when debugging.

## What Exists Today

The current implementation already covers:

- `Craft.toml` discovery and validation
- workspace discovery
- local and external package graph resolution
- deterministic `Craft.lock` writing and freshness checks
- source configuration for directory-backed and git-backed registries
- `craft.rn` and `build.rn` discovery and validation
- explicit build-plan derivation
- `build`, `run`, and `test` execution through planned `kernc` actions

So the right mental model is not "imaginary future tool." The right mental
model is "usable system whose long-term surface is still settling."

## Minimal Package Layout

```text
demo/
  Craft.toml
  src/
    main.rn
```

`Craft.toml`:

```toml
[package]
name = "demo"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "demo"
root = "src/main.rn"
```

`src/main.rn`:

```kern
use std.io;

extern fn main(args: [][]u8) i32 {
    io.println("demo", .{});
    return 0;
}
```

## The Core Command Loop

```bash
craft check
craft lock
craft build
craft run
craft test
```

What each command means:

- `check`: parse, validate, discover, resolve, and report graph/build facts
- `lock`: write or refresh `Craft.lock`
- `fetch`: materialize external source trees into `.craft/sources`
- `build`: derive and execute the build plan
- `run`: build and run the selected executable target
- `test`: derive and execute test targets

Feature and profile inputs are explicit:

```bash
craft build --release
craft check --no-default-features --features tls,simd
```

## `Craft.toml` First, Scripts Second

The best discipline is:

1. express everything you can in `Craft.toml`
2. add `craft.rn` only when target/profile/feature elaboration becomes awkward
3. add `build.rn` only when you truly need generated files, staged assets, or
   custom link orchestration

That keeps the package graph auditable.

## `craft.rn`: Pre-Resolution Elaboration

`craft.rn` runs before dependency resolution and lockfile finalization. Its job
is to shape the package plan, not to perform a build.

Example:

```kern
use craft.plan;

pub fn craft(p: *mut plan.Plan) void {
    if (p.target.os == .windows) {
        p.dep_registry(.normal, "win32", "default");
    }

    if (p.feature_enabled("simd")) {
        p.cfg_bool("simd", true);
    }

    p.define_string("profile_name", p.profile.name);
}
```

Good uses for `craft.rn`:

- add target-specific dependencies
- enable cfg/define values from features
- add or remove package-owned targets
- read explicitly allowlisted environment variables

Bad uses for `craft.rn`:

- hidden machine-state logic
- runtime build execution
- anything that should really happen after locking

## `build.rn`: Post-Lock Build Orchestration

`build.rn` runs after the graph and targets are already known. Its job is to
shape generated files, staged artifacts, and link details.

Example:

```kern
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    if (b.target.os == .darwin) {
        b.link_framework("CoreFoundation");
    }

    b.link_system_lib("c");
}
```

The builder API also supports generated files, copied package assets, staged
artifact files, and tool-produced outputs.

## Sources And Release Policy

External sources are configured explicitly:

```toml
[source.default]
directory = "vendor/registry"
```

or:

```toml
[source.default]
git = "https://example.com/registry.git"
tag = "v1"
```

The release-policy machinery already exists. `craft check --release` can warn or
reject floating git inputs and insecure transports depending on `[craft]`
policy.

## Practical Advice

- learn `craft check` output before you depend on `craft build`
- keep `craft.rn` small and deterministic
- keep `build.rn` focused on build products, not package selection
- when something feels mysterious, reproduce the underlying step with `kernc`

That last point is especially important. `craft` should orchestrate `kernc`, not
replace your understanding of it.
