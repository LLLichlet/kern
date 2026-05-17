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

- The server uses a bounded worker pool rather than an async runtime.
- Protocol IO and state mutation remain coordinator-owned and serialized.
- Cancellation is checked at scheduler, snapshot, driver analysis, parser token
  traversal, module loading, structure collector/import/type-resolution loops,
  body type-check worklist boundaries, and flow dataflow worklists; deeper
  parser recovery/list loops, lowering preparation, and expression/pattern
  traversal loops remain Phase 9 work.
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

### Implementation Status

This roadmap is active. The current 0.7.7 branch has already moved the LSP
toward the architecture below, but the scheduler is not complete yet.

Completed foundation work:

- IDE-owned result types now cover the main query surfaces, with LSP conversion
  kept at the server/protocol boundary.
- Request handlers capture `AnalysisSnapshot` at the request boundary.
- Document request results are submitted back through typed task results before
  protocol responses are written.
- Interactive request results stay ahead of diagnostics publication.
- Shared analysis artifacts use `Arc`, and analysis caches use `Mutex` instead
  of `Rc`/`RefCell`.
- `craft::AnalysisProject` uses a thread-safe shared build-plan cache, so LSP
  analysis can be borrowed by worker execution.
- Document requests now execute on worker threads and send typed results back to
  the coordinator through a result channel.
- Document request execution uses a bounded fixed worker pool instead of
  spawning an unbounded thread per request.
- Diagnostics and workspace refresh execution now run on the same bounded
  worker lane and return typed results to the coordinator for generation
  checking and diagnostic publication.
- The main server loop reads stdin on a dedicated reader thread, allowing the
  coordinator to accept additional messages while document request, diagnostics,
  and workspace refresh workers are still running.
- Server-level tests cover multiple document requests being in flight together
  bounded worker execution, and worker panic recovery as LSP error responses.
- Interactive analysis-tier tracing is carried by worker results, avoiding
  cross-request trace contamination from concurrent analysis workers.
- Diagnostics analysis-tier tracing is also carried by worker results, so
  concurrent diagnostics cannot read another worker's last selected tier.
- LSP document requests now carry scheduler-level cancellation tokens. A
  canceled queued request skips analysis before the worker closure runs, a
  canceled running request becomes inert before publishing stale semantic
  results, and both paths return the LSP `RequestCancelled` error code instead
  of leaving the client request unresolved.
- Worker traces now include queue wait time, completion/cancellation status, and
  execution latency for document requests, diagnostics, and workspace refresh
  work.
- The bounded worker pool size is configurable with `kern-lsp
  --worker-threads <N>`, while the default remains conservative.
- Document request cancellation tokens now reach `AnalysisSnapshot` and the
  LSP analysis query boundary. Canceled requests stop before resolving analysis
  context or entering cached/compiler-backed semantic phases when they have not
  already started that phase.
- `kernc_driver` analysis entry points now accept the same cancellation token
  path used by LSP snapshots. Cancellation is checked at public analysis
  entry, cache fallback, structure-to-artifact, navigation-artifact, and report
  construction boundaries without keeping parallel non-cancelable driver APIs.
- Scheduler tests now distinguish cancellation from stale generations:
  cancellation returns `RequestCancelled`, while superseded generations still
  drop stale responses silently.
- Workspace refresh now uses LSP work-done progress when the client advertises
  `window.workDoneProgress`. The server creates a progress token, emits
  `$/progress` begin/end notifications around refresh work, and ignores the
  corresponding client response without treating it as an invalid request.
- Workspace folder handling is now explicit: `kern-lsp` advertises a
  single-folder policy, records `rootUri` or the first `workspaceFolders`
  entry during initialization, and warns when additional workspace folders are
  ignored. Full multi-root indexing remains future work rather than an implicit
  half-supported behavior.
