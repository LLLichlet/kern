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
- explicit `[source.<name>]` manifest configuration for directory-backed or git-backed registries
- `craft fetch` materialization of external sources into `.craft/sources`

`craft` keeps package discovery decentralized. A source is just a concrete tree that
contains packages laid out as `<package>/<version>/...`, and that tree can come from:

- `directory = "vendor/registry"` for local mirrors
- `git = "https://.../registry.git"` with optional `branch`, `tag`, or `rev`

This keeps the package protocol independent from any central service while still
letting teams distribute source snapshots and mirrors over git.

For release and CI auditability, `craft fetch` now reports each external package's
resolved backend, named source, selector, and concrete git revision when the
source is git-backed.

`Craft.lock` also records deterministic registry source identity from the manifest:
the configured locator plus selector (`branch`, `tag`, `rev`, or default fetch).
It does not depend on prior network fetch state.

External package builds now resolve registry sources recursively. A package can
define its own `[source.<name>]` bindings, and missing names fall back to the
parent package's source configuration so transitive graphs stay composable.

`craft check` surfaces lightweight source safety warnings for floating git inputs
and insecure transports such as `http://` or `git://`.

`craft check --release` upgrades those warnings into a release policy gate, and CI
release smoke checks should exercise both allowed and rejected source fixtures.
The gate defaults to `enforce`, but `[craft]` may explicitly downgrade it or
allow specific source names for floating git or insecure transport cases.

See [docs/craft.md](../../docs/craft.md) for the V1 design draft.
