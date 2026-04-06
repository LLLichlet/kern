# Craft Analysis Context

`craft` and `kern-lsp` do not operate on the same kind of state.

`Craft.lock` exists to lock dependency resolution and source identity.
It is part of the reproducibility surface of a project.

`.craft/analysis.toml` exists to snapshot the most recent resolved analysis
context for editor tooling. It is a derived local artifact under `.craft/`.
The entire `.craft/` tree is machine-local state and should stay out of version
control.

## Responsibilities

`Craft.lock` is responsible for:

- dependency graph identity
- source bindings and source provenance
- package manifests and script digests needed to detect stale resolution

`.craft/analysis.toml` is responsible for:

- the feature selection used for the last resolved analysis plan
- unit-to-source-root mapping after `craft.rn` and `build.rn` have run
- compile-time `cfg` / `define` values that the frontend must see
- generated source roots that do not exist in the static manifest

## Non-Responsibilities

`Craft.lock` must not become the storage layer for editor or machine-local
analysis state.

`.craft/analysis.toml` must not become a lockfile. It is allowed to be replaced,
regenerated, or discarded at any time.

`craft` should maintain a root `.gitignore` entry for `.craft/` so that derived
artifacts under `.craft/` do not get committed by accident.

## Priority Rules

When both persisted context and explicit editor configuration exist:

- explicit editor feature selection wins
- persisted analysis context is the default fallback for the last known world
- if persisted context is missing or stale, tools fall back to live `craft`
  planning

## Staleness

`.craft/analysis.toml` is considered stale when any of the following changes:

- the root `Craft.toml`
- a package manifest participating in the resolved project
- the workspace `craft.rn`, if present
- a package-local `craft.rn`, if present

Stale persisted context must be ignored rather than partially trusted.
