# `toml`

`toml` is an independent Kern package hosted in the repository under
`library/toml/`.

It is intentionally not part of `std`. The package exists to exercise `craft`
with a real library target while providing a natural home for future TOML
parsing, document modeling, and configuration helpers.

## Status

The package has moved past pure classification and now performs a lightweight,
allocation-free validation pass:

- blank lines and `#` comments
- bare keys plus basic/literal quoted key segments in dotted key paths
- `[table]` headers
- integer, decimal float, boolean, basic/literal string, array, and inline table values
- document summary counts for root keys, table-local keys, tables, and total items

It is still a bootstrap parser rather than a full TOML implementation:

- multiline strings/arrays are not supported yet
- datetimes and array-of-tables are not supported yet
- parsing remains borrowed and allocation-free

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
cargo run -q -p craft -- test library/toml
```

## Design Direction

- keep parsing and allocation policy explicit
- keep the package usable outside `std`
- let this package serve as a small official `craft` example
- use it to dogfood `std`, `craft`, and compiler package-boundary behavior
