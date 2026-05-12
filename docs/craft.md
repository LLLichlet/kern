# The `craft` Package Manager and Builder

This document describes the current architecture and operating model of
`craft`, the Kern package manager and build orchestrator.

`craft` is intentionally separate from `kernc`.

- `kernc` compiles and links explicit inputs.
- `craft` discovers packages, evaluates package configuration, resolves dependencies, manages lockfiles, derives build plans, and executes those plans.

`craft` follows these design rules:

- orthogonality
- explicit phase boundaries
- deterministic behavior
- auditability
- low policy, high clarity

## Start Here

If you just want to use `craft` effectively, these are the commands that matter
first:

```bash
craft check
craft build
craft run
craft test
craft clean
```

Common target-selection variants are:

```bash
craft build --project-path path/to/workspace
craft build --profile release
craft build --examples
craft run --bin my-tool
craft run --example smoke
craft clean --project-path path/to/workspace
```

The practical split is:

- use `craft` when the question is "which package or target should be built?"
- use `kernc` when the question is "what exact compile or link command should happen?"
- use `craft clean` when derived `.craft/` build, fetch, staging, or analysis
  state should be removed without deleting project sources.

`craft init` creates the smallest ordinary single-package project:

```bash
mkdir hello
cd hello
craft init
```

The generated shape is intentionally simple:

```text
hello/
  Craft.toml
  Craft.lock
  src/
    main.rn
```

Use this shape for ordinary applications, tools, and small libraries. Move to a
workspace only when the repository needs more than one package, shared
dependency declarations, or a package namespace that exports selected members
to external users.

## Freestanding Kernel Package

For a minimal freestanding package, keep startup ownership explicit in
`Craft.toml`:

```toml
[package]
name = "kernel"
version = "0.1.0"
kern = "0.7.5"

[runtime]
entry = "none"
libc = false
bundle = "base"

[[bin]]
name = "kernel"
root = "src/main.rn"
```

The source file may export `_start` directly:

```kern
#[export_name("_start")]
fn kmain() void {
    while (true) {}
    @unreachable();
}
```

For a single-package project whose linker script sits next to `Craft.toml`,
`build.rn` can attach it to the final link:

```kern
use craft.builder;

pub fn build(b: &mut builder.Builder) void {
    b.link_config(.{
        system_libs: .{},
        frameworks: .{},
        search_paths: .{"native"},
        args: .{"-Wl,--gc-sections"},
        arg_paths: .{.{ flag: "-T", path: "kernel.ld" }},
    });
}
```

`build.rn` can also compile small C-family support files with the same C driver
resolution used by `kernc --cc`. The generated object is staged before Kern
compilation and added to the current target's final link:

```kern
use craft.builder;

pub fn build(b: &mut builder.Builder) void {
    let config = b.stage_generated("native_config.h", "#define KERNEL_BUILD 1\n");
    let _ = b.cc_config("native/support.c", .{
        include_dirs: .{"native/include", b.paths.generated_root},
        defines: .{"KERNEL_TARGET=1"},
        args: .{"-Wall"},
        dependencies: .{config},
    });
}
```

The default C driver is SDK-owned: when no C driver is explicitly selected,
`craft` uses the active Kern SDK/toolchain `clang`. It does not fall back to the
host `cc` when SDK clang is missing. Set `KERN_TOOLCHAIN_ROOT` to a valid SDK or
set `CC` only when you intentionally want an external C driver.

`cc_config.dependencies` is for generated headers or other staged pre-compile
outputs that the C source includes. Package include directories must already
exist; include directories under `b.paths.generated_root` may be produced by the
declared staged dependencies.

Then the ordinary workflow stays the same:

```bash
craft check
craft build
```

This is the important model:

- `entry = "none"` means the package owns startup itself
- `libc = false` means libc is not linked implicitly
- `bundle = "base"` keeps the library surface minimal and freestanding-oriented
- custom linker behavior belongs in `build.rn`, not in hidden tool defaults

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
derived state, not part of the reproducibility surface, and does not belong in
version control.

