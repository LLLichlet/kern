# Kern LSP Roadmap for 0.7.7

This document defines the 0.7.7 plan for making `kern-lsp` a core Kern
toolchain component rather than a thin synchronous adapter around compiler
queries.

The goal is not only to add missing editor features. The goal is to make the
language server mature enough that new Kern users can rely on it as their first
daily interface to the language.

## Why This Matters

For a community programming language in 2026, the LSP is part of the product
surface. Users will judge the language through diagnostics, completion,
navigation, rename, semantic tokens, and editor responsiveness before they read
most of the compiler documentation.

Kern also has a stronger-than-usual need for a reliable LSP because the language
intentionally values orthogonality and explicit semantics. The editor should
make those semantics discoverable without hiding compiler failures or silently
falling back to misleading partial analysis.

## Current State

`kern-lsp` already has useful language features:

- open, change, close, and save document synchronization
- diagnostics with hints and related information
- document symbols
- go to definition
- document highlights
- references
- hover
- signature help
- completion
- full semantic tokens
- inlay hints
- prepare rename and rename
- quick-fix code actions
- first-party VS Code integration

The current implementation is deliberately small:

- `transport.rs` handles stdio framing.
- `protocol.rs` contains a small hand-written subset of LSP types.
- `server.rs` and `server/*` run the message loop, request dispatch, lifecycle,
  diagnostics scheduling, and request generation checks.
- `analysis.rs` and `analysis/*` own document state, compiler interaction,
  caches, and LSP-facing query behavior.

Important architectural constraints today:

- The server is synchronous and single-threaded.
- There is no async runtime.
- Requests are handled one message at a time from stdin.
- Diagnostics scheduling is cooperative, not background execution.
- Cancellation only skips work before a request starts or drops stale results;
  it cannot stop a long-running analysis already in progress.
- `AnalysisEngine` uses `Rc` and `RefCell`, so it is not ready for worker
  threads.
- LSP protocol coverage is hand-written and incomplete.
- The LSP layer still knows too much about compiler artifacts.

## Baseline Comparison

The 0.7.7 target is not to copy another language server exactly, but the mature
servers establish useful baselines.

### rust-analyzer Baseline

rust-analyzer separates editor API from protocol handling. It has a stable
analysis host/snapshot model, uses immutable analysis snapshots for read
queries, and routes many requests through background execution. Its design makes
LSP one consumer of an IDE API rather than the owner of semantic analysis.

Kern should copy the principle, not the implementation:

- compiler and IDE data are separate from JSON-RPC plumbing
- requests run against a coherent snapshot
- state mutation is centralized
- read queries are parallelizable
- broken source code is normal input, while infrastructure failure is surfaced
  as an error

### ZLS Baseline

ZLS demonstrates the feature surface that users expect even from a smaller
systems-language server:

- completion
- hover
- diagnostics
- definition and declaration navigation
- references
- rename
- document and workspace symbols
- formatting
- semantic tokens
- inlay hints
- code actions

Kern already covers part of this surface, but lacks several workspace-wide and
editing features.

### Haskell Language Server Baseline

HLS demonstrates mature toolchain integration:

- compiler diagnostics
- plugin-style features
- formatting providers
- code actions
- code lenses
- workspace symbols
- call hierarchy
- folding and selection ranges
- build-tool/cradle integration

Kern should not add a plugin system before the core architecture is ready, but
0.7.7 should create boundaries that make future feature providers independent
and testable.

## Design Principles

1. Surface truth, never fake success.
   If craft, project resolution, compiler analysis, or internal LSP execution
   fails, the user must see a diagnostic, an LSP error response, or a clear log.
   Empty results are valid only when the semantic answer is genuinely empty.

2. Keep broken user code cheap.
   Syntax errors and incomplete edits are normal editor input. The server should
   provide lexical or structural service where full semantic analysis is not
   available.

3. Prefer snapshots over mutable shared state.
   A request should run against an explicit document/project snapshot. State
   mutation belongs in the coordinator.

4. Make interaction higher priority than background work.
   Completion, hover, signature help, definition, and rename preparation must not
   wait behind workspace diagnostics.

5. Cancellation must be real.
   A canceled or stale request should stop consuming expensive analysis time as
   soon as practical.

6. Do not hide compiler architecture behind LSP hacks.
   If IDE behavior needs a better compiler query, add the proper compiler/IDE
   boundary rather than patching string-level LSP output.

7. Keep reproducibility.
   LSP behavior must be testable through deterministic server-level tests and
   analysis-level tests, not only manual VS Code use.

## 0.7.7 Success Criteria

0.7.7 should be considered successful only if all of these are true:

- `kern-lsp` can process independent read requests without a single global
  synchronous bottleneck.
- Long-running diagnostics and workspace refreshes do not block completion,
  hover, or signature help.
- Canceled requests stop or become inert before doing avoidable expensive work.
- Stale analysis results cannot overwrite newer document generations.
- Bad `Craft.toml`, broken workspace configuration, and compiler/toolchain
  failures do not silently fall back to unrelated standalone analysis.
- The first-party VS Code extension remains responsive under rapid typing.
- Every supported LSP feature has server-level tests.
- The server has deterministic stress tests for open/change/save/cancel/request
  interleavings.
- Request latency, analysis tier, cancellation, queue depth, and panic recovery
  are observable through logs or test hooks.
- The architecture can accept new feature providers without changing the core
  message loop each time.

## Architecture Plan

### 1. Split IDE Queries From LSP Protocol

Create a dedicated IDE-facing layer. The exact crate/module name can be decided
during implementation, but the intended boundary is:

- `kern_ide` or `tools/lsp/src/ide/*`: document/project snapshots and semantic
  query API
- `tools/lsp/src/server/*`: LSP lifecycle, transport, request routing, task
  scheduling, cancellation, response writing
- `tools/lsp/src/protocol.rs`: protocol types and JSON conversion
- `compiler/kernc_driver`: compiler analysis artifacts and lower-level queries
- `craft`: project resolution and analysis context materialization

The IDE API should expose Kern-owned result types such as:

- `IdeDiagnostic`
- `IdeCompletion`
- `IdeHover`
- `IdeLocation`
- `IdeSymbol`
- `IdeSemanticTokens`
- `IdeInlayHint`
- `IdeWorkspaceEdit`

LSP conversion should happen after the IDE query returns.

This prevents protocol details from leaking into compiler-oriented logic and
makes it practical to test IDE behavior without JSON-RPC framing.

### 2. Introduce Snapshots

Replace direct request access to a mutable `AnalysisEngine` with snapshots.

Coordinator-owned state:

- open documents
- document versions
- dirty flags
- project roots
- workspace folders
- generation counters
- cancellation registry
- shared caches

Snapshot-owned state:

- document text map
- resolved project information
- dirty document overrides
- cache keys
- compiler driver handles
- open URI/path maps

Initial implementation can use coarse cloning and `Arc` before optimizing. The
important semantic property is that a worker request sees a stable view.

### 3. Make Analysis Thread-Ready

Current `AnalysisEngine` uses `Rc` and `RefCell`. For worker execution, move
toward:

- `Arc` for shared immutable artifacts
- `Mutex` or `RwLock` only around coordinator-owned mutable caches
- per-snapshot immutable maps for document state
- cache insertion through coordinator messages or carefully scoped concurrent
  cache structures

Avoid holding locks while running compiler analysis.

### 4. Add a Task Scheduler

Introduce an explicit scheduler with priorities:

1. Shutdown and lifecycle
2. Cancellation
3. Interactive requests: completion, hover, signature help, definition,
   references, rename preparation
4. Edit notifications and document state updates
5. Diagnostics for the active file
6. Workspace diagnostics and refresh
7. Indexing and background cache warming

The server main loop should remain responsible for protocol IO and state
mutation. Worker tasks should return typed task results to the coordinator,
which decides whether the result is still current before writing responses or
publishing diagnostics.

### 5. Implement Real Cancellation

Cancellation needs two layers:

- request-level cancellation from `$/cancelRequest`
- generation-level cancellation when a document changes

Each worker task should receive a cancellation token. Expensive phases should
check it at boundaries:

- before project resolution
- before parse/surface/semantic analysis
- between package/workspace targets
- before rendering large response lists
- before response publication

Canceled requests should produce the proper LSP error response when the request
has not already been abandoned by the client. Stale diagnostics should be
dropped silently if a newer generation superseded them.

### 6. Use Structured Error Policy

Define explicit error classes:

- `UserCodeIncomplete`: valid partial source; may return lexical/structural
  fallback
- `ProjectUnavailable`: missing project is allowed for standalone files
- `ProjectInvalid`: bad manifest/workspace; must surface
- `AnalysisFailed`: compiler reported diagnostics or could not produce the
  requested artifact
- `RequestCanceled`: user/client cancellation
- `InternalBug`: panic or invariant failure
- `ProtocolError`: invalid params, unsupported methods, unsupported encoding

Do not use `Err(_) => Ok(Vec::new())` patterns for analysis failures. If a
query chooses fallback behavior, the code should name that fallback explicitly.

### 7. Upgrade Protocol Coverage

