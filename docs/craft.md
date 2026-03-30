# The `craft` Package Manager and Builder (v1 Draft)

This document defines the first formal design direction for `craft`, the dedicated Kern package manager and build orchestrator.

`craft` is intentionally separate from `kernc`.

- `kernc` compiles and links explicit inputs.
- `craft` resolves packages, evaluates package configuration, constructs build plans, manages lockfiles, and invokes `kernc` with explicit actions.

The design goal is not to copy Cargo or Zig mechanically. The goal is to preserve Kern's core values:

- high abstraction, low policy
- strong orthogonality
- explicit phase boundaries
- deterministic and reproducible builds

## Scope and Responsibilities

`craft` is responsible for:

- reading `Craft.toml`
- evaluating optional `craft.rn`
- normalizing package metadata into a deterministic package graph
- resolving dependencies and writing `Craft.lock`
- constructing a build plan for targets, profiles, and workspaces
- optionally evaluating `build.rn` for low-level build orchestration
- invoking `kernc` and the system linker with explicit derived arguments
- managing local caches, registries, installs, and workspace builds

`craft` is not responsible for:

- replacing the Kern language module system
- hiding `kernc` behind opaque compilation behavior
- inventing implicit dependency versions or hidden policy
- allowing arbitrary pre-lock scripts to destroy reproducibility

## Core Artifact Model

The build pipeline is split into four explicit artifacts:

1. `Craft.toml`
2. `craft.rn`
3. `Craft.lock`
4. `build.rn`

The phase order is:

```text
Craft.toml
  -> craft.rn
  -> normalized package graph
  -> dependency resolver
  -> Craft.lock
  -> build.rn
  -> explicit compile/link actions
```

This split is intentional.

- `Craft.toml` describes static facts.
- `craft.rn` performs pure elaboration and adaptation.
- `Craft.lock` records the resolved dependency graph and the normalized package inputs.
- `build.rn` performs post-resolution build orchestration.

The most important design rule is:

- `craft.rn` may affect package normalization and dependency resolution.
- `build.rn` may affect build execution.
- `build.rn` must not affect dependency resolution or lockfile contents.

## Why `craft.rn` Exists

Pure TOML is excellent for declarations, but packages often need conditional structure:

- target-specific dependencies
- feature-driven target composition
- conditional module roots
- generated target lists
- workspace-local policy adaptation

Instead of pushing too much policy into the TOML schema, `craft` uses an optional Kern file, `craft.rn`, as a pure elaboration phase.

This keeps the system orthogonal:

- TOML remains a static manifest format.
- Kern remains the place for structured logic.
- The elaboration phase is explicit and bounded.

This is cleaner than a monolithic build script because it separates "what package graph exists" from "how the build executes".

## `craft.rn` Semantics

`craft.rn` is evaluated before dependency resolution and before lockfile generation.

Conceptually, it receives a mutable planning object that contains:

- the package metadata from `Craft.toml`
- the active target triple
- the active build profile
- the selected feature set
- workspace-local metadata
- command mode such as `build`, `test`, or `check`

Its job is to elaborate the manifest into a normalized package description.

### Purity Requirements

Because `craft.rn` runs before `Craft.lock`, it is part of the lock input. Therefore it must be deterministic.

`craft.rn` must be treated as a pure elaboration phase:

- no network access
- no wall clock access
- no randomness
- no probing transient machine state
- no implicit host-environment dependence

Allowed inputs should be narrowly defined:

- `Craft.toml`
- the current package tree
- the workspace graph
- explicit command-line inputs
- explicit target/profile/feature selection

Environment access, if any, should be an explicit allowlist and should become part of the lock input digest.

The intended model is:

- `Craft.toml` declares allowed elaboration environment inputs under `[craft]`
- `craft.rn` may only read environment inputs that were explicitly declared
- `Craft.lock` records the declared input values that participated in elaboration

For example:

```toml
[craft]
env = ["USE_SYSTEM_SSL", "KERN_SYSROOT"]
```

This keeps environment dependence explicit rather than incidental. If an allowed input changes, the lockfile becomes stale and `craft` must re-elaborate before reuse.

### Proposed API Shape

The initial shape should be minimal:

```kern
use craft.plan;

pub fn craft(p: *mut plan.Plan) void {
    if (p.target.os == .windows) {
        p.dep("win32", "1");
    }

    if (p.feature_enabled("simd")) {
        p.cfg("simd", true);
    }

    if (p.profile.name == "release") {
        p.define("aggressive_checks", false);
    }
}
```

The important point is not the exact method names. The important point is that this API manipulates a package plan, not a build executor.

### What `craft.rn` May Do

- add or remove target-specific dependencies
- elaborate feature-driven configuration
- add package-local cfg values
- add or modify build targets already owned by the current package
- choose source roots or generated source groups
- apply workspace-local policy

### What `craft.rn` Must Not Do

- perform I/O with external systems
- trigger actual compilation
- mutate dependencies after lock resolution
- inspect incidental host state in a hidden way
- behave differently across repeated identical invocations

## `build.rn` Semantics

`build.rn` is optional and runs after `Craft.lock` has already been derived.

Its role is not package elaboration. Its role is execution-phase build orchestration.

This includes:

- code generation
- resource processing
- system library linkage
- linker script setup
- custom packaging steps
- explicit extra build edges

### Proposed API Shape

```kern
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    match (b.target.os) {
        .windows => b.link_system_lib("ws2_32"),
        .darwin => b.link_framework("Security"),
        .linux => {},
        .unknown => {},
    }
}
```

Current V1 `Builder` support now includes:

- inspection of package, workspace, target, profile, command, and current unit
- inspection of derived build paths:
  - build root
  - generated root
  - object path
  - artifact path
  - optional metadata path
- per-unit compile-time mutation:
  - cfg bool/string
  - define bool/string
  - source-root override
- generated-file emission inside the designated generated directory
- package-local file copying into the designated generated directory
- link directives:
  - system libraries
  - frameworks
  - search paths
  - raw linker arguments

Generated files are now also recorded in the derived build plan, so `build.rn` effects remain auditable rather than being pure side effects.

Example:

```kern
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    let generated = b.copy_package_file("templates/main.rn", "src/main.rn");

    b.set_source_root(generated);
    b.cfg_bool("generated", true);
    b.define_string("entry", "generated");

    match (b.target.os) {
        .windows => b.link_system_lib("ws2_32"),
        .darwin => b.link_framework("Security"),
        .linux => {},
        .unknown => {},
    }
}
```

### `build.rn` May Do

- generate files inside designated build directories
- add link flags, libraries, frameworks, scripts
- define explicit pre-build and post-build actions
- register generated sources for current-package targets

### `build.rn` Must Not Do

- alter package resolution
- alter lockfile contents
- silently add versioned dependencies
- mutate workspace dependency topology

This separation is the central architectural difference between `craft.rn` and `build.rn`.

## `Craft.toml`

`Craft.toml` is the static declaration source.

It should remain readable and mostly sufficient for ordinary packages. Most packages should not need either script file.

The intended V1 sections are:

- `[package]`
- `[craft]`
- `[source.<name>]`
- `[lib]`
- `[[bin]]`
- `[[test]]`
- `[[example]]`
- `[dependencies]`
- `[dev-dependencies]`
- `[build-dependencies]`
- `[features]`
- `[profile.dev]`
- `[profile.release]`
- `[workspace]`
- `[workspace.package]`
- `[workspace.dependencies]`
- `[source]` or source-specific inline declarations

### Example

```toml
[package]
name = "http"
version = "0.1.0"
kern = "0.7"
edition = "2027"
publish = false

[lib]
root = "src/lib.rn"

[[bin]]
name = "http-cli"
root = "src/main.rn"

[dependencies]
net = "1"
log = { path = "../log" }

[dev-dependencies]
test_support = { path = "../test_support" }

[features]
default = []
tls = []
simd = []

[profile.dev]
opt = 0
debug = true

[profile.release]
opt = 3
debug = false
```

### Manifest Design Rules

- source structure should be explicit
- target declarations should be explicit
- dependency sources should be explicit
- features should be additive
- profile inheritance should be simple and deterministic
- target-specific specialization belongs in either explicit manifest tables or `craft.rn`

## `Craft.lock`

`Craft.lock` records the fully resolved external package graph and the normalized inputs that affect that graph.

