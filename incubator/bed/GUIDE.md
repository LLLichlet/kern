# bed Guide

This guide describes the current `bed` package as it exists today.

It is not a roadmap. Everything listed here is meant to reflect current
behavior in the repository.

## Running

From the repository root:

```sh
cargo run -q -p craft -- run --project-path incubator/bed
```

Open a file directly:

```sh
cargo run -q -p craft -- run --project-path incubator/bed -- path/to/file.txt
```

Check or test the package:

```sh
cargo run -q -p craft -- check --project-path incubator/bed
cargo run -q -p craft -- test --project-path incubator/bed
```

`bed` expects to run on a real terminal. It enters raw mode and uses the
alternate screen.

## Core Model

- `bed` is modal: normal mode for navigation and structural commands, insert mode for text entry, command mode for Ex and shell prompts.
- The text buffer is line-oriented and byte-oriented. Cursor columns are byte offsets, not Unicode grapheme columns.
- A split starts as another view onto the same buffer. The views have separate cursor and scroll state.
- If one split opens another path with `:e`, only that window switches buffers; the other windows keep their current buffer.
- Shell commands do not modify the main text buffer. Their captured output is shown in a temporary read-only output pane with history.

## Screen Layout

- The main editor area shows one or more windows.
- When shell output is visible, an output pane appears below the editor area.
- The last row is the status / prompt line.
- In normal mode the cursor is rendered as a block.
- In insert mode and command mode the cursor is rendered as a bar.

Status line fields:

- current mode
- active pane: `EDITOR` or `OUTPUT`
- active window label such as `W1/3` when more than one window exists
- current path, or `[No Name]`
- dirty marker ` [+]` when the active buffer has unsaved edits
- latest editor message when present

## Modes

### Normal Mode

Use normal mode for movement, window commands, and entering prompts.

Supported count prefixes:

- `[count]motion` works for many normal-mode motions and deletes
- examples: `5j`, `3w`, `10G`, `2x`, `3Ctrl-F`

Movement keys:

| Keys | Behavior |
| --- | --- |
| `h` `j` `k` `l` | left / down / up / right |
| arrow keys | left / down / up / right |
| `0` | move to column 0 |
| `^` | move to first non-blank byte on the line |
| `$` | move to end of line |
| `gg` | go to first line |
| `[count]gg` | go to line `count` |
| `G` | go to last line |
| `[count]G` | go to line `count` |
| `H` `M` `L` | jump to top / middle / bottom visible editor row |
| `w` `b` `e` | word forward / backward / end |
| `W` `B` `E` | blank-delimited word forward / backward / end |
| `ge` | move to previous word end |
| `{` `}` | paragraph backward / forward |
| `%` | jump to matching delimiter |
| `f<char>` `t<char>` | find / till forward on the current line |
| `F<char>` `T<char>` | find / till backward on the current line |
| `;` `,` | repeat last find forward or in reverse |
| `Ctrl-B` `Ctrl-F` | page up / page down |
| `Ctrl-U` `Ctrl-D` | half-page up / half-page down |
| `PageUp` `PageDown` | page up / page down |

Editing entry points:

| Keys | Behavior |
| --- | --- |
| `i` | enter insert mode at the cursor |
| `a` | move right once, then enter insert mode |
| `I` | move to first non-blank byte, then enter insert mode |
| `A` | move to end of line, then enter insert mode |
| `o` | open a blank line below and enter insert mode |
| `O` | open a blank line above and enter insert mode |
| `x` | delete byte under cursor |
| `Delete` | delete byte under cursor |
| `:` | enter Ex prompt |
| `>` | enter shell prompt |
| `Tab` | toggle focus between editor and output pane, or reopen the selected output history entry |
| `Q` | hide visible shell output pane |

### Insert Mode

Use insert mode for direct text entry into the active buffer.

| Keys | Behavior |
| --- | --- |
| printable bytes | insert at cursor |
| `Enter` | split the current line |
| `Backspace` | delete backward, joining lines at column 0 |
| `Delete` | delete at cursor |
| `Tab` | insert spaces up to the next tab stop |
| arrows | move cursor |
| `Home` `End` | line start / line end |
| `PageUp` `PageDown` | page up / page down |
| `Esc` | return to normal mode |

Current tab stop is 4 columns.