- `textDocument/documentLink` is advertised and implemented for file-backed
  `mod name;` declarations, resolved `use`/import bindings, and local Craft
  dependency package references. Module links use the compiler's resolved module
  graph and semantic import resolution; package links require a resolvable local
  dependency manifest. Inline modules, unresolved module declarations,
  unresolved imports, and unresolved or remote package references intentionally
  do not produce links.
- `textDocument/prepareCallHierarchy`, `callHierarchy/incomingCalls`, and
  `callHierarchy/outgoingCalls` are advertised and implemented for direct
  function/method calls and trait-object dynamic-dispatch method calls resolved
  by compiler semantic analysis. `kernc_driver` classifies dynamic-dispatch call
  edges and records candidate implementation targets, so LSP call hierarchy
  expansion is based on compiler facts rather than string-level guesses.
  Function-value and closure-object calls are classified as indirect call edges
  without fabricated targets; expanding them requires future data-flow-backed
  target recovery.
- Diagnostics scheduling tracks the most recently active document from sync and
  document requests, and drains that target first within the existing
  diagnostics budget before returning to stable workspace ordering.
- `workspace/didChangeWatchedFiles` now parses changed file events and
  distinguishes source refreshes from project metadata reloads. Changes to
  `Craft.toml`, `Craft.lock`, and `.craft/analysis.toml` clear project and
  driver state before queuing diagnostics; ordinary source changes refresh
  analysis without dropping the project resolution cache.
- `textDocument/references` now searches all resolved workspace analysis
  targets when the query belongs to a Craft project, deduplicates repeated target
  contexts, and reports standard work-done progress when the client supplies a
  `workDoneToken`. Standalone files keep the previous single-target behavior.
- `workspace/symbol` also reports standard work-done progress when the client
  supplies a `workDoneToken`.
- `textDocument/codeLens` is advertised and implemented for Craft target roots.
  Library, binary, and example roots expose build commands, while test roots
  expose precise run-test commands. The first-party VS Code extension registers
  the returned Craft commands and executes them with the configured feature and
  environment settings.
- Semantic token range requests are advertised and implemented. Range requests
  reuse the same semantic/lexical token production path as full-document
  requests, then filter the encoded token stream to the requested range. Delta
  requests are also advertised when the client supports them and use
  server-owned result ids with edit-aware invalidation.
- Completion responses include insert text/snippet data eagerly and advertise
  `completionItem/resolve` for real documentation hydration. Initial completion
  items carry opaque resolve data; `completionItem/resolve` expands that data into
  markdown documentation instead of echoing the item unchanged.

Compiler cancellation follow-up, now tracked as Phase 9:

- Cancellation is already real at scheduler, snapshot, driver entry, and major
  analysis artifact boundaries. That is enough for Phase 4, but not enough for
  the final 0.7.7 quality bar. Phase 9 below owns the deeper parser, lowering,
  and type-checking loop cancellation work so it cannot become an implicit
  historical debt.
- Keep the intentional protocol references in analysis limited to the documented
  coordinate, sync-input, diagnostics-location, and `ide.rs` conversion
  exceptions.

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

The 0.7.7 IDE boundary intentionally still allows a small set of protocol
types inside `tools/lsp/src/analysis/*`:

- `Position` and `Range` are currently shared text-coordinate primitives.
- document synchronization parameter types are accepted by the state-mutation
  entry points.
- diagnostic related information may still use `Location` until diagnostics get
  their own text-coordinate model.
- `tools/lsp/src/analysis/ide.rs` owns `into_lsp` conversion methods and is the
  intended boundary back to protocol payloads.

Any other direct LSP feature payload in analysis code should be treated as
architectural drift.

The current exception list is temporary and must be closed out in Phase 11. Kern
does not keep known protocol leakage as a permanent compatibility layer before
1.0.

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
- `workspace/workspaceFolders` support. Static initialize folders and
  `workspace/didChangeWorkspaceFolders` are supported and advertised; workspace
  symbol indexing now walks all configured roots instead of silently ignoring
  later folders.