The lockfile must be sufficient to answer:

- which package version was selected
- from which source it came
- with which checksum or immutable identity
- with which normalized dependency edges
- under which elaborated package configuration it was selected

The lockfile should capture digests for:

- `Craft.toml`
- `craft.rn`, if present
- normalized package metadata after elaboration

The current implementation direction should also persist a readable snapshot of normalized package targets, not only their digests, so lockfiles remain auditable.

It should not capture build-only details from `build.rn`.

### Lockfile Responsibilities

- reproducible dependency resolution
- deterministic workspace builds
- auditability of selected package sources
- support for offline rebuilds

### Lockfile Non-Responsibilities

- caching every compiler action
- recording every object file or artifact hash
- replacing a dedicated build cache index

## Dependency Sources

V1 should support a minimal but clean source model:

- path dependencies
- registry dependencies

Git dependencies can be added later, but only if they map cleanly onto lockfile identities.

Every resolved dependency should have a stable source identity:

- path source id
- registry package id plus checksum
- future git revision id

## Workspace Model

Workspaces should be first-class from the beginning.

Example:

```toml
[workspace]
members = [
    "compiler/*",
    "library/*",
    "tools/*",
]

[workspace.package]
license = "MIT"

[workspace.dependencies]
alloc = { path = "library/std/mem/alloc" }
```

### Workspace Rules

- one lockfile per workspace root by default
- workspace members share resolution
- local path members override registry lookups for the same member package id
- profiles may be overridden at the workspace root
- `craft.rn` may exist at package level and optionally at workspace root

If both root and package `craft.rn` exist, the root phase should elaborate workspace-level policy first, then each package elaborates within that explicit workspace context.

## Command Surface

The initial command surface should be narrow and composable:

- `craft init`
- `craft new`
- `craft check`
- `craft build`
- `craft run`
- `craft test`
- `craft fmt-manifest`
- `craft fetch`
- `craft update`
- `craft tree`
- `craft lock`
- `craft clean`

Later:

- `craft add`
- `craft remove`
- `craft publish`
- `craft install`
- `craft vendor`

### Behavioral Principles

- commands should be explicit about workspace root selection
- commands should expose target/profile/feature inputs directly
- commands should be replayable by CI without hidden machine state
- `craft` should print the derived `kernc` action graph in a debug mode

## Interaction with `kernc`

`craft` should treat `kernc` as an explicit compiler driver, not as a package manager.

For each planned build node, `craft` derives:

- source entry root
- module mappings
- target triple
- profile flags
- cfg values
- link profile
- artifact output path
- link inputs

Then it invokes `kernc` with explicit arguments.

This preserves the clean split already established by `docs/kernc.md`.

## Repository Layout

The package manager should live outside `compiler/`.

The recommended repository layout is:

```text
compiler/
library/
tools/
  craft/
docs/
```

This is preferable to placing `craft` under `compiler/` because `craft` is not a compiler pass. It is a top-level toolchain manager that happens to drive the compiler.

## V1 Implementation Phases

The most stable development order is:

1. manifest data model and parser
2. package ids, source ids, and workspace discovery
3. `craft.rn` elaboration engine
4. normalized package graph
5. dependency resolver
6. `Craft.lock` read/write
7. build plan model
8. `craft check/build/run/test`
9. `build.rn`
10. registry and publishing flows

The build-plan layer should exist as its own explicit model before actual command execution is implemented. `craft check` and future debug modes should be able to print this derived plan directly.

This order matters. A package manager becomes fragile when build execution is implemented before the package graph and lock model are well defined.

## V1 Non-Goals

The first version should explicitly avoid:

- hidden global caches with opaque invalidation
- pre-lock arbitrary scripts
- multiple competing manifest formats
- implicit dependency injection by build scripts
- complex plugin systems before the package graph is stable

## Summary

The defining architecture of `craft` is:

- `Craft.toml` for declarations
- `craft.rn` for pure elaboration
- `Craft.lock` for reproducible resolution
- `build.rn` for post-lock build orchestration

This keeps the system expressive without collapsing package definition, dependency resolution, and build execution into one opaque scripting phase.

That is the cleanest path to a package manager that fits Kern rather than imitating another language's historical compromises.
