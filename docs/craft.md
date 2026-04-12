# The `craft` Package Manager and Builder

This document describes the current architecture and design rules for `craft`, the dedicated Kern package manager and build orchestrator.

`craft` is intentionally separate from `kernc`.

- `kernc` compiles and links explicit inputs.
- `craft` discovers packages, evaluates package configuration, resolves dependencies, manages lockfiles, derives build plans, and executes those plans.

The goal is not to imitate Cargo or Zig mechanically. The goal is to preserve Kern's core values:

- orthogonality
- explicit phase boundaries
- deterministic behavior
- auditability
- low policy, high clarity

## Responsibilities

`craft` is responsible for:

- loading `Craft.toml`
- discovering workspace structure
- evaluating optional `craft.rn`
- normalizing package metadata into a deterministic package graph
- resolving dependencies
- reading and writing `Craft.lock`
- deriving explicit compile and link actions
- evaluating optional `build.rn`
- executing the derived build graph
- managing local package/source caches

All machine-local state owned by `craft` lives under `.craft/`. That tree is
derived state, not part of the reproducibility surface, and should not be
checked into version control.

`craft` maintains a root `.gitignore` entry for `.craft/` next to `Craft.toml`
when it creates local state. The ignore rule belongs at the package or
workspace root rather than inside `.craft/`, because the derived-state
directory itself should be ignored as one unit.

If a repository already tracked files under `.craft/`, that is a one-time VCS
cleanup problem rather than a build reproducibility input, and those entries
should be removed from the index.

`craft` is not responsible for:

- replacing the Kern language module system
- hiding `kernc` behind opaque behavior
- introducing implicit dependency or target policy
- allowing pre-lock scripts to smuggle in hidden machine state

## Concurrency And Derived-State Rules

`craft` must treat `.craft/` as shared derived state that can be touched by
multiple toolchain entry points over time. That means concurrency rules are part
of the design, not an implementation detail.

The current rules are:

- workspace-scoped operations take a workspace lock under `.craft/lock/`
- workspace identity is based on the canonicalized `Craft.toml` path, not on the
  textual path the user typed
- single-file shared state such as `Craft.lock` and `.craft/analysis.toml` must
  be written atomically
- shared artifact directories such as metadata snapshots must not rely on
  `remove_dir_all + recreate` without an output-specific lock
- stale locks may be reclaimed only when the recorded owner process is no longer
  alive

This split is intentional.

- workspace locks serialize commands that mutate shared workspace state at the
  command level
- artifact/output locks protect narrower shared outputs that may also be reached
  by lower-level compiler entry points
- atomic file replacement prevents readers from observing truncated or partially
  rewritten state files

When adding new `.craft/` state, the default policy should be:

1. decide whether the state is workspace-wide, output-wide, or private to one
   action
2. if it is shared, define the lock scope explicitly
3. if it is a file, prefer atomic replacement
4. if it is a directory tree, avoid in-place destructive rewrites unless the
   tree is covered by a dedicated output lock

If a new feature cannot state its lock scope and replacement strategy clearly,
its state model is still underspecified.

## Phase Model

The package pipeline is split into four explicit artifacts:

1. `Craft.toml`
2. `craft.rn`
3. `Craft.lock`
4. `build.rn`

The order is:

```text
Craft.toml
  -> craft.rn
  -> normalized package graph
  -> dependency resolution
  -> Craft.lock
  -> build.rn
  -> explicit compile/link actions
  -> execution
```

This split is the core of the design.

- `Craft.toml` carries static declarations.
- `craft.rn` elaborates those declarations before resolution.
- `Craft.lock` records the resolved graph and the normalized inputs that produced it.
- `build.rn` performs post-lock build orchestration.

The critical rule is:

- `craft.rn` may affect dependency resolution and therefore lockfile contents.
- `build.rn` may affect execution, staging, and linkage.
- `build.rn` must not affect dependency resolution or lockfile contents.

## Package And Workspace Model

`craft` treats workspaces as first-class.

- one workspace root owns the shared `Craft.lock`
- workspace members resolve together
- local members participate as explicit packages in the resolved graph
- package-level elaboration happens within explicit workspace context

The repository layout keeps `craft` outside `compiler/`:

```text
compiler/
library/
tools/
  craft/
docs/
```

This is intentional. `craft` is not a compiler pass. It is a top-level toolchain manager that drives the compiler.

## `Craft.toml`

`Craft.toml` is the static declaration source.

It should remain readable and sufficient for ordinary packages. Most packages should not need either script file.

The current schema direction includes:

- `[package]`
- `[craft]`
- `[runtime]`
- `[lib]`
- `[[bin]]`
- `[test]`
- `[example]`
- `[dependencies]`
- `[dev-dependencies]`
- `[build-dependencies]`
- `[features]`
- `[profile.dev]`
- `[profile.release]`
- `[workspace]`
- `[workspace.package]`
- `[workspace.dependencies]`

Example:

```toml
[package]
name = "http"
version = "0.1.0"
kern = "0.6.7"
publish = false

[runtime]
entry = "rt"
libc = false
bundle = "std"

[lib]
root = "src/lib.rn"

[[bin]]
name = "http-cli"
root = "src/main.rn"

[dependencies]
net = { git = "https://example.com/net.git", tag = "v1" }
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
lto = "thin"
```

Manifest rules:

- targets are explicit
- `[package].kern` must match the current installed Kern toolchain version exactly
- `Craft.toml` does not expose an `edition` field before Kern 1.0
- `[runtime]` is the package-level place to declare startup/library policy
- `[runtime].entry` controls startup ownership only
- `[runtime].libc` controls libc linkage only
- `[runtime].bundle` controls which official library root aliases are added
- `[runtime].bundle` is alias wiring, not a scope prelude; ordinary `use` still applies
- `sys`/`rt` implementation choice belongs to normal package/module wiring, not a manifest runtime-provider selector
- test targets are listed under `[test].roots`, and each test name is derived from its file stem
- example targets are listed under `[example].roots`, and each example name is derived from its file stem
- external dependencies must be explicit `path` or `git` entries; plain version strings are not a source model
- `build-dependencies` belong to the host build domain rather than the final target domain
- features are additive
- profile behavior is deterministic
- profile optimization policy is explicit: `opt`, `debug`, `codegen-units`, and `lto` are separate knobs
- target-domain `lto = "thin"` keeps compile actions in ThinLTO linker-input form until the final link step, so cross-package optimization is preserved instead of being collapsed inside each package
- target-specific or feature-specific elaboration belongs in either explicit manifest tables or `craft.rn`

## Publish Readiness

`craft publish` is a local release-readiness check. It does not upload
anywhere, talk to a registry, or rewrite dependency state.

The current rules are:

- `craft publish` always evaluates the release profile
- `craft publish` requires a current release `Craft.lock`
- if the release lock is missing or stale, `craft publish` fails and tells the
  user to run `craft lock --profile release`
- `craft publish` does not silently create or refresh `Craft.lock`

Required package metadata for a publishable package is:

- `description`
- `license`
- `authors`
- `readme`
- `repository`

These values may be declared directly under `[package]`. Workspace defaults may
also be placed in `[workspace.package]` for shared package metadata. If
`readme` comes from `[workspace.package]`, it is resolved relative to the
workspace root. A package may opt out entirely with `publish = false`.

## Feature, Profile, And Command Inputs

Feature selection is part of elaboration and build planning, not an afterthought.

The current command surface accepts:

- `--project-path <path>`
- `--profile <dev|release>`
- `--no-default-features`
- `--features <a,b,c>`

These inputs affect:

- manifest discovery
- `craft.rn` evaluation
- normalized package targets and dependencies
- lockfile freshness
- build plan derivation

Profiles and command mode are also explicit inputs. `craft.rn` and `build.rn` both receive:

- host information
- target information
- profile information
- command mode such as `check`, `build`, `run`, or `test`

This keeps the system orthogonal: command selection, feature selection, profile selection, host context, and target context are all ordinary inputs to explicit phases.

## `craft.rn`

`craft.rn` is an optional pre-resolution elaboration script.

It exists because pure TOML is good at declaration but weak at structured conditional adaptation. Rather than inflate the manifest format, `craft` allows a bounded Kern phase that elaborates package structure before resolution.

Conceptually, `craft.rn` works on package planning state:

- package metadata from `Craft.toml`
- workspace metadata
- host target
- final target
- profile
- command mode
- selected features

Its job is to elaborate the package graph, not to execute a build.

Example:

```kern
use craft.plan;

pub fn craft(p: *mut plan.Plan) void {
    if (p.target.os == .windows) {
        p.dep("win32", "1");
    }

    p.define_string("host_arch", p.host.arch);

    if (p.feature_enabled("simd")) {
        p.cfg_bool("simd", true);
    }

    if (p.profile.name == "release") {
        p.define_bool("aggressive_checks", false);
    }
}
```

### What `craft.rn` May Do

- add or remove target-specific dependencies
- elaborate feature-driven configuration
- add package-local cfg/define values
- adjust or add targets owned by the current package
- choose source roots
- apply workspace policy explicitly

### What `craft.rn` Must Not Do

