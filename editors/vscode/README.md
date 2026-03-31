# Kern VS Code Extension

This extension wires VS Code to `kern-lsp` and provides a baseline editing
experience for `.rn` source files.

## Features

- Kern language registration for `.rn`
- stdio LSP connection to `kern-lsp`
- diagnostics, hover, completion, rename, semantic tokens, and code actions
- a lightweight TextMate grammar and language configuration for editor basics
- a `Kern: Restart Language Server` command
- a `Kern: Show Language Server Output` command
- starter snippets for common Kern declarations

## Development

Install the JavaScript dependencies and compile the extension:

```bash
cd editors/vscode
npm install
npm run compile
```

Open the repository in VS Code and press `F5` from the `editors/vscode`
directory to launch an Extension Development Host.

The extension also includes checked-in VS Code launch/tasks files under
`editors/vscode/.vscode/` so repository-local extension debugging works
without extra manual setup.

For manual syntax-highlighting review, open:

`editors/vscode/testdata/highlighting-showcase.rn`

## Language Server Resolution

The extension resolves the language server in this order:

1. `kern.server.path`
2. `target/debug/kern-lsp` or `target/release/kern-lsp` inside the current
   workspace
3. `kern-lsp` on `PATH`

This makes local repository development convenient while still working with a
separately installed toolchain.

## Settings

- `kern.server.path`: explicit path to the `kern-lsp` executable
- `kern.server.args`: additional command-line arguments passed to `kern-lsp`

## Status

The extension is intentionally early and is currently aimed at preview use for
the `0.6.4` release while the broader `0.6.5` LSP maturity milestone is still
being hardened.