- `workspace/didChangeConfiguration`. The 0.7.7 server now parses supported
  `kern.project` analysis settings from the synchronized `kern` configuration
  payload, applies safe hot updates through the analysis engine, invalidates
  analysis caches, and schedules a workspace refresh only when the effective
  settings change. Unsupported settings are logged instead of being silently
  accepted.
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
- semantic token range support is done
- completion item resolve for documentation
- code action resolve for deferred/heavier quick fixes
- server-level tests for every advertised capability
- deterministic stress tests

### Should Have in 0.7.7

- type definition
- declaration
- implementation
- call hierarchy: direct resolved function/method calls and trait-object
  dynamic-dispatch target expansion are done; indirect function-value and
  closure-object calls are classified but not expanded until data-flow-backed
  target recovery exists
- document links for imports/modules/packages: file-backed module declarations,
  semantically resolved import/use bindings, and local Craft dependency package
  references are done
- code lens for tests/build targets is done for resolved Craft target roots
- workspace-wide references with progress reporting are done for resolved Craft
  workspace targets

### Can Wait Until After 0.7.7

- plugin system
- remote indexing
- cross-workspace symbol database persistence
- AI/editor-assistant integrations
- multi-root workspace polish beyond correct behavior
- deep refactoring tools beyond rename and local quick fixes

### Explicit Post-0.7.7 Capability Tasks

These are known capability gaps, not completed work:

- Multi-root workspace polish beyond the current correct in-memory behavior:
  root-scoped invalidation should continue to be hardened, but the server no
  longer uses the old single-root policy and cross-root workspace
  symbols/references are implemented.
- Advanced compiler-backed IDE facts: indirect call hierarchy expansion,
  import insertion, trait impl stubs, and wider refactoring/code-action
  providers are tracked in Phase 14.

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
- Return proper cancellation errors for requests. This is implemented for
  document requests: canceled requests return LSP `RequestCancelled` (`-32800`)
  instead of being silently dropped.
- Add progress notifications for workspace refresh/indexing and long workspace
  queries. Workspace refresh progress is implemented with
  `window/workDoneProgress/create` and `$/progress`, and now includes workspace
  symbol indexing counts; workspace references and workspace symbols use
  request-provided `workDoneToken`.

Exit criteria:

- A stress test can submit a long request, cancel it, and observe no expensive
  stale publication.
- Workspace refresh reports progress and remains lower priority than active-file
  interaction.

### Phase 5: Workspace Indexing

Purpose: support workspace features without recomputing everything on demand.

Tasks:

- Build a project/workspace index abstraction. The LSP now has a coordinator-
  shared workspace index state with a refresh generation, last refresh stats,
  per-target surface symbol indexes, and per-target metadata derived from
  `ResolvedAnalysis`.
- Track `Craft.toml`, workspace roots, package roots, source roots, generated
  aliases, analysis context files, and module/interface aliases in the
  workspace index.
- Cache document symbols and top-level definitions per package target. Surface
  analysis now feeds a shared per-target symbol index that stores both
  unfiltered workspace symbols and per-document outline trees, preserving the
  clean/dirty analysis cache split while avoiding repeated symbol-tree walks.
- Workspace refresh now prewarms those per-target symbol indexes before
  diagnostics are queued, so `workspace/symbol` can reuse the refreshed index.
- Invalidate precisely on watched file changes. Source changes clear driver and
  analysis/index artifacts while preserving project resolution; project metadata
  changes also clear project state before rebuilding the index.

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
- Semantic token range and delta support are done.
- Completion insert text is eager; `completionItem/resolve` adds markdown
  documentation.
- Code action quick fixes may remain eager for the cheapest edits, but deferred
  or heavier fixes must be represented with stable resolve data and completed
  through `codeAction/resolve`.

Exit criteria:

- Each advertised capability has direct server tests and VS Code smoke coverage.
- Unsupported capabilities are not advertised, and any intentional non-support
  must be listed as a post-0.7.7 task rather than treated as completed work.