- perform network access
- depend on wall-clock time
- use randomness
- inspect incidental host state implicitly
- trigger compilation or linking
- mutate the dependency graph after lock resolution

`craft.rn` is part of the lock input. It is therefore required to be deterministic.

## Explicit Environment Inputs

Some real projects need environment-based adaptation. `craft` supports that only through explicit declaration.

The model is:

- `Craft.toml` declares allowed environment inputs under `[craft]`
- `craft.rn` may read only those named inputs
- the values that participated in elaboration are recorded into `Craft.lock`

Example:

```toml
[craft]
env = ["USE_SYSTEM_SSL", "KERN_SYSROOT"]
```

This avoids hidden host dependence. If an allowed input changes, the lockfile becomes stale and `craft` must re-elaborate before reuse.

## `Craft.lock`

`Craft.lock` records the resolved graph and the normalized inputs that affect that graph.

It exists to answer:

- which packages were selected
- where they came from
- which dependency edges were chosen
- which normalized targets existed
- which elaboration inputs produced that graph

The current lockfile model records:

- manifest path and digest
- workspace `craft.rn` path and digest, when present
- package manifests and digests
- package `craft.rn` paths and digests, when present
- normalized package targets
- resolved external packages
- dependency edges
- declared environment inputs used by workspace or package elaboration

It intentionally does not record post-lock build execution details from `build.rn`.

Lockfile responsibilities:

- reproducible dependency resolution
- workspace-wide graph stability
- offline rebuild support
- auditability of elaboration inputs

Lockfile non-responsibilities:

- recording every object or artifact hash
- acting as a generic build cache index
- encoding post-link packaging behavior

## Dependency Sources

The source model is deliberately simple.

Current direction:

- path dependencies
- git dependencies

Build-domain rule:

- `dependencies` and `dev-dependencies` are target-domain edges
- `build-dependencies` are host-domain edges
- build-domain edges must not be silently merged into target compile inputs

Every resolved dependency must have a stable source identity that can be recorded in the lockfile.

## `build.rn`

`build.rn` is an optional post-lock build script.

Its role is build orchestration, not package elaboration.

It runs after resolution and after lock derivation, and it is allowed to affect:

- generated sources
- staged resource handling
- link directives
- target-local compile configuration
- artifact layout

It is not allowed to affect:

- dependency resolution
- package identity
- lockfile contents
- workspace topology

Example:

```kern
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    b.define_string("host_arch", b.host.arch);

    let generated = b.copy_package_file("templates/main.rn", "src/main.rn");
    let generated_from_tool =
        b.emit_generated_from_tool("codegen", "codegen", "src/generated.rn", .{});
    let _ = b.copy_package_file_to_artifact("assets/config.json", "config/config.json");
    let _ = b.copy_package_dir_to_artifact("assets", "bundle/assets");
    let _ = b.emit_artifact_file("notes/build.txt", "built by craft\n");

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

Important:

- generated source files participate in compilation through the generated root
- `build.rn` currently replaces a unit source root; it does not implicitly add sibling modules into the original package `src/` tree
- if you want `mod build_info;` to resolve against a generated file, copy or generate the entry source under the generated root as well, then bind that output with `set_source_root(...)` or `set_source_root_from(...)`

## Generated Files And Staged Actions

`build.rn` does not work by mutating the filesystem invisibly during planning.

Instead, it records explicit plan data:

- `generated_files`
- `staged_actions`

These are different concepts.

Generated files:

- belong to the designated generated source area
- exist to participate in compilation
- can replace the source root for a unit

Staged actions:

- are explicit execution-phase file operations
- target either the generated area or the final artifact area
- remain visible in the derived build plan and CLI output
- are bound to units as either `compile_inputs` or `artifact_outputs`

The staged action model currently has two explicit phases:

- `pre_compile`
- `post_link`

This phase split matters:

- `pre_compile` actions materialize inputs required before compiling the unit
- `post_link` actions materialize files or directories that belong next to the final artifact

The current staged action kinds are:

- `WriteFile`
- `RunTool`
- `CopyFile`
- `CopyDirectory`

This keeps build behavior explicit and inspectable instead of hiding it behind arbitrary script side effects.

## Current `build.rn` Capability Surface

The current `Builder` API includes:

- package, workspace, target, profile, command, and unit inspection
- explicit host/target inspection for cross compilation aware logic
- access to derived paths:
  - build root
  - generated root
  - artifact root
  - object path
  - artifact path
  - optional metadata path
- feature queries
- unit-domain inspection
- compile-time mutation:
  - cfg bool/string
  - define bool/string
  - source-root override
  - source-root binding from explicit outputs
- generated source production:
  - `stage_generated(...)`
  - `stage_copy_package_file(...)`
  - `stage_copy_output(...)`
  - `tool_path(dependency, tool)`
  - `stage_generated_from_tool(dependency, tool, ...)`
- post-link artifact staging:
  - `stage_artifact_file(...)`
  - `stage_artifact_file_from_tool(dependency, tool, ...)`
  - `stage_copy_package_file_to_artifact(...)`
  - `stage_copy_package_dir_to_artifact(...)`
- graph composition:
  - `output_path(output)`
  - `set_source_root_from(output)`
  - `depend(output, dependency)`
- link directives:
  - `link_system_lib(...)`
  - `link_framework(...)`
  - `link_search(...)`
  - `link_arg(...)`

The important property is not API breadth by itself. The important property is that these effects are represented in the build plan rather than being hidden behavior.

Current domain behavior:

- ordinary package units are target-domain units
- `build.rn` executes with both host and target context available
- `build-dependencies` are tracked separately from target unit dependencies so build-time tools do not pollute the final target graph
- local and external `build-dependencies` that expose binaries can be resolved as explicit tools inside `build.rn`
- tool-driven file generation is represented as explicit staged nodes with declared dependencies rather than opaque script-side process execution

## Build Plan And Execution

`craft` derives an explicit build plan before doing execution work.

The plan contains:

- packages
- build units
- compile actions
- link actions
- local and external dependency edges
- generated files
- staged actions
- link directives

Execution then consumes that plan in order.

- pre-compile staged actions are materialized before compilation
- compile actions invoke `kernc`
- link actions invoke the linker path through `kernc_driver`
- post-link staged actions are materialized after the final artifact exists

This is why `craft check` remains meaningful: it can inspect, validate, and print the derived graph without collapsing the system into an opaque build script.

## Interaction With `kernc`

`craft` treats `kernc` as a compiler driver, not as a package manager.

For each derived unit, `craft` supplies explicit inputs such as:

- source root
- target kind
- profile
- cfg/define values
- metadata output path
- link inputs
- linker libraries
- linker search paths
- raw linker arguments

This keeps package management, graph resolution, and code generation cleanly separated.

## Command Surface

The current command surface is intentionally narrow:

- `craft help`
- `craft check`
- `craft lock`
- `craft fetch`
- `craft build`
- `craft run`
- `craft test`

Current behavior:

- `check` loads the package graph, evaluates scripts, derives the build plan, materializes staged inputs, and runs semantic analysis for every selected compile unit without codegen or final linking
- `lock` writes a deterministic `Craft.lock`
- `fetch` materializes external package sources into the local cache
  - source backends are explicit package paths or git repositories
- `build` executes the selected build plan
- `run` builds and runs the selected runnable binary from its owning package root
- `test` builds and runs test targets from their owning package roots

When `craft` launches a runtime target for `run` or `test`, it also injects:

- `CRAFT_WORKSPACE_ROOT`
- `CRAFT_PACKAGE_ROOT`

`check` also reports:

- feature inputs
- workspace/package script presence
- environment input counts
- normalized target counts
- dependency counts
- build-unit and action counts
- generated files
- resolved source roots (`package`, `absolute`, or `build_output`)
- unit-bound `compile_inputs` and `artifact_outputs`
- link directives
- lockfile freshness

That audit output is part of the design, not decoration.

## Design Rules

The system should continue to follow these rules:

- no implicit host-environment dependence
- no hidden pre-lock side effects
- no `build.rn` influence on resolution
- no silent mutation of dependency topology after locking
- explicit build edges over ad hoc script behavior
- readable lockfiles over opaque hashes alone

Where behavior must vary, it should vary through explicit inputs and explicit phases.

## Implementation Status

The current implementation already includes:

- manifest parsing and validation
- workspace discovery
- package graph normalization
- `craft.rn` evaluation
- feature-aware elaboration
- explicit environment input tracking for elaboration
- deterministic lockfile generation and freshness checks
- source fetching for external packages
- explicit build-plan derivation
- `build.rn` execution through generated files and staged actions
- compile/link execution through `kernc_driver`
- build/test/run command flow through the same planning model

This means `craft` is already structured around the intended architecture rather than being a temporary script runner.

## Expansion Areas

The remaining work should extend this model, not bypass it.

Natural next steps include:

- richer git and workspace flows
- more complete workspace ergonomics
- additional explicit build-graph nodes where justified
- improved packaging and install flows
- stronger cache/index modeling around the same deterministic plan structure

The standard for future additions is simple: if a capability cannot be expressed cleanly with explicit inputs, explicit phases, and explicit graph effects, it should not be added.
