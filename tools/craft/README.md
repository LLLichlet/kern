# `craft`

`craft` is the Kern package manager and build orchestrator.

This directory intentionally lives under `tools/` rather than `compiler/`.

- `kernc` is the compiler/link driver.
- `craft` is the package graph resolver, automatic lockfile synchronizer,
  build planner, and command runner.

## Current Scope

The current implementation covers:

- `Craft.toml` discovery
- manifest parsing
- manifest validation
- `craft check`
- workspace discovery and validation
- workspace namespace exports through `[workspace.exports]`
- workspace package metadata inheritance through `[workspace.package]`
- local package graph construction
- resolved external package graph construction
- `craft.kn` discovery and pre-lock normalization scaffolding
- normalized package-plan snapshots for declared targets
- `workspace = true` dependency inheritance
- dependency export selection with `export = "..."`
- automatic deterministic canonical `Craft.lock` synchronization during `init` and package-graph commands
- release-oriented publish readiness checks via `craft publish`
- build-plan derivation from normalized package targets and resolved dependencies
- package-level `build.kn` discovery, validation, and structured per-target link-plan orchestration for execution-sensitive adaptation
- package-level `build.kn` C-family source compilation through the resolved SDK-first `kernc --cc` path with structured include directories, defines, and generated dependencies
- host `craft build/run/test` execution through explicit `kernc` compile/link action graphs
- `craft install/uninstall` for copying package `bin` targets into an install root
- `craft fetch` materialization of external sources into `.craft/sources`
- canonicalized workspace identity for shared `.craft` locks and state paths
- atomic writes for shared workspace state such as `Craft.lock` and `.craft/analysis.toml`
- output-scoped locking for shared metadata snapshot directories

## Core Commands

The current command surface is:

- `craft check`
- `craft fetch`
- `craft publish`
- `craft build`
- `craft install`
- `craft uninstall`
- `craft run`
- `craft test`

`craft` keeps package discovery decentralized by staying on concrete dependency
edges:

- local dependencies use `path`
- external dependencies use `git`
- selectors such as `branch`, `tag`, or `rev` stay on that dependency entry
- there is no registry table or source indirection layer

Derived tool state stays under `.craft/`, and `craft` maintains a root
`.gitignore` entry for `.craft/` next to `Craft.toml`.

`craft publish` is also local-only. It checks required package metadata and
auto-synchronizes `Craft.lock` as part of the normal command flow, but it does
not upload anywhere.

`craft install` defaults to the active Kern home (`KERN_HOME` or `~/.kern`) and
copies selected package binaries into `bin/`. `craft uninstall` removes those
installed binaries using the same package-target selection rules.

## Stress Tests

The workspace execution race check lives in the `workspace_concurrency`
integration test and is ignored by default because it intentionally spawns
concurrent `craft test` processes over copied workspaces:

```sh
cargo test -p craft --test workspace_concurrency -- --ignored
```

It accepts the same environment knobs as the retired shell script:
`CRAFT_STRESS_PROJECT`, `ROUNDS`, `JOBS`, and `KEEP_SUCCESS`.

## Important Internal Modules

- `src/manifest.rs` and `src/manifest/`: `Craft.toml` parsing and validation
- `src/workspace.rs` and `src/project/`: workspace/package discovery and path rules
- `src/elaborate.rs` and `src/script/`: pre-lock `craft.kn` and post-lock `build.kn` execution
- `src/graph.rs`, `src/resolver.rs`, and `src/source.rs`: package graph and
  source resolution
- `src/lockfile/`: `Craft.lock` parsing, rendering, validation, and build data
- `src/build_plan/`: explicit compile/link/build action derivation
- `src/execute/`: action execution, fingerprints, runtime packages, and parallel scheduling
- `src/analysis_context/`: `.craft/analysis.toml` parse/render/validation

`docs/craft.md` remains the high-level public architecture document. This
README is the tool-local index for the current implementation.
