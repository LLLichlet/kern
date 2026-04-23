# SDK Rebuild Plan

This document defines the active restructuring pass for Kern's host tooling and
release engineering.

The current repository is still too dependent on:

- ad-hoc shell / PowerShell orchestration
- CI runner defaults
- host LLVM / linker behavior that is not yet under repository control

That is acceptable for local experiments. It is not suitable as the
distribution model for `kernc`, `craft`, and `kern-lsp`.

## Environment Separation

The rebuild must keep three environments distinct:

- current CI provider: what ordinary verification jobs use today
- bootstrap provider: what Kern uses to assemble or refresh controlled
  toolchain artifacts
- end-user SDK environment: what installed Kern SDK users actually execute

These are not interchangeable. In particular, a future switch to
archive-based CI inputs must not remove Kern's ability to bootstrap and refresh
those archives.

## Goals

The rebuild has four goals:

- unify repository operations behind one cross-platform entrypoint
- move release artifacts toward a real SDK layout
- make CI validate a controlled build environment instead of lucky host state
- prepare for bundled toolchain distribution without another packaging rewrite

## Phase 1 Status

Phase 1 is complete:

- the canonical repository operations entrypoint now lives under `ops/`
- release packaging moved to `python -m ops release package`
- release archives now include `manifest/sdk.json`

This removed the split shell/PowerShell packaging logic and established one
cross-platform source of truth.

## Canonical Operations Entry Point

Repository operations should converge on:

```text
python -m ops <area> <command> ...
```

The canonical implemented paths now include:

```text
python -m ops ci ...
python -m ops release package
```

For installation, the intended split is:

- user-facing install: native `install.sh` / `install.ps1`
- repository and CI automation: `python -m ops install`

## SDK Layout Direction

Release artifacts should evolve toward this structure:

```text
kern-<version>-<host-target>/
  bin/
  lib/kern/
  manifest/sdk.json
  toolchain/
```

Current SDK archives populate:

- `bin/`
- `lib/kern/`
- `manifest/sdk.json`
- `toolchain/host/bin`
- `toolchain/host/lib`
- `toolchain/host/sysroot`

The packaged host LLVM/Clang toolchain now lives under `toolchain/host/`, but
that does not mean every installed SDK should carry a full relocatable LLVM
development prefix.

End-user SDK archives should prefer the smallest host-tool runtime that lets
installed `kernc` / `craft` / `kern-lsp` run correctly. Full LLVM headers,
`llvm-config`, and other source-build-oriented development assets belong in
repository-managed environments or standalone toolchain artifacts, not in the
default user install path.

Platform sysroots remain host responsibilities unless Kern explicitly vendors
them later.

## Planned Toolchain Resolution Order

`kernc` resolves its toolchain in this order:

1. explicit `--toolchain-root`
2. bundled SDK-relative toolchain
3. explicit environment overrides
4. system `PATH`

The rebuild is intended to make step 2 the default stable path for installed
SDKs while keeping steps 3 and 4 as source-build fallbacks.

## Why Python

Python is the right orchestration layer for this repository because:

- it is already present on CI runners
- it avoids shell quoting drift across Unix and Windows
- it can own packaging, archive layout, downloading, checksums, and manifests
- it avoids bootstrap problems that would come from using a Rust binary as the
  primary environment-preparation tool

Shell and PowerShell may still exist for narrowly scoped local tasks, but they
should not remain part of release engineering or CI control flow.

## Next Phases

### Phase 2 Status

Phase 2 is mostly in place:

- CI helper entrypoints moved into `ops/`
- CI toolchain policy is now declared in `manifest/ci-toolchains.json`
- CI policy now distinguishes current providers from the intended vendor-artifact target
- installer-side archive handling consumes `sdk.json`
- release packaging now bundles the host LLVM/Clang toolchain layout
- bundled toolchain components now carry manifest-level path/checksum metadata
- driver resolution prefers explicit and SDK-relative toolchains before ambient
  PATH lookup
- release workflows now smoke-install their freshly packaged SDK archives
- release workflows now also package the controlled host LLVM toolchain as a
  standalone artifact
- CI setup logic now supports archive-based toolchain providers generically,
  while Linux/macOS still temporarily fall back to package-manager inputs
- CI policy now distinguishes current and bootstrap toolchain providers
- CI toolchain policy resolution is now host-target aware, so macOS x86_64 and
  aarch64 vendor artifacts can be addressed independently
- standalone toolchain artifacts are now versioned by LLVM payload version, not
  by SDK release version
- release workflow now publishes a stable `toolchain-llvm-<llvm-version>` GitHub
  release channel for controlled host toolchain artifacts
- CI and release now bootstrap a host toolchain first, package a controlled
  toolchain artifact locally, then switch subsequent validation/package steps
  onto that packaged artifact instead of staying on the bootstrap environment
- release publishing now separates SDK, toolchain, and editor artifacts at the
  workflow level, and the SDK release is gated on a post-publish verification
  pass that consumes the published `toolchain-llvm-*` channel
- ordinary branch CI now uses the published `toolchain-llvm-*` channel in its
  main verification matrix
- bootstrap assembly is retained as a dedicated CI job so Kern does not lose
  day-to-day coverage of the toolchain packaging path
- published toolchain releases now emit sha256 sidecars and a structured
  release manifest, and target-mode CI resolves archive checksums from those
  published artifacts instead of accepting unchecked downloads

### Next Work

- decide which host runtime libraries should be bundled versus treated as host
  OS baselines
- define a stricter manifest for bundled toolchain provenance and component
  health checks
- move CI verification from "runner has LLVM" to "SDK archive contains the
  exact toolchain that release users will execute"
