# Kern VS Code Extension

This extension wires VS Code to `kern-lsp` and provides a baseline editing
experience for `.rn` source files.

## Features

- Kern language registration for `.rn`
- Kern logo as the bundled language icon
- optional `Kern Icons` file icon theme for `.rn` files
- stdio LSP connection to `kern-lsp`
- diagnostics, hover, completion, rename, semantic tokens, and code actions
- a lightweight TextMate grammar and language configuration for editor basics
- a `Kern: Restart Language Server` command
- a `Kern: Show Language Server Output` command
- a `Kern: Refresh Craft Analysis Context` command
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

`editors/vscode/testdata/highlighting-showcase.rn`

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

`kern-lsp` resolves the official libraries relative to its own executable:
installed toolchains use `lib/kern`, while repository builds use the repository
`library/` directory. This is why the extension avoids launching a bundled
server by default: editing the standard library should affect the same library
tree the running language server analyzes.

## Packaging

To package a VSIX:

```bash
cd editors/vscode
npm run package:vsix -- --target linux-x64
```

This packages the extension entrypoint, grammar, snippets, icons, and runtime
JavaScript dependencies. It intentionally excludes `server/` and does not embed
`kern-lsp` or the official libraries.

## Icons

The extension contributes the Kern logo as the default language icon for `kern`
documents and also ships a `Kern Icons` file icon theme that maps `.rn` files
to the same mark.

The packaged extension also declares the PNG logo as its Marketplace icon, so
the published listing and installed extension entry stay visually aligned with
the language/file icon assets.

`../../logo.svg` and `../../logo.png` are the canonical repository logo assets.
Run `npm run sync:icons` in `editors/vscode/` after updating them to refresh
`icons/kern.svg` and `icons/kern.png` before packaging or publishing.

If your current file icon theme already overrides language/file icons, switch
VS Code's File Icon Theme to `Kern Icons` to guarantee that `.rn` files use the
bundled logo.

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

## Release Posture

- Marketplace name: `Kern`
- Release packaging: ship editor integration only; use the installed Kern toolchain for `kern-lsp` and libraries
- Local fallback behavior: configured path, configured toolchain, `PATH`, installed toolchain, workspace build
- Current release check: `npm run check && npm run package:vsix -- --target <target>`