### Phase 7: Deferred Code Actions and Resolve

Purpose: make code actions protocol-complete instead of relying only on eager
quick fixes.

Tasks:

- Added an opaque `CodeActionResolveData` schema with document URI, document
  version, requested range, diagnostic code, action kind, and stable fix id.
- Split code action production into lightweight action discovery and edit
  materialization so cheap fixes can stay eager while heavier fixes are resolved
  lazily.
- Added stable internal fix identifiers for every quick fix instead of matching
  actions by title text.
- Implemented `codeAction/resolve` by validating resolve data, checking document
  staleness, rerunning the relevant analysis path, and materializing the edit or
  command for the selected fix.
- Defined stale-data behavior explicitly: stale or invalid resolve data must not
  return an old edit.
- Advertised `codeActionProvider.resolveProvider = true` after the resolve path
  became real and covered by tests.
- Added the nonexhaustive-match catch-all quick fix as a genuinely deferred
  action: initial code action responses carry resolve data without the edit, and
  `codeAction/resolve` materializes the catch-all arm edit from analysis.
- Extended deferred action coverage to every semantic/analysis-backed quick fix:
  `let mut`, unused-binding rename, dead-store removal, make-public,
  nonexhaustive-match catch-all, and irrefutable-let-else removal. Cheap
  parse-only text fixes such as delimiter insertion remain eager.
- Deferred code action deduplication includes stable resolve data so repeated
  actions with the same title do not collapse to one stale candidate.
- Server smoke coverage now resolves a deferred action and applies the returned
  workspace edit to source text, matching the standard VS Code/LSP client edit
  path.
- Extend the deferred model to future heavier fixes such as import insertion,
  trait impl stubs, and wider multi-edit quick fixes.

Exit criteria:

- Initial code action responses for deferred fixes include stable `data` and do
  not include the heavy edit.
- `codeAction/resolve` materializes the edit from analysis, removes stale data
  from the returned action, and handles stale/invalid data without applying old
  edits.
- Server capability tests count `codeAction/resolve` only when
  `resolveProvider` is true.
- Automated server smoke coverage exercises resolving and applying at least one
  deferred code action. Release validation still includes a manual VS Code smoke
  pass for the same workflow.

### Phase 8: Stress, Fuzz, and Release Hardening

Purpose: make the LSP robust enough for community usage.

Tasks:

- Deterministic protocol stress tests now exercise opening 100 files through
  server dispatch, publishing diagnostics for each file, and querying workspace
  symbols across the resulting open-document workspace.
- Fuzz-like incremental edit coverage exists at both the analysis level and the
  server level. The server stress case drives a rapid `didChange` burst,
  verifies diagnostics coalescing, and confirms a following workspace-symbol
  request sees only the latest document text.
- Dirty-navigation hardening now distinguishes parse-failure/body-only dirty
  files from parseable structural edits. Structural top-level edits force dirty
  semantic navigation instead of reusing stale clean hover/definition data, and
  the compiler's body-only comparison canonicalizes symbols through the real
  sessions rather than relying on raw `SymbolId` equality or placeholder IDs.
- Server stress coverage now alternates rapid document changes with completion
  requests and verifies that cancel-then-edit hover flows drop stale canceled
  responses while answering from the latest dirty structural state.
- Server stress coverage now directly exercises workspace refresh while an
  interactive hover is pending, proving the refresh remains lower priority than
  active-file interaction.
- Server stress coverage now repeatedly transitions `Craft.toml` between
  invalid and valid contents, proving project reload diagnostics become visible
  and then clear without poisoning later analysis.
- Workspace-scale coverage now includes refreshed workspace indexes, generated
  source aliases, real std/example projects, and the open-100-files protocol
  stress fixture.
- Worker panic recovery tests cover document request workers and diagnostics
  workers returning LSP errors/diagnostics instead of unwinding through the
  server.
- Deterministic latency budget tests cover interactive, diagnostics, and
  workspace-refresh trace paths by forcing budget thresholds in tests.
