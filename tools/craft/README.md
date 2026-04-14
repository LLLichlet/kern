# `craft`

`craft` is the Kern package manager and build orchestrator.

This directory intentionally lives under `tools/` rather than `compiler/`.

- `kernc` is the compiler/link driver.
- `craft` is the package graph resolver, lockfile manager, build planner, and
  command runner.

## Current Scope

The current implementation covers:

- `Craft.toml` discovery
- manifest parsing
- manifest validation
- `craft check`
- workspace discovery and validation
- local package graph construction
- resolved external package graph construction
- `craft.rn` discovery and elaboration-input scaffolding
- normalized package-plan snapshots for declared targets
- explicit `[craft].env` allowlists recorded into `Craft.lock`
- `workspace = true` dependency inheritance
- deterministic `Craft.lock` writing via `craft lock`
- `Craft.lock` loading, validation, and stale/current status reporting via `craft check`
- release-oriented publish readiness checks via `craft publish`
- build-plan derivation from normalized package targets and resolved dependencies
- package-level `build.rn` discovery, validation, and per-target link-plan orchestration
- host `craft build/run/test` execution through explicit `kernc` compile/link action graphs
- `craft fetch` materialization of external sources into `.craft/sources`
- canonicalized workspace identity for shared `.craft` locks and state paths
- atomic writes for shared workspace state such as `Craft.lock` and `.craft/analysis.toml`
- output-scoped locking for shared metadata snapshot directories

## Core Commands

The current command surface is:

- `craft check`
- `craft lock`
- `craft fetch`
- `craft publish`
- `craft build`
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

`craft publish` is also local-only. It checks for a current release
`Craft.lock` plus required package metadata, but it does not upload anywhere or
rewrite the lockfile implicitly.

## Important Internal Modules

- `src/manifest.rs` and `src/manifest/`: `Craft.toml` parsing and validation
- `src/workspace.rs` and `src/project/`: workspace/package discovery and path rules
- `src/elaborate.rs` and `src/script/`: `craft.rn` and `build.rn` execution
- `src/graph.rs`, `src/resolver.rs`, and `src/source.rs`: package graph and
  source resolution
- `src/lockfile/`: `Craft.lock` parsing, rendering, validation, and build data
- `src/build_plan/`: explicit compile/link/build action derivation
- `src/execute/`: action execution, fingerprints, runtime packages, and parallel scheduling
- `src/analysis_context/`: `.craft/analysis.toml` parse/render/validation

`docs/craft.md` remains the high-level public architecture document. This
README is the tool-local index for the current implementation.