`craft` maintains a root `.gitignore` entry for `.craft/` next to `Craft.toml`
when it creates local state. The ignore rule belongs at the package or
workspace root rather than inside `.craft/`, because the derived-state
directory itself should be ignored as one unit.

If a repository already tracked files under `.craft/`, that is a one-time VCS
cleanup problem rather than a build reproducibility input, and those entries
are expected to be removed from the index.

`craft` is not responsible for:

- replacing the Kern language module system
- hiding `kernc` behind opaque behavior
- introducing implicit dependency or target policy
- allowing pre-lock scripts to smuggle in hidden machine or invocation state

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

When adding new `.craft/` state, the default policy is:

1. decide whether the state is workspace-wide, output-wide, or private to one
   action
2. if it is shared, define the lock scope explicitly
3. if it is a file, prefer atomic replacement
4. if it is a directory tree, avoid in-place destructive rewrites unless the
   tree is covered by a dedicated output lock

If a new feature cannot state its lock scope and replacement strategy clearly,
its state model is still underspecified.

## Path Identity

`craft` treats path identity as part of correctness, not presentation.

This matters especially on macOS. The system may expose temporary or workspace
paths as `/var/...`, while `canonicalize()` resolves the same location as
`/private/var/...`.

Those two strings often refer to the same directory tree, but they are not
textually equal. If one subsystem stores `/private/var/...` and another later
looks up `/var/...`, project discovery, workspace membership checks, cached
analysis context, and LSP project resolution can all fail even though the user
is still pointing at the same files.

The current rule is:

- Windows verbatim prefixes are stripped
- macOS `/private/var/...` paths are normalized to `/var/...`
- path comparisons inside `craft` and `kern-lsp` should happen only after that
  normalization

This is not a cosmetic rewrite. It is required so manifest discovery,
generated-source tracking, persisted analysis state, and LSP document URIs all
share one stable path identity.

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

This split is a core part of the design.

- `Craft.toml` carries static declarations.
- `craft.rn` normalizes those declarations before resolution, but only from lock-stable inputs.
- `Craft.lock` records the canonical resolved graph for the workspace.
- `build.rn` performs post-lock build orchestration.

The critical rule is:

- `craft.rn` may affect dependency resolution and therefore lockfile contents,
  but only from checked-in, lock-stable inputs.
- `build.rn` may affect execution, staging, and linkage.
- `build.rn` must not affect dependency resolution or lockfile contents.

## Package And Workspace Model

`craft` has two manifest shapes:

- a package manifest: a `Craft.toml` with `[package]`
- a workspace manifest: a root `Craft.toml` with `[workspace]`

A manifest must not contain both `[package]` and `[workspace]`. A workspace root
is a namespace and coordination manifest; buildable packages live in listed
members. This keeps one obvious place for each concept:

- package identity, targets, runtime, resources, and package-local dependencies
  live in member `[package]` manifests
- workspace membership, shared dependency declarations, shared package metadata,
  and external exports live in the root `[workspace]` manifest

`craft` treats workspaces as first-class:

- one workspace root owns the shared `Craft.lock`
- workspace members resolve together
- local members participate as explicit packages in the resolved graph
- package-level elaboration happens within explicit workspace context
- external users see the workspace through its declared export namespace

The repository layout keeps `craft` outside `compiler/`:

```text
compiler/
library/
tools/
  craft/
docs/
```

This separation is intentional. `craft` is not a compiler pass. It is a top-level toolchain manager that drives the compiler.

## `Craft.toml`

`Craft.toml` is the static declaration source.

It stays readable and sufficient for ordinary packages. Most packages do not
need either script file.

The schema includes:

- `[package]`
- `[craft]`
- `[craft.style]`
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
- `[workspace.exports]`
- `[workspace.dependencies]`