### Command Mode

Command mode is used for both prompts:

- `:` Ex prompt
- `>` shell prompt

Prompt editing:

| Keys | Behavior |
| --- | --- |
| printable bytes | insert into prompt |
| `Backspace` | delete backward |
| `Delete` | delete at cursor |
| arrows left/right | move inside prompt |
| `Home` `End` | prompt start / end |
| arrows up/down | walk prompt history for the current prompt kind |
| `Enter` | run the prompt |
| `Esc` | cancel the prompt and return to normal mode |

Ex history and shell history are kept separately.

## Ex Commands

Supported Ex commands today:

| Command | Behavior |
| --- | --- |
| `:q` | quit if the active buffer is not dirty |
| `:q!` | force quit |
| `:w` | write current buffer to its current path |
| `:wq` | write, then quit |
| `:wq!` | same as `:wq` today |
| `:e <path>` | open a path into the active window |
| `:w <path>` | set the current buffer path, then write |
| `:split` | horizontal split |
| `:vsplit` or `:vs` | vertical split |
| `:close` | close active window |
| `:only` | keep only active window |
| `:set number` / `:set nonumber` | show / hide absolute line numbers |
| `:set relativenumber` / `:set norelativenumber` | show / hide relative numbers |

Behavior notes:

- `:q` and `:e <path>` refuse to discard dirty buffer state.
- `:w <path>` changes the active buffer's stored path before writing.
- `:set relativenumber` shows relative numbers away from the cursor row.
- If both `number` and `relativenumber` are enabled, the cursor row keeps its absolute line number while other rows use relative numbers.

## Window Commands

`bed` already has a real split tree instead of a fake flat list.

`Ctrl-W` commands:

| Keys | Behavior |
| --- | --- |
| `Ctrl-W w` | focus next window |
| `Ctrl-W W` | focus previous window |
| `Ctrl-W p` | focus previously active window |
| `Ctrl-W t` | focus top-left window |
| `Ctrl-W b` | focus bottom-right window |
| `Ctrl-W h` `j` `k` `l` | focus neighboring window |
| `Ctrl-W H` `J` `K` `L` | move active window to far left / bottom / top / right |
| `Ctrl-W s` | horizontal split |
| `Ctrl-W v` | vertical split |
| `Ctrl-W q` | close active window |
| `Ctrl-W o` | keep only active window |
| `Ctrl-W =` | equalize layout weights |
| `Ctrl-W +` `-` | grow / shrink active window height |
| `Ctrl-W >` `<` | grow / shrink active window width |

Layout notes:

- horizontal splits insert a separator row between child windows
- vertical splits insert a separator column between child windows
- windows keep local cursor and scroll state even when they still share the same underlying buffer

## Shell Output Pane

Press `>` in normal mode to open the shell prompt.

When the shell command finishes:

- its captured output is materialized into a temporary read-only text buffer
- that buffer is shown in an output pane below the editor
- the separator shows history position, command text, and exit status
- the latest shell command is added to shell prompt history

Output pane controls:

| Keys | Behavior |
| --- | --- |
| `Tab` | toggle focus between editor and output pane |
| `q` `Q` | hide visible output pane |
| `[` `]` | load previous / next output history entry |
| `h` `j` `k` `l` | navigate inside output |
| arrows | navigate inside output |
| `0` `$` | line start / line end |
| `gg` `G` | first / last output line |
| `H` `M` `L` | top / middle / bottom visible output row |
| `Ctrl-B` `Ctrl-F` | page up / page down |
| `Ctrl-U` `Ctrl-D` | half-page up / half-page down |
| `:` | open Ex prompt while output pane is focused |
| `>` | run another shell command |

The output pane is read-only. It is meant as a temporary inspection surface,
not as another editable buffer kind.

## Current Limits

The current package is intentionally narrow.

- text editing is byte-oriented rather than Unicode-grapheme-aware
- there is no undo / redo stack yet
- there are no registers, yanks, paste operators, or visual mode
- there is no search command line, substitute engine, or general command language beyond the listed motions and Ex commands
- shell output is temporary editor state, not a persistent project buffer
- the editor is terminal-only; there is no GUI layer, plugin system, or config runtime

That is a deliberate boundary. The current value of `bed` is that the core is
small, explicit, and already exercised by end-to-end tests.
