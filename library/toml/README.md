# `toml`

`toml` is an independent Kern package hosted in the repository under
`library/toml/`.

It is intentionally not part of `std`. The package exists to exercise `craft`
with a real library target while providing a natural home for future TOML
parsing, document modeling, and configuration helpers.

## Status

The current package is a minimal bootstrap:

- `Craft.toml` defines an ordinary `craft` library package
- `src/lib.rn` exposes the initial public API surface
- `parse` performs lightweight document classification without allocation

This keeps the package small, buildable, and ready for incremental expansion.

## Layout

```text
library/toml/
  Craft.toml
  README.md
  src/
    lib.rn
    document.rn
    error.rn
    parser.rn
```

## Commands

```bash
cargo run -q -p craft -- check library/toml
cargo run -q -p craft -- build library/toml
```

## Design Direction

- keep parsing and allocation policy explicit
- keep the package usable outside `std`
- let this package serve as a small official `craft` example
