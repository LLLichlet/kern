# `craft`

`craft` is the planned Kern package manager and build orchestrator.

This directory intentionally lives under `tools/` rather than `compiler/`.

- `kernc` is the compiler driver.
- `craft` is the package graph resolver, lockfile manager, and build planner.

The current implementation now covers the first graph and lockfile milestones:

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
- build-plan derivation from normalized package targets and resolved dependencies
- package-level `build.rn` discovery, validation, and per-target link-plan orchestration
- host `craft build/run/test` execution through explicit `kernc` compile/link action graphs
- explicit `[source.<name>]` manifest configuration for directory-backed registries
- `craft fetch` materialization of external sources into `.craft/sources`

Current limitation:

- external registry dependencies can now be fetched, but are not yet recursively compiled into the execution graph

See [docs/craft.md](../../docs/craft.md) for the V1 design draft.