Example:

```toml
[package]
name = "http"
version = "0.1.0"
kern = "0.7.5"
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

[craft.style]
suggestions = "info"
disabled-rules = []
exclude = []

[profile.dev]
opt = 0
debug = true

[profile.release]
opt = 3
debug = false
```

Manifest rules:

- targets are explicit
- `[package].kern` must match the current installed Kern toolchain version exactly
- `[package].name` is always package-local; workspace inheritance never names a member package
- a root `Craft.toml` cannot be both a package and a workspace
- `Craft.toml` does not expose an `edition` field before Kern 1.0
- `[runtime]` is the package-level place to declare startup/library policy
- `[runtime].entry` controls startup ownership only
- `[runtime].libc` controls libc linkage only
- `[runtime].bundle` controls which official library root aliases are added
- `[runtime].bundle` is alias wiring, not a scope prelude; ordinary `use` still applies
- without an explicit `[runtime]` override, `craft` keeps runnable targets on the pure-first default (`entry = "rt"`, `libc = false`, `bundle = "std"`)
- `rt` implementation choice belongs to normal package/module wiring, not a manifest runtime-selection field
- test targets are listed under `[test].roots`, and each test name is derived from its file stem
- example targets are listed under `[example].roots`, and each example name is derived from its file stem
- external dependencies must be explicit `path` or `git` entries; plain version strings are not a source model
- `build-dependencies` belong to the host build domain rather than the final target domain
- features are additive
- profile behavior is deterministic
- profile optimization policy is explicit: `opt`, `debug`, `codegen-units`, and `lto` are separate knobs
- default `release` keeps LTO off unless the manifest opts in explicitly
- target-domain `lto = "thin"` keeps compile actions in ThinLTO linker-input form until the final link step, so cross-package optimization is preserved instead of being collapsed inside each package
- `[craft.style]` configures advisory source-style analysis only; it does not
  change compilation or formatting
- `[craft.style].suggestions` accepts `info`, `warn`, or `off`
- `[craft.style].disabled-rules` may contain `index-while`,
  `long-postfix-chain`, or `repeated-borrow-receiver`
- `[craft.style].exclude` matches package-relative path prefixes, with
  `/**` accepted for subtree notation
- declarative package-graph structure belongs in `Craft.toml` plus lock-stable `craft.rn`
- invocation-sensitive adaptation belongs in `build.rn`

## Workspace Namespaces

A workspace root is a namespace manifest. It names the project, lists member
packages, and declares which member packages are visible to external consumers.

Example library workspace:

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
description = "JSON parsing and document utilities for Kern"
license = "MIT"
authors = ["Example <dev@example.com>"]
readme = "README.md"
repository = "https://example.com/json-kern.git"

[workspace.dependencies]
json = { path = "json" }
```

The member package remains a normal package manifest:

```toml
[package]
name = "json"
publish = true

[lib]
root = "src/lib.rn"
```

The test and benchmark tools can stay private to the workspace:

```toml
[package]
name = "json-test"
publish = false
description = "Extended conformance tests for json-kern"

[[bin]]
name = "json-test"
root = "src/main.rn"

