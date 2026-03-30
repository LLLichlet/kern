# `kraft`

`kraft` is the planned Kern package manager and build orchestrator.

This directory intentionally lives under `tools/` rather than `compiler/`.

- `kernc` is the compiler driver.
- `kraft` is the package graph resolver, lockfile manager, and build planner.

The current implementation now covers the first graph and lockfile milestones:

- `Kraft.toml` discovery
- manifest parsing
- manifest validation
- `kraft check`
- workspace discovery and validation
- local package graph construction
- resolved external package graph construction
- `kraft.kr` discovery and elaboration-input scaffolding
- normalized package-plan snapshots for declared targets
- explicit `[kraft].env` allowlists recorded into `Kraft.lock`
- `workspace = true` dependency inheritance
- deterministic `Kraft.lock` writing via `kraft lock`
- `Kraft.lock` loading, validation, and stale/current status reporting via `kraft check`
- build-plan derivation from normalized package targets and resolved dependencies
- package-level `build.kr` discovery, validation, and per-target link-plan orchestration
- host `kraft build/run/test` execution through explicit `kernc` compile/link action graphs
- explicit `[source.<name>]` manifest configuration for directory-backed registries
- `kraft fetch` materialization of external sources into `.kraft/sources`

Current limitation:

- external registry dependencies can now be fetched, but are not yet recursively compiled into the execution graph

See [docs/kraft.md](../../docs/kraft.md) for the V1 design draft.
