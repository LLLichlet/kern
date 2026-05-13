# kernlib

`kernlib` is the official Kern library workspace.

The libraries are ordinary Craft packages. They are not compiler-privileged
crates, and toolchains should resolve them through this workspace root.

## Packages

- `base`: freestanding primitives, traits, memory helpers, collections, text,
  numeric utilities, and other platform-independent foundations.
- `rt`: startup glue and minimal runtime fallbacks used by selected runtime
  entry modes.
- `std`: user-facing facilities built on `base`, with hosted implementation
  owned internally by `std.host`.
- `kernlib-test`: internal Kern tests for library behavior across `base`,
  `std`, and `rt`; this package is not published.

## Toolchain Integration

Set `KERNLIB_PATH` to this workspace root when testing an external compatible
library snapshot:

```sh
export KERNLIB_PATH=/path/to/library
```

SDK installs place the same workspace at `lib/kern`. The `base`, `rt`, and
`std` package paths are derived from the single workspace root instead of being
configured as independent roots.

## Development

Run checks from this workspace root:

```sh
craft check
craft test
craft style --verbose
craft doc
```

`craft test` runs the internal Kern test package. Toolchain integration tests
may still prove `kernc`, `craft`, packaging, and editor tooling can consume an
alternate workspace through `KERNLIB_PATH`, but library behavior belongs here.
