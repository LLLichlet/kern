# Kern VS Code Extension

This extension wires VS Code to `kern-lsp` and provides a baseline editing
experience for `.kn` source files.

## Features

- Kern language registration for `.kn`
- Kern mark as the bundled language icon
- stdio LSP connection to `kern-lsp`
- diagnostics, hover, completion, rename, semantic tokens, code actions,
  formatting, code lenses, document links, folding ranges, selection ranges,
  inlay hints, call hierarchy, and workspace symbols
- a lightweight TextMate grammar and language configuration for editor basics
- a `Kern: Restart Language Server` command
- a `Kern: Show Language Server Output` command
- a `Kern: Refresh Craft Analysis Context` command
- Craft build/test code lenses that run in VS Code terminals with colored
  output
- starter snippets for common Kern declarations

## Development

Install the JavaScript dependencies and compile the extension:

```bash
cd editors/vscode
npm ci
npm run compile
npm run check
npm run test
```

Open the repository root in VS Code and press `F5` to launch an Extension
Development Host.

If you prefer opening `editors/vscode` directly as the workspace, the checked-in
debug configuration there now works as well.

The extension also includes checked-in VS Code launch/tasks files under
`editors/vscode/.vscode/` so repository-local extension debugging works
without extra manual setup.

For manual syntax-highlighting review, open:

`editors/vscode/testdata/highlighting-showcase.kn`

## Language Server Resolution

The extension resolves the language server in this order:

1. `kern.server.path`
2. `kern.toolchain.path/bin/kern-lsp`
3. `kern-lsp` found on `PATH`
4. `KERN_HOME/bin/kern-lsp` or the default `~/.kern/bin/kern-lsp`
5. `target/release/kern-lsp` or `target/debug/kern-lsp` inside the current
   workspace
6. the plain `kern-lsp` command as a final spawn attempt

This keeps the extension tied to the active Kern toolchain instead of a stale
copy embedded in the VSIX. For normal users, the installer-provided toolchain or
`PATH` entry is enough. For repository development, putting
`target/release/` on `PATH` makes the extension use that freshly built server;
opening the compiler repository can also fall back to the local `target/`
binary.

The language server currently advertises workspace folder support, semantic
token delta support, deferred resolve for code lenses and document links, and
workspace-wide search/navigation over every configured workspace root.

Craft build and test code lenses execute `craft build` or `craft test` in a
dedicated VS Code terminal. The terminal is kept open after completion so
diagnostics and colored command output remain visible.

`kern-lsp` resolves the official libraries relative to its own executable:
installed toolchains use `lib/kern`, while repository builds use the repository
`library/` directory. This is why the extension avoids launching a bundled
server by default: editing the standard library should affect the same library
tree the running language server analyzes.

## Packaging

To package a VSIX:

```bash
cd editors/vscode
npm run package:vsix
```

This packages the extension entrypoint, grammar, snippets, icons, and runtime
JavaScript dependencies. It intentionally excludes `server/` and does not embed
`kern-lsp` or the official libraries, so the VSIX is platform-independent.
Release CI therefore publishes one `kern-language-<version>.vsix` artifact rather
than one VSIX per Kern host target.

## Icons

The extension contributes theme-specific Kern marks as the default language
icon for `kern` documents.

The packaged extension also declares the PNG mark as its Marketplace icon, so
the published listing and installed extension entry stay visually aligned with
the language icon assets.

`../../assets/brand/kern-mark-light.svg`,
`../../assets/brand/kern-mark-dark.svg`, and
`../../assets/brand/kern-mark.png` are the canonical repository mark assets.
Run `npm run sync:icons` in `editors/vscode/` after updating them to refresh
`icons/kern-light.svg`, `icons/kern-dark.svg`, and `icons/kern.png` before
packaging or publishing. The full README logo is kept separately at
`../../assets/brand/kern-logo.svg`.

The extension README intentionally does not embed the raw SVG asset. GitHub
renders local SVGs fine, but Marketplace publishing is stricter about extension
assets, so the actual icon file is shipped inside the extension package instead.

## Settings

- `kern.server.path`: explicit path to the `kern-lsp` executable
- `kern.toolchain.path`: explicit Kern toolchain root containing `bin/kern-lsp`
  and `lib/kern`
- `kern.server.args`: additional command-line arguments passed to `kern-lsp`
- `kern.server.env`: extra environment variables injected into the `kern-lsp` process
- `kern.craft.path`: explicit path to the `craft` executable used by the refresh command
- `kern.project.features`: explicit `craft` features enabled for analysis
- `kern.project.noDefaultFeatures`: disable default `craft` features for analysis

For `craft`-driven conditional compilation, configure the editor with the same
feature and environment context you use at the command line. Example:

```json
{
  "kern.project.features": ["experimental"],
  "kern.server.env": {
    "KERN_DEV": "1"
  }
}
```

After changing your active feature or environment setup, run `Kern: Refresh
Craft Analysis Context`. The command executes `craft check` for workspace roots
that contain `Craft.toml`, updates `.craft/analysis.toml`, and restarts the
language server so diagnostics and navigation pick up the new plan immediately.

The extension forwards supported `kern.project.*` settings to `kern-lsp`.
Unsupported editor-only settings are ignored by the server.

## Release Posture

- Marketplace name: `Kern`
- Release packaging: ship editor integration only; use the installed Kern toolchain for `kern-lsp` and libraries
- Local fallback behavior: configured path, configured toolchain, `PATH`, installed toolchain, workspace build
- Current release check: `npm run check && npm run test && npm run package:vsix`
- Manual release smoke should cover diagnostics, completion, hover, signature
  help, navigation, references, rename, code actions, formatting, semantic
  tokens, inlay hints, document links, code lenses, call hierarchy, workspace
  refresh, and rapid typing while diagnostics or refresh work is queued