[dependencies]
json = { workspace = true }
```

The workspace root controls external exports. Internal members are not exported
just because they are listed in `members`.

Multiple exports are explicit:

```toml
[workspace.exports]
json = { member = "json" }
json-schema = { member = "json-schema" }
```

External users depend on the workspace source and select an export. If the local
dependency name is the same as the export name, no extra selector is needed:

```toml
[dependencies]
json = { git = "https://example.com/json-kern.git", tag = "v0.1.0" }
```

If the local dependency name should differ from the export name, use `export`:

```toml
[dependencies]
kern_json = { git = "https://example.com/json-kern.git", tag = "v0.1.0", export = "json" }
```

`export` is the dependency selector for workspace namespaces. It answers "which
exported package from that source should this dependency edge use?" It is not a
source alias and it is not a module import. Source files still use Kern `use`
statements to import names from the dependency's public module API.

`[workspace.package]` is the shared package-metadata table for a workspace.
Member packages may receive workspace defaults for:

- `version`
- `kern`
- `description`
- `license`
- `authors`
- `readme`
- `repository`
- `homepage`
- `documentation`

`version` and `kern` participate in member package identity and validation when
the member omits them. `description`, `license`, `authors`, `readme`, and
`repository` are used as defaults for publish-readiness checks and publish
proofs. `homepage` and `documentation` are accepted shared metadata fields for
the package surface. A member can override an inherited/defaulted field by
declaring it directly under its own `[package]`.

`publish` is deliberately not inherited. Each member declares its own release
intent with `publish = true` or `publish = false`.

`[workspace.dependencies]` is dependency declaration reuse for members:

```toml
[workspace.dependencies]
json = { path = "json" }

# member Craft.toml
[dependencies]
json = { workspace = true }
```

The member owns the dependency edge. The workspace only owns the shared source
declaration. A member can still add local feature choices on the inherited
edge.

## Publish Readiness

`craft publish` is a local release-readiness check. It does not upload
anywhere, talk to a registry, or rewrite dependency state.

The current rules are:

- `craft publish` always evaluates release-mode publish readiness
- `craft publish` requires the package to be inside a Git worktree with a
  resolvable `HEAD`
- the Git worktree must be clean before release checks run
- the canonical `Craft.lock` must already exist and be committed
- after release graph resolution, `Craft.lock` must still be current; if it
  would be created or updated, publish fails without rewriting the lockfile
- each publishable package's `repository` URL must match a configured Git
  remote after normalizing common HTTPS and SSH GitHub forms
- each publishable package must have a current publish proof in `Craft.lock`
- `craft publish` runs deterministic source formatting checks and reports
  source-style and public-doc metrics without rewriting source files

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

`Craft.lock` records a distributed publish proof for each publishable package,
not a registry entry. The proof records the package name, version, Kern version,
repository URL, and SHA-256 digests for `Craft.toml` and the package source
tree. The source-tree digest excludes `.git`, `.craft`, and `Craft.lock` so the
lockfile can carry the proof without changing the digest it proves.

Git dependencies are verified automatically when they are fetched. A Git
dependency is rejected if it has no committed `Craft.lock` publish proof, if
the proof does not match the fetched package contents, if the proof metadata
does not match the requested package and version, or if the proof repository
does not match the fetched Git source. This is the default ecosystem boundary
for Git packages; callers do not opt in to it with a local policy flag.

## Resolution And Execution Inputs

`craft` must not treat all command-line inputs as one undifferentiated planning
surface.

The command surface accepts:

- `--project-path <path>`
- `--profile <dev|release>`
- `--root <path>` for install roots
- `--bin <name>` for selecting one installed runnable target
- `--no-default-features`
- `--features <a,b,c>`

These inputs do not all belong to the same phase.

Resolution inputs determine the canonical workspace graph and therefore belong
to `Craft.lock`.

Execution inputs determine how one already-resolved graph is checked, built,
run, or tested.

The intended split is:

- manifest discovery affects resolution
- checked-in package/workspace declarations affect resolution
- `craft.rn` affects resolution, but only through lock-stable inputs
- selected profile affects execution
- command mode affects execution
- selected CLI feature sets affect execution
- host- and machine-local state affect execution, not canonical resolution

This keeps the system orthogonal: `Craft.lock` is the shared resolution
artifact, while profile and command mode are execution concerns layered on top
of it.

## `craft.rn`

`craft.rn` is an optional pre-resolution normalization script.

It exists because pure TOML is good at declaration but weak at structured
normalization. Rather than inflate the manifest format, `craft` allows a
bounded Kern phase that rewrites package planning state before resolution.

Because `craft.rn` contributes to `Craft.lock`, it must stay on canonical
resolution inputs only.

Conceptually, `craft.rn` works on lock-stable package planning state:

- package metadata from `Craft.toml`
- workspace metadata
- checked-in target declarations
- checked-in dependency declarations
- other checked-in package structure owned by the workspace

It does not receive:

- host target
- final target
- profile
- command mode
- process environment
- ad hoc CLI-selected feature state

Its job is to elaborate the canonical package graph, not to execute a build.

Example:

```kern
use craft.plan;