- CI runs `cargo test -p kern-lsp`, VS Code extension checks/tests, and VSIX
  packaging verification.

Exit criteria:

- `cargo test -p kern-lsp` covers scheduler, cancellation, protocol, snapshots,
  diagnostics, stress paths, and all advertised features.
- CI rejects regressions that reintroduce silent analysis failure through the
  LSP robustness guard tests and the required `kern-lsp` CI job.
- Automated server tests cover diagnostics, completion, hover, definition,
  rename, semantic tokens, and the main editor-facing request paths. Release
  validation still includes a manual VS Code smoke pass for launch and GUI
  integration behavior because the extension test suite is not a full VS Code
  UI harness.

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

### Phase 9: Deep Compiler Cancellation

Purpose: make cancellation reach expensive compiler inner loops instead of
stopping only at driver phase boundaries.

Tasks:

- Done: compiler cancellation token has been moved to `kernc_utils`
  and re-exported by `kernc_driver`, so lower compiler crates can share the
  same cancellation contract without depending on the driver.
- Done: LSP semantic/navigation analysis paths now use cancelable body
  pipelines. Type-checker global/body worklists check cancellation between
  work items, including dirty function-body reuse and dirty
  structure-plus-parsed report paths. Deterministic tests use a check-budgeted
  token to cancel after structure analysis and inside the type-checking
  worklist, not merely at the request boundary.
- Done: `ModuleLoader`, `load_asts`, `parse_modules`, outline, document-link,
  semantic, and navigation structure paths now have cancelable module traversal.
  Collected-structure cache production uses a fallible memo API so cancellation
  does not cache partial or failed results. Deterministic tests cancel inside
  multi-module loading for parse and analysis requests.
- Done: structure artifact production now threads cancellation through
  collector, import resolver, and top-level type-resolution pass loops,
  including collected/imported/typed cache production and body-only clean-reuse
  paths. Cancellation is no longer swallowed into cache misses on these
  fallible paths. Deterministic tests cancel inside collector declaration
  loops, import resolver pending-import loops, and type resolver module/item
  loops.
- Done: parser construction and frontend parse-cache production now accept the
  compiler cancellation token. Parser cancellation checks run through token
  advancement and major recursive-descent entry points, and canceled parses do
  not poison the frontend parse memo as failed syntax parses. Tests distinguish
  cancellation from ordinary parse errors and verify a canceled parse can be
  retried successfully.
- Done: parser cancellation now also reaches specialized recovery and list
  loops, including doc/meta scanning, attribute metadata lists, comma-separated
  generic/parameter/where/type/data/call/pattern lists, use-tree/path loops,
  match/let-else arm loops, data-initializer recovery, balanced delimiter
  skipping, and type/pattern lookahead loops. Deterministic parser tests cancel
  inside attribute metadata, call argument, and data-initializer recovery loops.
- Done: body analysis now threads cancellation through flow owner/reference
  collection, per-owner CFG/dataflow phase boundaries, module-item reachability,
  unused-item/unused-binding/dead-store diagnostics, and linkage checking.
  Deterministic tests cancel after body type-checking inside flow collection and
  again after flow while producing body diagnostics.
- Done: `kernc_flow` now exposes cancelable dataflow algorithms and the driver
  uses them for CFG topology, node fact/transfer collection, liveness,
  reaching-definition worklists, use-def/def-use maps, resolved/single-source
  uses, binding summaries, and materialized analysis views. Deterministic
  `kernc_flow` tests cancel inside the liveness and reaching-definition
  worklists rather than only at driver phase boundaries.
- Done: LSP semantic-token, reference, and workspace-symbol collection now
  observe request cancellation inside their own long loops. This includes dirty
  lexical semantic tokenization, semantic class/reference merging, semantic
  token range decode/filter, workspace symbol index materialization and query
  filtering, recursive symbol flattening, single-target reference scans,
  workspace reference target iteration, and definition identity scans.
  Deterministic LSP tests cancel inside these helper loops without relying on
  timing.
