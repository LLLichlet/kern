# Chapter 4: LSP And Editor Workflow

`kern-lsp` is the editor-facing analysis server for Kern. It is intentionally
small and it reuses the compiler workspace instead of rebuilding language logic
in another stack.

## What It Supports Today

The current server implements:

- diagnostics
- document symbols
- definition / references / highlights
- hover
- signature help
- completion
- semantic tokens
- code actions
- prepare-rename and rename

This already covers the main edit-compile-debug loop.

## Why The Design Matters

The project does not maintain a second parser or second type checker for editor
features. `kern-lsp` drives compiler analysis with in-memory source overrides.

That means:

- unsaved buffers are analyzed against real compiler semantics
- diagnostics and navigation stay close to `kernc`
- LSP work tends to be analysis-plumbing work, not language reimplementation

## Running It Locally

Build the server:

```bash
cargo build -p kern-lsp
```

Run it over stdio:

```bash
cargo run -p kern-lsp
```

Useful startup overrides:

```bash
kern-lsp --library-bundle none -M std=./library/std
```

The server currently supports `file://` URIs and UTF-16 positions.

## How Project Resolution Works

When possible, the analysis layer resolves a `Craft.toml` and workspace context
for the file being edited. That lets the server infer:

- the correct analysis root for the current file
- local package module aliases
- workspace-local dependency roots

If a file belongs to a package library root, the analysis layer can also set the
root module name accordingly.

This is why editor behavior should usually be tested inside a realistic project
layout instead of only on isolated single files.

## Current Limitations

- only `file://` URIs are supported
- formatting is not implemented
- workspace-wide indexing is not implemented
- semantic tokens do not yet cover every semantic case
- code actions are intentionally conservative and local

These are normal tool maturity constraints, not signs that the analysis model is
wrong.

## VS Code

The repository includes a first-party extension under `editors/vscode/`.

That extension currently provides:

- language registration and syntax assets
- server launch wiring
- packaged icons and snippets

For practical day-to-day use, this is the easiest way to drive `kern-lsp`.

## Good LSP Debugging Habits

1. confirm the same file compiles through `kernc`
2. confirm the server was started with the expected `--library-bundle` or `-M` inputs
3. test the behavior on a saved file first
4. only after that inspect protocol-layer or client-layer issues

Most LSP bugs in this repository are likely to fall into one of three buckets:

- compiler analysis gap
- project/module-resolution mismatch
- client capability mismatch

That framing usually narrows the problem quickly.
