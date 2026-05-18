# Kern Maintenance Plan

This plan replaces the temporary audit report that was kept outside the
repository. It records the issues we have verified against the current tree and
keeps speculative findings out of the active queue.

## Current State

- `actions/checkout@v5` is valid and should not be downgraded.
- `macos-15-intel` is a valid GitHub-hosted runner label and should not be
  replaced with `macos-13`.
- The local branch may contain unpushed maintenance commits. Pushes are handled
  manually by the maintainer.

## Completed: Installer Integrity

`install.sh` now validates SDK contents with the same basic manifest guarantees
as `install.ps1`.

Completed work:

- Parse `manifest/sdk.json` with Python's structured JSON parser.
- Validate required component records from the manifest when the toolchain is
  bundled.
- Check component existence, file size, and SHA-256 when present.
- Keep local `--archive` installs working without requiring network access.
- Add focused script-level validation in `scripts/tests/install-sh-manifest.sh`.

Why this matters:

The Unix installer is a public entry point. It currently checks SDK shape and
whether selected tools start, but it does not verify the manifest component
checksums that release packages already carry.

## Completed: Build Script Invalidation

`compiler/kernc_codegen/build.rs` now registers the exact LLVM environment
variables it consumes from `llvm-sys`.

Completed work:

- Add `cargo:rerun-if-env-changed` coverage for the discovered
  `DEP_LLVM_*_LIBDIR` and `DEP_LLVM_*_CONFIG_PATH` variables.
- Avoid broad or invalid Cargo directives that only look correct.
- Confirm `cargo check -p kernc_codegen` works.

## Completed: Small Robustness Fixes

Lossy `kernc_ty` internal ID casts have been replaced with explicit exhaustion
checks.

Completed work:

- Update `kernc_ty` type and const-expression ID allocation from `as u32` casts
  to checked conversion with clear panic messages.
- Keep this narrow; do not redesign the ID representation.
- Run `cargo test -p kernc_ty`.

## Priority 1: Toolchain And Lint Policy

Decide policy before changing CI.

Options to evaluate:

- Add `rust-version = "1.85"` metadata to document the minimum Rust version for
  Edition 2024.
- Consider a concrete `rust-toolchain.toml` only if the project wants pinned
  local toolchains.
- Add `cargo fmt --check` first if CI linting is desired.
- Evaluate `cargo clippy` separately and avoid enabling `-D warnings` until the
  current baseline is known.
- Treat dependency audit tooling as a separate policy decision.

## Backlog

- Improve selected LLVM wrapper safety documentation.
- Review production-path LLVM wrapper panics as ICE quality work, not as a
  quick `Result` conversion.
- Reduce CI Unix/Windows duplication only when touching that area for functional
  reasons.
- Revisit JSON handling in shell scripts if installer manifest parsing grows
  beyond simple validation.

## Not Planned From The Report

- Downgrade `actions/checkout@v5`.
- Replace `macos-15-intel` with `macos-13`.
- Add broad CI lint/audit gates without first checking the current repository
  baseline.
