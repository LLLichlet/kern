# `kern-lsp`

`kern-lsp` is the Language Server Protocol implementation for Kern.

The server is intentionally designed as a small Rust executable that speaks
JSON-RPC over stdio and reuses the compiler workspace for language semantics.

## Design Goals

- Keep the implementation small, explicit, and easy to port into future Kern tooling.
- Reuse `kernc_*` crates instead of rebuilding a second frontend in another language.
- Avoid protocol/framework-heavy dependencies.
- Prefer solid editor interoperability over a large but shallow feature surface.

## Current Scope

The current server implements:

- `initialize`
- `initialized`
- `shutdown`
- `exit`
- `textDocument/didOpen`
- `textDocument/didChange`
- `textDocument/didClose`
- `textDocument/publishDiagnostics`
- `textDocument/documentSymbol`
- `textDocument/definition`
- `textDocument/documentHighlight`
- `textDocument/references`
- `textDocument/hover`
- `textDocument/signatureHelp`
- `textDocument/completion`
- `textDocument/semanticTokens/full`
- `textDocument/codeAction`
- `textDocument/prepareRename`
- `textDocument/rename`

Document state is maintained in memory and reanalyzed through compiler source
overrides, so diagnostics and editor queries stay aligned with unsaved buffers.
`textDocument/didChange` accepts both whole-document replacements and
incremental range updates. Semantic tokens currently combine lexer-driven token
classes with compiler analysis for declarations and identifier references, then
fill common syntax contexts such as parameters, field access, and type
positions.
Diagnostics now surface compiler hints inline and forward related spans through
LSP `relatedInformation`, which improves cross-location error navigation in
clients that support it. Document highlights resolve same-file definition and
reference spans for the symbol under the cursor. Signature help resolves
function parameter labels and tracks the active argument for callable
expressions with compiler-known signatures. Code actions currently focus on
safe quick fixes such as inserting a missing semicolon or closing delimiter,
plus a small set of compiler-guided semantic repairs.

Current limitations:

- only `file://` URIs are supported
- analysis defaults to `--use-std`, but this can now be overridden at server
  startup with `--no-use-std`, `-M name=path`, and `-I name=path`
- semantic tokens do not yet cover every semantic reference class
- code actions are intentionally limited to safe, local edits
- formatting and workspace-wide indexing are not implemented yet

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

This makes it practical to integrate with lightweight clients first, then add
editor-specific polish on top without changing core analysis behavior.

## Planned Architecture

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
kern-lsp --no-use-std -M std=./library/std
```

As of the `0.6.5` release cycle, the repository also carries a first-party
VS Code extension under `editors/vscode/` that launches `kern-lsp` directly,
including bundled release packaging for the language server binary.

## Dependency Policy

`kern-lsp` should stay close to zero dependencies, but not at the expense of
clarity. The initial crate uses:

- `serde`
- `serde_json`

These are limited to protocol parsing/encoding. Compiler analysis should remain
in the existing workspace crates.
