# `kern-lsp`

`kern-lsp` is the Language Server Protocol implementation for Kern.

The server is intentionally designed as a small Rust executable that speaks
JSON-RPC over stdio and reuses the compiler workspace for language semantics.

## Design Goals

- Keep the implementation small, explicit, and easy to port into future Kern tooling.
- Reuse `kernc_*` crates instead of rebuilding a second frontend in another language.
- Avoid protocol/framework-heavy dependencies.
- Start with diagnostics and document state management before richer editor features.

## Initial Scope

The first milestone focuses on a stable server skeleton:

- `initialize`
- `initialized`
- `shutdown`
- `exit`
- `textDocument/didOpen`
- `textDocument/didChange`
- `textDocument/didClose`
- `textDocument/publishDiagnostics`
- `textDocument/documentSymbol`

The current scaffold already implements the protocol loop, full-text document
sync, and compiler-backed diagnostic publication through in-memory document
overrides.

Current limitations:

- only `file://` URIs are supported
- only full-text sync is supported
- only diagnostics and document symbols are implemented
- the temporary analysis policy currently enables `--use-std` by default to
  match common Kern project editing flows

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

At this stage the easiest editor integration path is a manual LSP client
configuration that launches the compiled `kern-lsp` binary.

## Dependency Policy

`kern-lsp` should stay close to zero dependencies, but not at the expense of
clarity. The initial crate uses:

- `serde`
- `serde_json`

These are limited to protocol parsing/encoding. Compiler analysis should remain
in the existing workspace crates.
