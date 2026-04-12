# bed

`bed` is the first serious editor incubator for Kern.

This reset version deliberately stops trying to invent a new terminal workflow
and instead tracks the shape of a lightweight `nvim` core:

- modal editing with explicit normal/insert/command states
- a line-oriented text buffer with cursor motion and in-place edits
- a terminal screen renderer with ANSI cursor control
- a bottom command line for `:q`, `:q!`, `:w`, `:wq`, `:e`, and `:w <path>`

Explicit non-goals for this stage:

- Lua support
- plugin/config runtimes
- remote APIs
- multi-window and split management
- full Vim command language coverage

The near-term objective is to harden a small but correct editor loop whose
module boundaries line up with a future Kern-native reimplementation of more of
Neovim's core subsystems.
