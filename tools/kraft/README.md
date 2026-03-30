# `kraft`

`kraft` is the planned Kern package manager and build orchestrator.

This directory intentionally lives under `tools/` rather than `compiler/`.

- `kernc` is the compiler driver.
- `kraft` is the package graph resolver, lockfile manager, and build planner.

The current implementation now covers phase 1:

- `Kraft.toml` discovery
- manifest parsing
- basic validation
- `kraft check`

See [docs/kraft.md](../../docs/kraft.md) for the V1 design draft.