pub fn craft(p: &mut plan.Plan) void {
    if (p.package.is_root) {
        p.add_bin("tools", "src/tools.rn");
    }

    p.set_lib_root("src/lib.rn");
    p.cfg_bool("workspace_member", p.workspace.has_workspace);
}
```

Path semantics for `craft.rn` are intentionally display-oriented and
workspace-relative:

- `p.workspace.root` is the workspace root relative to itself, so it is `"."`
- `p.package.root` is the current package root relative to the workspace root
- for the root package, `p.package.root` is also `"."`
- these values are canonical lock inputs, not machine-local absolute paths

That split is important. `craft.rn` participates in elaboration and lock
derivation, so its root strings are stable across machines as long as the
workspace layout is the same.

### What `craft.rn` May Do

- normalize checked-in package structure deterministically
- add or remove dependency edges only from lock-stable checked-in inputs
- add package-local cfg/define values that are themselves lock-stable
- adjust or add targets owned by the current package
- choose source roots
- apply workspace policy explicitly

### What `craft.rn` Must Not Do

- inspect host, target, profile, command mode, or process environment
- perform network access
- depend on wall-clock time
- use randomness
- trigger compilation or linking
- mutate the dependency graph after lock resolution

`craft.rn` is part of the lock input. It is therefore required to be
deterministic and canonical across team members and CI.

## Environment And Canonical Resolution

Machine-local environment is not part of canonical resolution.

That means:

- `Craft.lock` must not vary with process environment
- pre-lock `craft.rn` must not branch on environment values
- machine-local adaptation belongs after lock, typically in `build.rn`

If a project needs environment-sensitive execution behavior, that behavior
should be modeled as post-lock build orchestration rather than as dependency
resolution.

## `Craft.lock`

`Craft.lock` records the canonical resolved graph for a workspace.

It is intended to be committed to version control and shared across developers
and CI.

It exists to answer:

- which packages were selected
- where they came from
- which dependency edges were chosen
- which normalized targets existed
- which canonical checked-in inputs produced that graph

The current lockfile model records:

- manifest path and digest
- workspace `craft.rn` path and digest, when present
- package manifests and digests
- package `craft.rn` paths and digests, when present
- normalized package targets
- resolved external packages
- dependency edges

It intentionally does not record post-lock build execution details from `build.rn`.

Lockfile responsibilities:

- reproducible dependency resolution
- workspace-wide graph stability
- one canonical team/CI snapshot for the resolved workspace graph
- offline rebuild support
- auditability of elaboration inputs

Lockfile non-responsibilities:

- recording every object or artifact hash
- acting as a generic build cache index
- encoding post-link packaging behavior
- tracking local invocation state such as profile, command mode, or environment

## Dependency Sources

The source model is deliberately simple.

Supported source forms:

- path dependencies
- git dependencies

Build-domain rule:

- `dependencies` and `dev-dependencies` are target-domain edges
- `build-dependencies` are host-domain edges
- build-domain edges must not be silently merged into target compile inputs

Every resolved dependency must have a stable source identity that can be recorded in the lockfile.

## Package Resources

Some build inputs are not `craft` packages.

Typical examples:

- bootloader trees such as `limine`
- linker scripts or vendor configuration bundles
- prebuilt headers, templates, or runtime data copied into final artifacts

Those inputs belong in package-local `[resources]`, not in `build.rn` shell commands.

Example:

```toml
[resources]
limine = { git = "https://github.com/limine-bootloader/limine.git", tag = "v11.4.0-binary" }
assets = { path = "vendor/assets" }
```

Resource rules:

- resources are declared per package
- a resource must declare exactly one source backend: `path` or `git`
- git resources may use at most one selector: `rev`, `branch`, or `tag`
- resources are fetched and materialized by `craft`, not by arbitrary script code
- resource declarations participate in source-policy checks
- resource declarations are recorded in `Craft.lock`

Fetch/materialization rules:

- `path` resources resolve relative to the owning package root
- `git` resources use the same cached clone/materialization model as external package sources
- fetched resource trees are materialized under `.craft/resources/`
- repeated `craft fetch` / `craft build` reuses unchanged resource trees instead of recloning or recopying them

This keeps remote source acquisition auditable, cacheable, and reproducible without turning `build.rn` into a general shell escape hatch.

## `build.rn`

`build.rn` is an optional post-lock build script.

Its role is build orchestration, not package elaboration.

It runs after resolution and after lock derivation, and it is allowed to affect:

- machine-local adaptation
- host- and target-specific execution behavior
- profile-sensitive execution behavior
- command-sensitive execution behavior
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
- network fetch behavior
- arbitrary host-side process orchestration

Example:

```kern
use craft.builder;

