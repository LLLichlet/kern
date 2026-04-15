# bed

`bed` is a Kern-hosted terminal editor incubator with a deliberately small,
explicit core.

The current package already covers a usable modal editing loop:

- normal / insert / command modes
- byte-oriented line editing
- window splits and window focus / rearrange commands
- Ex commands for open / write / quit / window management
- shell-command capture into a temporary output pane
- ANSI terminal rendering with status line, message area, and optional line numbers

Quick start from the repository root:

```sh
cargo run -q -p craft -- run --project-path incubator/bed
```

Open a file directly:

```sh
cargo run -q -p craft -- run --project-path incubator/bed -- path/to/file.txt
```

Useful maintenance commands:

```sh
cargo run -q -p craft -- check --project-path incubator/bed
cargo run -q -p craft -- test --project-path incubator/bed
```

The full user-facing guide lives in [GUIDE.md](./GUIDE.md).

Current scope is deliberate:

- the editor is modal and keyboard-driven, closer to a small `nvim`-style core than to a general GUI editor
- windows have independent view state, and splits initially share the same buffer until one window opens another path
- shell output is treated as temporary read-only text shown in a separate pane with short history
- text editing is byte-oriented today; there is no grapheme-aware cursor or layout layer yet

The package does not yet aim to cover the whole Vim surface. It focuses on a
small core that is explicit, test-covered, and easy to extend.
