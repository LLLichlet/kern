# The `kraft` Package Manager and Builder (v1 Draft)

This document defines the first formal design direction for `kraft`, the dedicated Kern package manager and build orchestrator.

`kraft` is intentionally separate from `kernc`.

- `kernc` compiles and links explicit inputs.
- `kraft` resolves packages, evaluates package configuration, constructs build plans, manages lockfiles, and invokes `kernc` with explicit actions.

The design goal is not to copy Cargo or Zig mechanically. The goal is to preserve Kern's core values:

- high abstraction, low policy
- strong orthogonality
- explicit phase boundaries
- deterministic and reproducible builds

## Scope and Responsibilities

`kraft` is responsible for:

- reading `Kraft.toml`
- evaluating optional `kraft.kr`
- normalizing package metadata into a deterministic package graph
- resolving dependencies and writing `Kraft.lock`
- constructing a build plan for targets, profiles, and workspaces
- optionally evaluating `build.kr` for low-level build orchestration
- invoking `kernc` and the system linker with explicit derived arguments
- managing local caches, registries, installs, and workspace builds

`kraft` is not responsible for:

- replacing the Kern language module system
- hiding `kernc` behind opaque compilation behavior
- inventing implicit dependency versions or hidden policy
- allowing arbitrary pre-lock scripts to destroy reproducibility

## Core Artifact Model

The build pipeline is split into four explicit artifacts:

1. `Kraft.toml`
2. `kraft.kr`
3. `Kraft.lock`
4. `build.kr`

The phase order is:

```text
Kraft.toml
  -> kraft.kr
  -> normalized package graph
  -> dependency resolver
  -> Kraft.lock
  -> build.kr
  -> explicit compile/link actions
```

This split is intentional.

- `Kraft.toml` describes static facts.
- `kraft.kr` performs pure elaboration and adaptation.
- `Kraft.lock` records the resolved dependency graph and the normalized package inputs.
- `build.kr` performs post-resolution build orchestration.

The most important design rule is:

- `kraft.kr` may affect package normalization and dependency resolution.
- `build.kr` may affect build execution.
- `build.kr` must not affect dependency resolution or lockfile contents.

## Why `kraft.kr` Exists

Pure TOML is excellent for declarations, but packages often need conditional structure:

- target-specific dependencies
- feature-driven target composition
- conditional module roots
- generated target lists
- workspace-local policy adaptation

Instead of pushing too much policy into the TOML schema, `kraft` uses an optional Kern file, `kraft.kr`, as a pure elaboration phase.

This keeps the system orthogonal:

- TOML remains a static manifest format.
- Kern remains the place for structured logic.
- The elaboration phase is explicit and bounded.

This is cleaner than a monolithic build script because it separates "what package graph exists" from "how the build executes".

## `kraft.kr` Semantics

`kraft.kr` is evaluated before dependency resolution and before lockfile generation.

Conceptually, it receives a mutable planning object that contains:

- the package metadata from `Kraft.toml`
- the active target triple
- the active build profile
- the selected feature set
- workspace-local metadata
- command mode such as `build`, `test`, or `check`

Its job is to elaborate the manifest into a normalized package description.

### Purity Requirements

Because `kraft.kr` runs before `Kraft.lock`, it is part of the lock input. Therefore it must be deterministic.

`kraft.kr` must be treated as a pure elaboration phase:

- no network access
- no wall clock access
- no randomness
- no probing transient machine state
- no implicit host-environment dependence

Allowed inputs should be narrowly defined:

- `Kraft.toml`
- the current package tree
- the workspace graph
- explicit command-line inputs
- explicit target/profile/feature selection

Environment access, if any, should be an explicit allowlist and should become part of the lock input digest.

### Proposed API Shape

The initial shape should be minimal:

```kern
use kraft.plan;

pub fn kraft(p: *mut plan.Plan) void {
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

### What `kraft.kr` May Do

- add or remove target-specific dependencies
- elaborate feature-driven configuration
- add package-local cfg values
- add or modify build targets already owned by the current package
- choose source roots or generated source groups
- apply workspace-local policy

### What `kraft.kr` Must Not Do

- perform I/O with external systems
- trigger actual compilation
- mutate dependencies after lock resolution
- inspect incidental host state in a hidden way
- behave differently across repeated identical invocations

## `build.kr` Semantics

`build.kr` is optional and runs after `Kraft.lock` has already been derived.

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
use kraft.build;

pub fn build(b: *mut build.Builder) void {
    if (b.target.os == .windows) {
        b.link.system_lib("ws2_32");
    }

    if (b.target.os == .darwin) {
        b.link.framework("Security");
    }
}
```

### `build.kr` May Do

- generate files inside designated build directories
- add link flags, libraries, frameworks, scripts
- define explicit pre-build and post-build actions
- register generated sources for current-package targets

### `build.kr` Must Not Do

- alter package resolution
- alter lockfile contents
- silently add versioned dependencies
- mutate workspace dependency topology

This separation is the central architectural difference between `kraft.kr` and `build.kr`.

## `Kraft.toml`

`Kraft.toml` is the static declaration source.

It should remain readable and mostly sufficient for ordinary packages. Most packages should not need either script file.

The intended V1 sections are:

- `[package]`
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
root = "src/lib.kr"

[[bin]]
name = "http-cli"
root = "src/main.kr"

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
- target-specific specialization belongs in either explicit manifest tables or `kraft.kr`

## `Kraft.lock`

`Kraft.lock` records the fully resolved external package graph and the normalized inputs that affect that graph.

The lockfile must be sufficient to answer:

- which package version was selected
- from which source it came
- with which checksum or immutable identity
- with which normalized dependency edges
- under which elaborated package configuration it was selected

The lockfile should capture digests for:

- `Kraft.toml`
- `kraft.kr`, if present
- normalized package metadata after elaboration

It should not capture build-only details from `build.kr`.

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
- `kraft.kr` may exist at package level and optionally at workspace root

If both root and package `kraft.kr` exist, the root phase should elaborate workspace-level policy first, then each package elaborates within that explicit workspace context.

## Command Surface

The initial command surface should be narrow and composable:

- `kraft init`
- `kraft new`
- `kraft check`
- `kraft build`
- `kraft run`
- `kraft test`
- `kraft fmt-manifest`
- `kraft fetch`
- `kraft update`
- `kraft tree`
- `kraft lock`
- `kraft clean`

Later:

- `kraft add`
- `kraft remove`
- `kraft publish`
- `kraft install`
- `kraft vendor`

### Behavioral Principles

- commands should be explicit about workspace root selection
- commands should expose target/profile/feature inputs directly
- commands should be replayable by CI without hidden machine state
- `kraft` should print the derived `kernc` action graph in a debug mode

## Interaction with `kernc`

`kraft` should treat `kernc` as an explicit compiler driver, not as a package manager.

For each planned build node, `kraft` derives:

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
  kraft/
docs/
```

This is preferable to placing `kraft` under `compiler/` because `kraft` is not a compiler pass. It is a top-level toolchain manager that happens to drive the compiler.

## V1 Implementation Phases

The most stable development order is:

1. manifest data model and parser
2. package ids, source ids, and workspace discovery
3. `kraft.kr` elaboration engine
4. normalized package graph
5. dependency resolver
6. `Kraft.lock` read/write
7. build plan model
8. `kraft check/build/run/test`
9. `build.kr`
10. registry and publishing flows

This order matters. A package manager becomes fragile when build execution is implemented before the package graph and lock model are well defined.

## V1 Non-Goals

The first version should explicitly avoid:

- hidden global caches with opaque invalidation
- pre-lock arbitrary scripts
- multiple competing manifest formats
- implicit dependency injection by build scripts
- complex plugin systems before the package graph is stable

## Summary

The defining architecture of `kraft` is:

- `Kraft.toml` for declarations
- `kraft.kr` for pure elaboration
- `Kraft.lock` for reproducible resolution
- `build.kr` for post-lock build orchestration

This keeps the system expressive without collapsing package definition, dependency resolution, and build execution into one opaque scripting phase.

That is the cleanest path to a package manager that fits Kern rather than imitating another language's historical compromises.
