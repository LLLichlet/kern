# `kern-lsp`

`kern-lsp` is the Language Server Protocol implementation for Kern.

The server is intentionally designed as a small Rust executable that speaks
JSON-RPC over stdio and reuses the compiler workspace for language semantics.

## Design Goals

- Keep the implementation small, explicit, and easy to reuse in other Kern tooling.
- Reuse `kernc_*` crates instead of rebuilding a second frontend in another language.
- Avoid protocol/framework-heavy dependencies.
- Prefer solid editor interoperability over a large but shallow feature surface.

## Current Scope

The server implements these protocol entry points:

- `initialize`
- `initialized`
- `$/setTrace`
- `$/cancelRequest`
- `shutdown`
- `exit`
- `workspace/didChangeWorkspaceFolders`
- `workspace/didChangeConfiguration`
- `workspace/didChangeWatchedFiles`
- `textDocument/didOpen`
- `textDocument/didChange`
- `textDocument/didClose`
- `textDocument/didSave`
- `textDocument/publishDiagnostics`
- `textDocument/documentSymbol`
- `textDocument/definition`
- `textDocument/declaration`
- `textDocument/typeDefinition`
- `textDocument/implementation`
- `textDocument/prepareCallHierarchy`
- `callHierarchy/incomingCalls`
- `callHierarchy/outgoingCalls`
- `textDocument/documentHighlight`
- `textDocument/references`
- `textDocument/selectionRange`
- `textDocument/foldingRange`
- `textDocument/hover`
- `textDocument/signatureHelp`
- `textDocument/completion`
- `completionItem/resolve`
- `textDocument/semanticTokens/full`
- `textDocument/semanticTokens/full/delta`
- `textDocument/semanticTokens/range`
- `textDocument/codeAction`
- `codeAction/resolve`
- `textDocument/prepareRename`
- `textDocument/rename`
- `textDocument/codeLens`
- `codeLens/resolve`
- `textDocument/documentLink`
- `documentLink/resolve`
- `textDocument/inlayHint`
- `textDocument/formatting`
- `textDocument/rangeFormatting`
- `workspace/symbol`

## Editor Behavior

Document state is maintained in memory and reanalyzed through compiler source
overrides, so diagnostics and editor queries stay aligned with unsaved buffers.

`textDocument/didChange` accepts both whole-document replacements and
incremental range updates. Broken or incomplete user code is normal editor
input, so features fall back to cheaper lexical or structural analysis where a
full semantic answer is not available.

Diagnostics surface compiler hints inline and forward related spans through
LSP `relatedInformation`, which improves cross-location error navigation in
clients that support it. Bad `Craft.toml`, invalid workspace members, stale
analysis context, and compiler/toolchain failures are reported instead of
silently falling back to unrelated standalone analysis.

Completion, hover, signature help, definition, references, document highlights,
rename, symbols, formatting, folding, selection ranges, inlay hints, document
links, code lenses, code actions, semantic tokens, and call hierarchy are all
compiler-backed. Semantic tokens combine lexer-driven token classes with
compiler analysis for declarations and identifier references, then fill common
syntax contexts such as parameters, field access, and type positions. Full
semantic-token requests support server-owned delta updates with result IDs.

Call hierarchy expands direct calls, compiler-known dynamic dispatch targets,
local function values, closure object calls, and higher-order function facts
where the compiler can prove the target set. Partial or unknown sources remain
visible as incomplete facts; the server does not invent global call edges from
ambiguous local evidence.

Code actions focus on safe quick fixes, including local parse repairs and
compiler-guided semantic repairs such as import insertion and trait impl method
stubs. Code lenses and document links use deferred resolve payloads so initial
responses stay cheap and stable. Workspace-aware queries use all configured
workspace roots.

The scheduler separates interactive requests from diagnostics and workspace
refresh work. Requests run against explicit snapshots, stale generations are
dropped before publication, and cancellation makes stale or canceled work inert
before it can overwrite newer results. Verbose traces include request latency,
analysis tier, queue/cache state, cancellation, stale response handling, and
workspace refresh progress.

Current limitations:

- `file://` and `untitled:` document URIs are supported, but other custom URI
  schemes are still rejected
- analysis defaults to `--library-bundle std`, but this can be overridden at
  server startup with `--library-bundle <none|base|std>`,
  `--module-path name=path`, and `--module-interface-path name=path`
- semantic tokens do not yet cover every possible semantic reference class
- code actions remain intentionally limited to edits that are local, safe, and
  backed by parser or compiler facts
- call hierarchy includes only targets backed by complete or explicitly partial
  compiler facts; unresolved indirect calls are omitted rather than guessed

## Client Interoperability

`kern-lsp` currently expects a client that can:

- speak stdio JSON-RPC
- use UTF-16 position encoding
- send incremental document updates

The server also negotiates several optional capabilities:

- code actions are disabled when the client does not support code action
  literals
- `prepareRename` falls back to plain `rename` when prepare support is absent
- semantic tokens are only advertised when the client declares semantic token
  support
- semantic token deltas and inlay hints are advertised only when the client
  supports them

Workspace configuration is accepted in both common LSP shapes:

```json
{ "project": { "features": ["simd"] } }
```

and the VS Code `configurationSection: "kern"` shape:

```json
{ "kern": { "project": { "features": ["simd"] } } }
```

Supported project settings are:

- `features`
- `noDefaultFeatures`
- `libraryBundle`
- `modulePaths`
- `moduleInterfacePaths`

This makes it practical to integrate with lightweight clients first, then add
editor-specific polish on top without changing core analysis behavior.

## Observability

Use standard LSP tracing through `initialize.trace` or `$/setTrace`.
`messages` emits trace events, while `verbose` includes analysis details such as
request id, document generation, document version, snapshot generation, latency
budget state, selected analysis tier, and cache hits or misses.

Set `KERN_LSP_LOG` to a file path to mirror trace events to newline-delimited
JSON without requiring client trace support. Log write failures are ignored so
they do not break protocol delivery.

Clients that support work-done progress receive progress notifications for
workspace refreshes and long workspace requests.

## Source Layout

- `transport.rs`: LSP message framing (`Content-Length` headers over stdio)
- `protocol.rs`: small typed JSON-RPC/LSP protocol structures
- `server.rs`: request dispatch and session lifecycle
- `analysis.rs`: document store and compiler-facing analysis coordination

Compiler-side analysis now exposes in-memory source overrides and an owned
symbol artifact so the language server can query compiler-derived structure
without touching codegen or linking.

## Running Locally

Build the server:

```bash
cargo build -p kern-lsp
```

Run it directly over stdio:

```bash
cargo run -p kern-lsp
```

The common integration path is a manual LSP client configuration that launches
the compiled `kern-lsp` binary over stdio.

Useful analysis overrides:

```bash
kern-lsp --library-bundle none --module-path std=./library/std
```

In a source checkout this path is inside the in-tree `library/` workspace. For
an external compatible workspace, prefer setting `KERNLIB_PATH` or passing the
matching explicit module paths.

The repository also carries a first-party VS Code extension under
`editors/vscode/` that launches `kern-lsp` from the active Kern toolchain,
`PATH`, or a local repository build.

## Dependency Policy

`kern-lsp` stays close to zero dependencies without sacrificing clarity. The
crate uses:

- `serde`
- `serde_json`

These are limited to protocol parsing/encoding. Compiler analysis remains in
the existing workspace crates.