Keep hand-written protocol types only if they remain maintainable. Otherwise,
consider adopting `lsp-types` while preserving Kern-specific server structure.

Required 0.7.7 protocol work:

- proper LSP error codes for cancellation and request failure
- `window/workDoneProgress/create` and `$/progress` for long workspace work
- `workspace/workspaceFolders` support or explicit single-folder policy
- `workspace/didChangeConfiguration`
- dynamic client capability handling where needed
- request tracing with IDs, method names, generation, and elapsed time

## Feature Plan

Feature work should follow the architecture work. New features should not be
implemented by adding more direct compiler calls inside request dispatch.

### Must Have in 0.7.7

- stable async/concurrent request execution
- real cancellation
- diagnostics background execution
- robust project/craft error reporting
- formatting entry point, even if initially conservative
- workspace symbols
- folding ranges
- selection ranges
- semantic token range or delta support
- completion resolve or a documented reason to keep completion fully eager
- code action resolve for heavier quick fixes
- server-level tests for every advertised capability
- deterministic stress tests

### Should Have in 0.7.7

- type definition
- declaration
- implementation
- call hierarchy
- document links for imports/modules
- code lens for tests/build targets once test/build metadata is stable
- workspace-wide references with progress reporting
- active-file priority diagnostics
- incremental project reload when `Craft.toml`, lockfiles, or analysis context
  files change

### Can Wait Until After 0.7.7

- plugin system
- remote indexing
- cross-workspace symbol database persistence
- AI/editor-assistant integrations
- multi-root workspace polish beyond correct behavior
- deep refactoring tools beyond rename and local quick fixes

## Work Breakdown

### Phase 0: Audit and Guardrails

Purpose: stop known bad behavior before the larger rewrite.

Tasks:

- Search for and remove remaining silent analysis failure conversions.
- Add regression tests for project load failures, compiler panics, stale
  responses, and cancellation.
- Document which empty responses are legitimate.
- Add lint-like tests or review checks for `Err(_) => Ok(empty)` in LSP analysis
  code.

Exit criteria:

- No known infrastructure failure can become a successful empty response.
- Server-level tests cover error response behavior for at least definition,
  references, hover, completion, diagnostics, and rename.

### Phase 1: IDE Boundary

Purpose: make LSP protocol a consumer, not the semantic core.

Tasks:

- Define IDE result types.
- Move query logic out of direct LSP protocol types where practical.
- Add conversion functions from IDE result types to LSP protocol JSON/types.
- Keep existing behavior stable through tests.

Exit criteria:

- At least diagnostics, hover, completion, definition, references, rename, and
  semantic tokens have IDE-level APIs independent of JSON-RPC dispatch.

### Phase 2: Snapshot Model

Purpose: make requests independent and safe to run outside the main loop.

Tasks:

- Define `AnalysisSnapshot`.
- Move open-document state into snapshot-compatible structures.
- Make cache keys and source overrides snapshot-derived.
- Convert request handlers to acquire a snapshot before analysis.
- Preserve generation checks for stale results.

Exit criteria:

- A request can run using only a snapshot and immutable/shared compiler state.
- Main server state can accept document changes while old snapshots remain
  readable.

### Phase 3: Worker Scheduler

Purpose: make the server responsive under load.

Tasks:

- Add worker pool or async task runtime.
- Keep stdio writing serialized through the coordinator.
- Add task IDs, request IDs, priorities, and generations.
- Route diagnostics through background tasks.
- Keep interactive requests above diagnostics.

Exit criteria:

- Rapid typing does not block completion behind diagnostics.
- Multiple independent read requests can be in flight.
- Responses are still ordered by JSON-RPC semantics where required, and each
  response carries the correct request ID.

### Phase 4: Cancellation and Progress

Purpose: make long-running work controllable and visible.

Tasks:

- Add cancellation tokens.
- Handle `$/cancelRequest` for queued and running requests.
- Add generation cancellation on document edits.
- Return proper cancellation errors for requests.
- Add progress notifications for workspace refresh/indexing.

Exit criteria:

- A stress test can submit a long request, cancel it, and observe no expensive
  stale publication.
- Workspace refresh reports progress and remains lower priority than active-file
  interaction.

### Phase 5: Workspace Indexing

Purpose: support workspace features without recomputing everything on demand.

Tasks:

- Build a project/workspace index abstraction.
- Track `Craft.toml`, workspace members, package roots, source roots, generated
  aliases, and analysis context files.
- Cache document symbols and top-level definitions per package target.
- Invalidate precisely on watched file changes.

Exit criteria:

- Workspace symbols are fast after initial indexing.
- Project reload errors are surfaced and do not poison unrelated standalone
  documents.

