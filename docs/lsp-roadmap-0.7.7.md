# Kern LSP Roadmap for 0.7.7

This document tracks the remaining release work for making `kern-lsp` a core
Kern toolchain component. Completed implementation history has been removed from
this roadmap so release review can focus on what still needs validation.

## Goal

For a community programming language in 2026, the LSP is part of the product
surface. Users will judge Kern through diagnostics, completion, navigation,
rename, semantic tokens, code actions, call hierarchy, and editor responsiveness
before they read most compiler documentation.

The 0.7.7 goal is therefore twofold:

- make the language server reliable enough for daily use
- make advanced editor behavior compiler-backed rather than protocol glue or
  name-matching approximations

## Design Principles

1. Surface truth, never fake success.
   Compiler, project, protocol, cancellation, and internal failures must become
   visible errors, diagnostics, or traces. Empty results are valid only when the
   semantic answer is genuinely empty.

2. Keep broken user code cheap.
   Syntax errors and incomplete edits are normal editor input. The server should
   provide lexical or structural service where full semantic analysis is not
   available.

3. Prefer snapshots over mutable shared state.
   Requests run against explicit document/project snapshots. Coordinator-owned
   state remains responsible for mutation, generation checks, and publication.

4. Make interaction higher priority than background work.
   Completion, hover, signature help, definition, and rename preparation must not
   wait behind workspace diagnostics or refresh work.

5. Cancellation must be real.
   Canceled or stale requests should stop consuming expensive analysis time as
   soon as practical and must not publish stale semantic results.

6. Do not hide compiler architecture behind LSP hacks.
   If IDE behavior needs a better compiler query, add the proper compiler or IDE
   fact. Do not synthesize semantic answers from strings.

7. Keep reproducibility.
   LSP behavior must be testable through deterministic server-level and
   analysis-level tests, not only manual VS Code use.

## Success Criteria

0.7.7 should be considered ready only if all of these are true:

- `kern-lsp` processes independent read requests without a single global
  synchronous bottleneck.
- Long-running diagnostics and workspace refreshes do not block interactive
  requests.
- Canceled requests stop or become inert before avoidable expensive work.
- Stale analysis results cannot overwrite newer document generations.
- Bad `Craft.toml`, broken workspace configuration, and compiler/toolchain
  failures do not silently fall back to unrelated standalone analysis.
- Every advertised LSP feature has server-level tests.
- Deterministic stress tests cover open/change/save/cancel/request
  interleavings.
- Request latency, analysis tier, cancellation, queue depth, cache behavior, and
  panic recovery are observable through logs or test hooks.
- The first-party VS Code extension remains responsive under rapid typing.
- Documentation and README feature lists match the capabilities the server
  actually advertises.

## Remaining Work

### 1. User Documentation

Update user-facing documentation after feature implementation is frozen.

Required updates:

- `tools/lsp/README.md`
- VS Code extension README and marketplace text
- any feature list that says which LSP requests are supported
- any troubleshooting section that describes logging, tracing, project errors,
  cancellation, or workspace refresh behavior

Exit criteria:

- Documentation does not advertise unsupported capabilities.
- Documentation does not omit major supported capabilities.
- Advanced features are described in terms of actual user behavior, not internal
  implementation phases.

### 2. VS Code Release Verification

The first-party VS Code extension is part of the LSP release surface.

Required checks:

- server discovery through configured path, active toolchain, `PATH`, installed
  toolchain, and workspace build
- clear user-facing error when no `kern-lsp` can be found
- restart and output commands
- server launch smoke test
- core request-flow smoke test
- VSIX package verification

Exit criteria:

- VS Code extension checks/tests pass.
- VSIX packaging succeeds.
- The packaged extension launches the intended server and handles missing-server
  paths cleanly.

### 3. Manual VS Code Smoke Test

Manual GUI validation remains required because automated extension tests are not
yet a full VS Code UI harness.

Run against a medium Kern workspace and cover:

- launch and server discovery
- diagnostics
- completion
- hover
- signature help
- go to definition
- references
- rename and prepare rename
- code actions and `codeAction/resolve`
- import insertion
- trait impl stubs
- workspace symbols
- document symbols
- semantic tokens
- inlay hints
- document links
- code lenses
- call hierarchy, including indirect and dynamic-dispatch cases
- workspace refresh progress
- rapid typing while diagnostics or refresh work is queued
- cancellation of a long request followed by a fresh interactive request

Exit criteria:

- No stale response is published after edits or cancellation.
- Rapid typing does not freeze interactive requests.
- Project reload errors appear and clear correctly.
- Any manual-only risk is recorded before tagging.

### 4. Release Test Pass

Run the automated release checks after the final documentation and VS Code
changes.

Required commands:

- `cargo test -p kern-lsp`
- VS Code extension check command
- VS Code extension test command
- VSIX packaging command

Exit criteria:

- All release checks pass on a clean worktree.
- Any known flake is either fixed or explicitly documented with a concrete
  reproduction and mitigation.

## Test Matrix

The release pass should preserve coverage in these areas.

### Protocol

- initialize capability negotiation
- UTF-16 position handling
- invalid params
- unknown method
- shutdown/exit ordering
- queued cancellation
- running cancellation
- stale generation response dropping
- progress notifications
- worker panic recovery

### Documents

- whole-document changes
- incremental UTF-16 changes
- rapid edit bursts
- save while diagnostics are queued
- close while request is in flight
- untitled documents
- unsupported URI schemes

### Projects

- no-manifest standalone files
- valid single-package project
- valid workspace
- invalid manifest
- invalid workspace member
- stale `.craft/analysis.toml`
- generated source aliases
- changed `Craft.toml`
- changed lockfile or analysis context
- workspace folder changes
- multi-root workspace queries

### Features

- diagnostics
- completion and `completionItem/resolve`
- hover
- signature help
- definition
- declaration
- type definition
- implementation
- references
- document highlights
- rename and prepare rename
- document symbols
- workspace symbols
- semantic tokens: full, range, and delta
- inlay hints
- code actions and `codeAction/resolve`
- formatting and range formatting
- folding ranges
- selection ranges
- document links and `documentLink/resolve`
- code lenses and `codeLens/resolve`
- call hierarchy

### Stress

- open many files, then request symbols and diagnostics
- alternate edits and completion requests on one active file
- submit references, cancel, edit, then request hover
- workspace refresh while interactive requests continue
- repeated invalid/valid `Craft.toml` transitions
- advanced-provider mixed request sequences

## Release Checklist

Before tagging 0.7.7:

- `cargo test -p kern-lsp` passes.
- LSP stress tests pass in CI.
- VS Code extension checks/tests pass.
- VSIX packaging verification passes.
- Manual VS Code smoke test passes on a medium Kern workspace.
- Bad project manifests produce visible errors.
- Rapid typing does not freeze completion or hover.
- Canceled long requests do not publish stale results.
- Workspace refresh reports progress.
- Documentation and README feature lists match advertised capabilities.

## Non-Goals for 0.7.7

- Full rust-analyzer parity.
- Persistent global symbol database.
- Remote language server mode.
- Plugin API for third-party LSP extensions.
- Supporting non-stdio transports.
- Supporting non-UTF-16 position encoding before there is a real client need.

These are valid future directions, but 0.7.7 should first ship the current
compiler-backed, tested LSP foundation cleanly.
