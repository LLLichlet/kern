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
2. a bundled `kern-lsp` shipped inside the extension
3. `target/debug/kern-lsp` or `target/release/kern-lsp` inside the current
   workspace
4. `kern-lsp` on `PATH`

This makes local repository development convenient while still working with a
separately installed toolchain. Release packages should bundle the matching
platform `kern-lsp` binary inside the extension so the published `Kern`
extension is self-contained.

## Bundled Server Packaging

To stage a platform binary into the extension package:

```bash
cargo build -p kern-lsp --release
cd editors/vscode
npm run stage:server
```

By default this copies `../../target/release/kern-lsp` into
`server/<platform>/`. You can override the source path with
`KERN_VSCODE_SERVER_SOURCE=/abs/path/to/kern-lsp npm run stage:server`.

To package a platform-specific VSIX after staging the matching server:

```bash
cargo build -p kern-lsp --release
cd editors/vscode
npm run package:vsix -- --target linux-x64
```

This command packages the bundled extension entrypoint and the staged
`server/<target>/` binary. The release VSIX does not need to ship runtime
`node_modules/`.

CI uses the same script and passes `--server-source` explicitly so release
artifacts always bundle the just-built `kern-lsp`.

## Icons

The extension contributes the Kern logo as the default language icon for `kern`
documents and also ships a `Kern Icons` file icon theme that maps `.rn` files
to the same mark.

The packaged extension also declares the PNG logo as its Marketplace icon, so
the published listing and installed extension entry stay visually aligned with
the language/file icon assets.

If your current file icon theme already overrides language/file icons, switch
VS Code's File Icon Theme to `Kern Icons` to guarantee that `.rn` files use the
bundled logo.

The extension README intentionally does not embed the raw SVG asset. GitHub
renders local SVGs fine, but Marketplace publishing is stricter about extension
assets, so the actual icon file is shipped inside the extension package instead.

## Settings

- `kern.server.path`: explicit path to the `kern-lsp` executable
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
- Release packaging: bundle the platform-specific `kern-lsp` into each VSIX
- Local fallback behavior: configured path, bundled server, workspace build, then `PATH`
- Current release check: `npm run check && npm run package:vsix -- --target <target>`