### Phase 6: Feature Completion

Purpose: close the most visible feature gaps.

Tasks:

- Formatting and range formatting.
- Workspace symbols.
- Folding ranges.
- Selection ranges.
- Type definition, declaration, implementation.
- Call hierarchy.
- Semantic token range or delta.
- Completion resolve.
- Code action resolve.
- Document links for module imports and package references.

Exit criteria:

- Each advertised capability has direct server tests and VS Code smoke coverage.
- Unsupported capabilities are not advertised.

### Phase 7: Stress, Fuzz, and Release Hardening

Purpose: make the LSP robust enough for community usage.

Tasks:

- Add deterministic protocol stress tests.
- Add fuzz-like incremental edit tests at the server level.
- Add workspace-scale fixtures.
- Add panic recovery tests for worker tasks.
- Add request latency budget tests where deterministic.
- Add CI jobs for LSP tests and VS Code extension integration.

Exit criteria:

- `cargo test -p kern-lsp` covers scheduler, cancellation, protocol, snapshots,
  diagnostics, and all advertised features.
- CI rejects regressions that reintroduce silent analysis failure.
- VS Code smoke tests cover launch, diagnostics, completion, hover, definition,
  rename, and semantic tokens.

## Concrete Test Matrix

### Protocol Tests

- initialize capability negotiation
- UTF-16 encoding requirement
- invalid params
- unknown method
- shutdown/exit ordering
- cancellation before execution
- cancellation during execution
- stale generation response dropping
- progress notifications
- worker panic recovery

### Document Tests

- whole-document changes
- incremental UTF-16 changes
- rapid edit bursts
- save while diagnostics are queued
- close while request is in flight
- untitled documents
- unsupported URI schemes

### Project Tests

- no manifest standalone file
- valid single-package project
- valid workspace
- invalid manifest
- invalid workspace member
- stale `.craft/analysis.toml`
- generated source aliases
- changed `Craft.toml`
- changed lockfile or analysis context

### Feature Tests

- diagnostics
- completion
- hover
- signature help
- definition
- references
- document highlights
- rename and prepare rename
- document symbols
- workspace symbols
- semantic tokens
- inlay hints
- code actions
- formatting
- folding and selection ranges
- call hierarchy

### Stress Tests

- open 100 files, then request symbols and diagnostics
- alternate edits and completion requests on one active file
- submit references, then cancel, then edit, then request hover
- workspace refresh while interactive requests continue
- repeated invalid/valid `Craft.toml` transitions

## Observability Requirements

`kern-lsp` should expose enough detail to debug user reports without attaching a
debugger.

Required fields for request tracing:

- request ID
- method
- target URI when available
- document generation
- snapshot generation
- queue wait time
- execution time
- analysis tier
- cache hit/miss summary
- cancellation status
- error class

The default output should remain quiet. Verbose tracing should be available
through LSP trace settings and, if useful, a `KERN_LSP_LOG` environment
variable.

## VS Code Requirements

The first-party VS Code extension should remain thin, but it must be treated as
part of the LSP release surface.

0.7.7 VS Code requirements:

- reliable server discovery through configured path, active toolchain, `PATH`,
  installed toolchain, and workspace build
- clear user-facing error when no `kern-lsp` can be found
- no embedded server binary in the VSIX
- restart and output commands
- smoke tests for server launch and core request flow
- no platform-specific VSIX unless native assets are reintroduced

## Risk Register

### Risk: Rewriting too much at once

Mitigation: keep phases independently shippable. Preserve existing feature tests
while changing internals.

### Risk: Async runtime increases complexity

Mitigation: choose a minimal scheduler design first. The key requirement is
concurrent request execution and cancellation, not adopting a specific runtime.

### Risk: Compiler APIs are not cancellation-aware

Mitigation: add cancellation checks at LSP/IDE phase boundaries first. Later,
thread cancellation deeper into compiler analysis if profiling shows it is
needed.

### Risk: Background indexing becomes stale

Mitigation: require generation checks and precise invalidation tests before
advertising workspace-wide features.

### Risk: Feature additions hide architecture bugs

Mitigation: no new advertised capability without server-level tests, stress
coverage, and error-path coverage.

## Release Checklist

Before tagging 0.7.7:

- `cargo test -p kern-lsp` passes.
- LSP stress tests pass in CI.
- VS Code extension tests pass.
- Manual VS Code smoke test passes on a medium Kern workspace.
- Bad project manifests produce visible errors.
- Rapid typing does not freeze completion.
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

These are valid future directions, but 0.7.7 should first establish the
foundation that makes them straightforward rather than risky.