pub fn build(b: &mut builder.Builder) void {
    b.define_string("host_arch", b.host.arch);

    let generated = b.copy_package_file("templates/main.rn", "src/main.rn");
    let generated_from_tool =
        b.emit_generated_from_tool("codegen", "codegen", "src/generated.rn", .{});
    let artifact = b.primary_artifact();
    let _ = b.copy_output_to_artifact(artifact, "bundle/app");
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
- `build.rn` replaces a unit source root; it does not implicitly add sibling modules into the original package `src/` tree
- if you want `mod build_info;` to resolve against a generated file, copy or generate the entry source under the generated root as well, then bind that output with `set_source_root(...)` or `set_source_root_from(...)`

### `build.rn` Path Semantics

`build.rn` path semantics are intentionally different from `craft.rn`.

- `b.workspace.root` is an absolute normalized path to the workspace root
- `b.package.root` is an absolute normalized path to the current package root
- for a workspace member package, those two values differ
- for the workspace root package, they are the same absolute path

This is deliberate. `build.rn` runs in the execution-oriented phase, so it
needs direct filesystem coordinates rather than lock-stable display strings.

Derived execution paths are also absolute:

- `b.paths.build_root`
- `b.paths.generated_root`
- `b.paths.artifact_root`
- `b.paths.object`
- `b.paths.artifact`
- `b.paths.metadata` when present

Relative path rules:

- `set_source_root("src/main.rn")` resolves from the current package root
- `set_source_root("/abs/path/to/file.rn")` keeps the absolute path as given
- `set_source_root_from(output)` is the preferred way to bind staged generated outputs
- `link_search("native")` records a relative search path in the plan, then resolves it from the current package root during execution
- `link_search("/abs/path")` keeps the absolute search path as given
- `link_arg_path("-T", "link/kernel.ld")` resolves the path from the current package root, validates that it exists, records the normalized final path, and tracks that file as a real link input
- `resource_root("limine")` returns the absolute fetched root for the declared resource
- `resource_path("limine", "cfg/limine.conf")` resolves a path inside that fetched resource root and returns an absolute normalized path

Package-relative staging rules:

- `copy_package_file(...)` and `stage_copy_package_file(...)` read from the package root
- `copy_package_file_to_artifact(...)` and `copy_package_dir_to_artifact(...)` also read from the package root
- `copy_resource_file_to_artifact(...)` and `copy_resource_dir_to_artifact(...)` read from fetched resource roots
- `primary_artifact()` is only available on executable units and returns that unit's linked primary artifact as an output handle
- `copy_output_to_artifact(...)` copies an output handle into the post-link artifact tree for an executable unit
- generated destination paths are relative to `b.paths.generated_root`
- artifact destination paths are relative to `b.paths.artifact_root`

The practical rule is simple:

- use `craft.rn` roots for canonical workspace-relative elaboration
- use `build.rn` roots and `b.paths.*` for real filesystem work

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
- treat the generated root as build-owned state rather than a historical cache of every old emitted file

Staged actions:

- are explicit execution-phase file operations
- target either the generated area or the final artifact area
- remain visible in the derived build plan and CLI output
- are bound to units as either `compile_inputs` or `artifact_outputs`
- treat the artifact root as build-owned state rather than an append-only cache directory

The staged action model has two explicit phases:

- `pre_compile`
- `post_link`

This phase split matters:

- `pre_compile` actions materialize inputs required before compiling the unit
- `post_link` actions materialize files or directories that belong next to the final artifact

The staged action kinds are:

- `WriteFile`
- `CcCompile`
- `RunTool`
- `CopyFile`
- `CopyDirectory`

Dependency edges added with `depend(output, dependency)` are not just ordering hints:

- dependencies are executed before the dependent staged action
- dependency outputs are tracked as staged-action inputs for cache invalidation
- changing an upstream staged file reruns downstream post-link tools that consume it indirectly
- files under an executable's artifact root that are no longer planned staged outputs are deleted before current post-link staging runs
- files under a unit's generated root that are no longer planned pre-compile outputs are deleted before current pre-compile staging runs
- staged outputs within one unit and one phase must not reuse or overlap each other's paths

This keeps build behavior explicit and inspectable instead of hiding it behind arbitrary script side effects.

## `build.rn` Capability Surface

The `Builder` API includes:

- package, workspace, target, profile, command, and unit inspection
- explicit host/target inspection for cross compilation aware logic
- access to derived paths:
  - build root
  - generated root
  - artifact root
  - object path
  - artifact path
  - optional metadata path
- output handle inspection:
  - `primary_artifact()`
  - `output_path(output)`
- feature queries
- unit-domain inspection
- compile-time mutation:
  - cfg bool/string
  - define bool/string
  - source-root override
  - source-root binding from explicit outputs
- link-plan mutation:
  - `link_config(options)` for structured system libraries, frameworks, search
    paths, raw args, and `arg_paths: &[LinkArgPath]`
  - `link_system_lib(...)`, `link_framework(...)`, `link_search(...)`,
    `link_arg(...)`, and `link_arg_path(...)` as focused convenience helpers
- generated source production:
  - `stage_generated(...)` and `emit_generated(...)`
  - `stage_copy_package_file(...)` and `copy_package_file(...)`
  - `stage_copy_output(...)` and `copy_output(...)`
  - `cc(source, args)` for simple package-local C-family sources
  - `cc_config(source, options)` for C-family sources with structured
    `include_dirs`, `defines`, raw `args`, and staged `dependencies`
  - `tool_path(dependency, tool)`
  - `resource_root(name)` and `resource_path(name, relative_path)`
  - `stage_generated_from_tool(dependency, tool, ...)` and `emit_generated_from_tool(dependency, tool, ...)`
- post-link artifact staging:
  - executable units only
  - `stage_artifact_file(...)` and `emit_artifact_file(...)`
  - `stage_artifact_file_from_tool(dependency, tool, ...)` and `emit_artifact_file_from_tool(dependency, tool, ...)`
  - `stage_copy_output_to_artifact(output, ...)` and `copy_output_to_artifact(output, ...)`
  - `stage_copy_package_file_to_artifact(...)` and `copy_package_file_to_artifact(...)`
  - `stage_copy_package_dir_to_artifact(...)` and `copy_package_dir_to_artifact(...)`
  - `stage_copy_resource_file_to_artifact(name, ...)` and `copy_resource_file_to_artifact(name, ...)`
  - `stage_copy_resource_dir_to_artifact(name, ...)` and `copy_resource_dir_to_artifact(name, ...)`
- graph composition:
  - `output_path(output)`
  - `set_source_root_from(output)`
  - `depend(output, dependency)`
- link directives:
  - `link_config(...)`
  - `link_system_lib(...)`
  - `link_framework(...)`
  - `link_search(...)`
  - `link_arg_path(flag, path)`
  - `link_arg(...)`

The important property is not API breadth by itself. The important property is that these effects are represented in the build plan rather than being hidden behavior.

Domain behavior:

- ordinary package units are target-domain units
- `build.rn` executes with both host and target context available
- `build-dependencies` are tracked separately from target unit dependencies so build-time tools do not pollute the final target graph
- local and external `build-dependencies` that expose binaries can be resolved as explicit tools inside `build.rn`
- tool-driven file generation is represented as explicit staged nodes with declared dependencies rather than opaque script-side process execution
- `build.rn` is the correct place for host/target/profile/command-sensitive adaptation because those inputs do not belong to canonical resolution

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

The command surface is intentionally narrow:

- `craft help`
- `craft check`
- `craft fetch`
- `craft publish`
- `craft doc`
- `craft fmt`
- `craft style`
- `craft build`
- `craft install`
- `craft uninstall`
- `craft run`
- `craft test`

Command behavior:

- `init` creates `Craft.toml`, package stubs when needed, `.gitignore`, and the initial `Craft.lock`
- `check` loads the package graph, auto-synchronizes `Craft.lock`, evaluates scripts, derives the build plan, materializes staged inputs, and runs semantic analysis for every selected compile unit without codegen or final linking
- `fetch` auto-synchronizes `Craft.lock`, then materializes both external package sources and declared package resources into the local cache
  - package source backends are explicit package paths or git repositories
  - resource source backends are explicit package-relative paths or git repositories
- `publish` requires a clean Git worktree with committed `Craft.lock`, a
  current lockfile publish proof, and a matching repository remote, then runs
  release-oriented metadata, source-policy, format, style, and public-doc checks
  without uploading anywhere
- `doc` auto-synchronizes `Craft.lock`, builds the selected package graph, and renders Markdown package docs under `.craft/docs`
- `fmt` normalizes Kern source text deterministically by removing trailing
  horizontal whitespace and enforcing final-newline consistency
- `style` reports source metrics, public-doc coverage, and configurable
  advisory style suggestions without mutating source files
- `build` auto-synchronizes `Craft.lock` and executes the selected build plan
- `install` auto-synchronizes `Craft.lock`, builds selected package `bin` targets, and copies them into the active install root's `bin/` directory
- `uninstall` auto-synchronizes `Craft.lock` and removes installed package `bin` targets from that same install root
- `run` auto-synchronizes `Craft.lock`, then builds and runs the selected runnable binary from its owning package root
- `test` auto-synchronizes `Craft.lock`, then builds and runs test targets from their owning package roots

When `craft` launches a runtime target for `run` or `test`, it also injects:

- `CRAFT_WORKSPACE_ROOT`
- `CRAFT_PACKAGE_ROOT`

`check` also reports:

- feature inputs
- workspace/package script presence
- normalized target counts
- dependency counts
- build-unit and action counts
- generated files
- resolved source roots (`package`, `absolute`, or `build_output`)
- unit-bound `compile_inputs` and `artifact_outputs`
- link directives
- lockfile synchronization result

That audit output is part of the design, not decoration.

## Operating Rules

The system follows these rules:

- no implicit host-environment dependence
- no hidden pre-lock side effects
- no profile- or command-dependent lockfile variance
- no `build.rn` influence on resolution
- no network or shell side effects hidden inside `build.rn`
- no silent mutation of dependency topology after locking
- explicit build edges over ad hoc script behavior
- readable lockfiles over opaque hashes alone

Where behavior varies, it varies through explicit inputs and explicit phases.
