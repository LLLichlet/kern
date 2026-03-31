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
- `textDocument/references`
- `textDocument/hover`
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
Code actions currently focus on safe parser quick fixes such as inserting a
missing semicolon or closing delimiter.

Current limitations:

- only `file://` URIs are supported
- the temporary analysis policy currently enables `--use-std` by default to
  match common Kern project editing flows
- semantic tokens do not yet cover every semantic reference class
- code actions are currently limited to a small set of safe quick fixes
- formatting and workspace-wide indexing are not implemented yet

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

## Dependency Policy

`kern-lsp` should stay close to zero dependencies, but not at the expense of
clarity. The initial crate uses:

- `serde`
- `serde_json`

These are limited to protocol parsing/encoding. Compiler analysis should remain
in the existing workspace crates.