- Done: structure/lowering preparation and body-only reuse now use cancelable
  module indexing, AST reuse, rebinding, and function-body reuse plan
  construction. Dirty reuse no longer performs non-cancelable clean/dirty
  module scans before entering body analysis.
- Done: type-checker cancellation now reaches beyond global/body worklist
  boundaries into expression, block statement, match arm, pattern, aggregate
  literal, generic-argument, field-default, and trait-default-method traversal.
  A deterministic driver test cancels during a large function body rather than
  only between body work items.
- Done: lowering now has a real cancelable API. `Lowerer` owns a compiler
  cancellation token, `lower_all_with_report_cancelable` checks root discovery,
  root lowering, pending monomorphization draining, expression lowering, block
  statement traversal, aggregate field ordering, and selected intrinsic
  aggregate construction loops. Driver lowering preparation exposes the
  cancelable path without leaving a dead public wrapper, and a deterministic
  lowering test cancels inside root body lowering.
- Done: workspace refresh/index warmup now uses the same cancelable snapshot
  path as request analysis. Refresh target enumeration and workspace-symbol
  index prewarming observe cancellation, while ordinary project-resolution
  failures remain failed refresh targets instead of being mislabeled as
  cancellation or worker panic. The background scheduler uses a fresh token
  because workspace refresh is not tied to an LSP request ID.
- Done: deterministic tests now cover parser recovery/list loops, module
  loading, collector/import/type-resolution loops, type-checker worklists and
  body traversal, flow/dataflow, diagnostics/linkage, lowering, LSP
  semantic/reference/workspace-symbol loops, and workspace index warmup.
- Done: cancellation exits as `Canceled`/`RequestCancelled` on cancelable
  paths and does not publish partial successful artifacts or poison fallible
  caches.

Exit criteria:

- A canceled large parse/type-check/navigation request stops inside the
  expensive loop it is currently executing.
- No public LSP analysis path keeps a non-cancelable compiler call variant.
- Cancellation tests cover parser, lowering/structure, type-checking, and
  workspace target iteration.

Status: complete. Phase 10 can start from observability rather than additional
compiler cancellation plumbing.

### Phase 10: Complete Observability

Purpose: make production LSP failures diagnosable without attaching a debugger.

Tasks:

- Done: introduced structured LSP trace context for document requests,
  diagnostics, workspace refreshes, cancellations, stale response drops, and
  worker panic recovery. Verbose traces now include request ID, method, target
  URI, scheduler document generation, document version when a document is open,
  snapshot generation, queue wait, execution time, analysis tier, cancellation
  status, and explicit error class.
- Done: analysis now records per-request cache events and summarizes project
  resolution, driver, parse/surface/structure/semantic/navigation artifacts,
  workspace symbol index reuse, semantic token cache reuse, lexical cache reuse,
  and dirty fallback decisions in verbose trace output.
- Done: string-only worker failure traces were replaced by explicit
  `LspErrorClass` values for project availability/validity, analysis failures,
  request cancellation, internal bugs, and protocol encoding errors. Unsupported
  or stale behavior is traced honestly instead of being hidden behind success
  responses.
- Done: added verbose trace regression coverage for success, diagnostics,
  workspace refresh, workspace symbol cache reuse, stale response dropping,
  project metadata invalidation, semantic token cache reuse, cancellation, and
  worker panic paths.
- Done: `KERN_LSP_LOG` is now a real optional JSONL trace sink. When set, trace
  records are appended independently of the client's `$/setTrace` setting, so
  editor clients that suppress `$/logTrace` output can still produce complete
  server-side trace evidence without making default output noisy.
- Add structured request trace fields for request ID, method, target URI,
  document generation, snapshot generation, queue wait, execution time,
  analysis tier, cancellation status, and error class.
- Add cache hit/miss summaries for project resolution, driver/artifact caches,
  workspace symbol index reuse, dirty fallback selection, and semantic token
  cache reuse.
