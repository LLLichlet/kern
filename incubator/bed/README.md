# bed

`bed` is the first serious editor incubator for Kern.

This reset version deliberately stops trying to invent a new terminal workflow
and instead tracks the shape of a lightweight `nvim` core:

- modal editing with explicit normal/insert/command states
- a line-oriented text buffer with cursor motion and in-place edits
- a terminal screen renderer with ANSI cursor control
- a bottom command line for `:q`, `:q!`, `:w`, `:wq`, `:e`, and `:w <path>`

Explicit non-goals for the current package:

- Lua support
- plugin/config runtimes
- remote APIs
- multi-window and split management
- full Vim command language coverage

The package currently focuses on a small but correct editor loop with module
boundaries that mirror the pieces it already implements.