- Replace string-only failure traces with explicit error classes:
  `ProjectUnavailable`, `ProjectInvalid`, `AnalysisFailed`, `RequestCanceled`,
  `InternalBug`, and `ProtocolError`.
- Keep default output quiet; expose complete details under verbose LSP trace and
  the optional `KERN_LSP_LOG` JSONL sink.
- Add server tests asserting the complete verbose trace shape for success,
  cancellation, stale response dropping, project invalidation, cache hit, cache
  miss, and worker panic paths.

Exit criteria:

- Every worker result and published LSP error can be traced with an error class
  and enough generation/cache context to reproduce the decision.
- Verbose trace tests cover interactive requests, diagnostics, workspace
  refresh, workspace symbols/references, cancellation, stale results, and panic
  recovery.

Status: complete. Phase 11 has started from the IDE/protocol boundary cleanup.

### Phase 11: IDE Boundary Cleanup

Purpose: remove the remaining protocol leakage from analysis code and make the
IDE layer a stable Kern-owned API.

Tasks:

- Done: introduced Kern-owned text coordinate and range types for
  document synchronization, incremental text edits, and production analysis
  query inputs. Server dispatch now converts protocol `Position`/`Range` into
  `IdePosition`/`IdeRange` before calling analysis. Internal text, formatting,
  structure, semantic-token, navigation, code-action, and diagnostics helpers
  now also use the Kern-owned coordinate primitives instead of protocol
  coordinates.
- Done: migrated IDE result fields for edits, locations, diagnostics,
  diagnostic related information, document symbols, document highlights,
  selection ranges, document links, code lenses, call hierarchy, prepare rename,
  hover ranges, and inlay hints to Kern-owned `IdeRange`/`IdePosition`/
  `IdeLocation` storage. LSP response payloads are now produced only by
  explicit `into_lsp` conversion.
- Done: replaced document synchronization protocol params in production
  `AnalysisEngine` open/change/close mutation APIs with Kern-owned
  `IdeOpenDocument`, `IdeChangeDocument`, and `IdeCloseDocument` structs.
  Protocol params are converted at server dispatch and only accepted directly by
  analysis test helpers through test-only adapters.
- Done: replaced diagnostic related-information `Location` usage with
  `IdeLocation`.
- Done: kept remaining `into_lsp` conversions in the explicit
  server/protocol boundary. `tools/lsp/src/analysis/ide.rs` is the only
  analysis module that owns LSP response conversion until a separate
  `kern_ide` crate is justified.
- Done: added a guard test that scans `tools/lsp/src/analysis.rs` and
  `tools/lsp/src/analysis/*` and fails on document-sync protocol payload usage
  outside analysis tests. The same guard also rejects public production analysis
  query APIs that directly expose protocol `Position`/`Range` inputs, protocol
  field regressions on IDE result types, and unexpected protocol imports outside
  the documented adapter/conversion exceptions.
- Done: documented the final IDE API boundary in this phase: non-test analysis
  modules may use protocol resolve-data at the boundary, URI conversion helpers,
  and `analysis.rs` coordinate conversion impls; ordinary feature providers must
  consume and return Kern-owned IDE types.

Exit criteria:

- Non-test analysis modules no longer depend on LSP protocol payload types
  except the single documented adapter boundary.
- Diagnostics, hover, completion, navigation, rename, semantic tokens, inlay
  hints, formatting, code actions, document links, code lenses, and call
  hierarchy all return Kern-owned IDE result types before LSP conversion.

Status: complete. Phase 12 has started from the remaining unadvertised or
explicitly scheduled capability gaps rather than boundary cleanup debt.

### Phase 12: Remaining Capability Gaps

Purpose: finish or formally schedule the known capability gaps without treating
non-support as completion.

Tasks:

- Done: semantic token delta support now has client capability negotiation,
  advertised `full.delta` only when supported, server-owned result-id lifecycle,
  minimal common-prefix/suffix token edits, and full-token fallback when the
  previous result id is unknown or invalidated by document changes. Result ids
  are recorded only by the coordinator when a non-stale response is actually
  written, so canceled or stale requests cannot poison the delta cache. Server
  tests cover advertised capability gating, normal delta edits, unknown
  result-id fallback, and edit-aware invalidation fallback.
- Done: multi-root workspace support now records all initialize
  `workspaceFolders`, advertises workspace folder support and change
  notifications, handles `workspace/didChangeWorkspaceFolders`, refreshes
  project metadata after folder changes, and makes workspace symbol queries and
  index warmup walk every configured root with deterministic de-duplication.
  Cross-root references also walk every configured root's resolved workspace
  targets instead of stopping at the queried document's project. Server tests
  cover initialization, folder changes, refreshed indexing, cross-root
  workspace symbol results, and cross-root references.
- Done: `codeLens/resolve` and `documentLink/resolve` now have real lazy
  payloads. Initial code-lens and document-link requests return range plus
  stable opaque `data`; `codeLens/resolve` materializes the Craft command and
  `documentLink/resolve` materializes the target URI. Both providers are
  advertised only with server dispatch and tests, so there is no no-op resolve
  support.
- Moved to Phase 14: indirect call hierarchy expansion needs new compiler or
  data-flow facts to recover function-value and closure-object call targets.
  Phase 12 must not synthesize targets from names or treat the current
  classified-but-unexpanded indirect calls as completed work.
- Moved to Phase 14: larger refactoring/code-action providers such as import
  insertion, trait impl stubs, and wider multi-edit fixes need dedicated
  compiler facts plus the deferred resolve model. They are not advertised as
  existing 0.7.7 LSP behavior until implemented.

Exit criteria:

- Each capability is either implemented with advertised provider support and
  server tests, or remains explicitly unadvertised with a tracked task. No
  unsupported request is counted as completed work.

Status: complete. The remaining known capability ideas are now tracked as
post-Phase-12 compiler-backed work instead of being counted as unfinished Phase
12 LSP protocol support.

### Phase 13: Documentation, VS Code, and Release Verification

Purpose: finish release hygiene separately from core architecture work.

Tasks:

- Update `tools/lsp/README.md`, VS Code README, and user-facing feature lists
  so they match the actually advertised capabilities.
- Run and fix `cargo test -p kern-lsp`.
- Run and fix VS Code `npm run check`, `npm run test`, and `npm run
  package:vsix`.
- Run VSIX verification through `kernworker`.
- Perform a manual VS Code smoke test on a medium Kern workspace covering
  launch, diagnostics, completion, hover, definition, rename, code actions,
  code action resolve, workspace symbols, progress, and rapid typing.
- Record any manual-only release risks before tagging.

Exit criteria:

- Release checklist below is complete and there is no stale documentation that
  contradicts server capabilities.

### Phase 14: Compiler-Backed Advanced IDE Facts

Purpose: finish advanced IDE behavior that cannot be implemented correctly with
LSP protocol glue alone. These items require explicit compiler or data-flow
facts and must not be approximated with name matching.

Tasks:

- Indirect call hierarchy expansion: expose enough compiler/data-flow facts to
  recover function-value and closure-object call targets, then expand incoming
  and outgoing call hierarchy results from those facts.
- Larger refactoring/code-action providers: implement import insertion, trait
  impl stubs, and wider multi-edit fixes through deferred resolve payloads
  backed by compiler facts.
- Add stress and correctness tests for the new facts before advertising any new
  provider behavior.

Exit criteria:

- Advanced providers either use stable compiler facts with tests or remain
  unadvertised/unimplemented. No name-synthesis fallback is accepted.

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

Mitigation: the LSP/IDE path now uses cancelable `kernc_driver` analysis APIs
without a legacy non-cancelable public analysis path. Deeper cancellation inside
parser, lowering, and type-checking loops should be driven by profiling and
stress tests.

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
