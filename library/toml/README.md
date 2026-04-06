# `toml`

`toml` is an independent Kern package hosted in the repository under
`library/toml/`.

It is intentionally not part of `std`. The package exists to exercise `craft`
with a real library target while providing a natural home for future TOML
parsing, document modeling, and configuration helpers.

## Status

The package now has two layers:

- `parse(text)` is the primary user-facing API. It returns a managed TOML
  document with an internal fixed allocation strategy chosen by the package.
- `scan(text)` is the lower-level lightweight validation/classification pass.

The low-level scanner remains allocation-free:

- blank lines and `#` comments
- bare keys plus basic/literal quoted key segments in dotted key paths
- `[table]` and `[[array-of-tables]]` headers, including trailing comments
- decimal plus hex/octal/binary integer, decimal/special float, boolean, single-line and multiline basic/literal string, array, inline table, and local/offset date-time values
- single-line and multiline arrays, including comments and trailing commas
- duplicate key/table namespace validation for the common scalar-vs-table conflicts
- document summary counts for root keys, table-local keys, tables, array tables, and total items

The package also exposes an explicit model layer for programmatic construction
and mutation:

- managed `Document` as the default package-level document handle
- managed `TableRef` / `ArrayRef` mutation helpers that do not require callers
  to thread an allocator through every operation
- `OwnedDocument` for root keys, named tables, and array-of-tables sections
- deterministic document-level section ordering across tables and array-of-tables
- `Table` for ordered key/value storage with `set_*`, `get_*`, `set_path_*`, `get_path_*`, `ensure_path_*`, and `remove_path`
- `OwnedDocument` section mutation with `ensure_table`, `append_array_table`, `remove_table`, `remove_array_table`, and single-item array-table removal
- `OwnedDocument` path lookups across root keys and named tables via `get_path_*` and `get_table_path`
- `Array` for explicit owned TOML arrays with typed push/insert/set/remove/clear helpers
- `render_document` for deterministic TOML emission from the owned model
- low-level owned APIs take a caller-supplied allocator and report `ModelError`

The default managed `parse(text)` path currently fixes its memory strategy
internally:

- `toml` is treated as a user-space ecosystem package rather than a `std` peer
- the package chooses a concrete strategy instead of making ordinary users pick
  allocators
- the current fixed strategy is optimized for parse/edit/drop style workloads

The underlying model still uses ordered `List` storage rather than `Map` or
`Tree`:

- TOML editing and rendering want stable insertion order
- small and medium configuration documents benefit from compact contiguous
  storage
- allocator and memory behavior stay explicit, simple, and easy to profile
- a secondary lookup index can still be added later if real workloads justify
  it

It is still a bootstrap parser rather than a full TOML implementation:

- `parse` is implemented for the current bootstrap surface, including nested table and array-table headers under array-of-tables
- rendering is deterministic but not round-trip preserving yet
- `scan` remains borrowed and allocation-free

## Layout

```text
library/toml/
  Craft.toml
  README.md
  scripts/
    run_toml_test.sh
    sync_toml_test_corpus.sh
    run_bench.sh
  src/
    lib.rn
    document.rn
    error.rn
    toml_bench.rn
    parser/
      init.rn
      common.rn
      path.rn
      key.rn
      value.rn
      value_string.rn
      value_number.rn
      value_compound.rn
      owned_decode.rn
      owned.rn
```

## Commands

```bash
cargo run -q -p craft -- check library/toml
cargo run -q -p craft -- build library/toml
cargo run -q -p craft -- test library/toml
printf 'title = "demo"\n' | cargo run -q -p craft -- run library/toml
library/toml/scripts/run_toml_test.sh -parallel 1
library/toml/scripts/run_bench.sh
library/toml/scripts/sync_toml_test_corpus.sh
```

The package also ships a `toml-test-decoder` binary target. It reads TOML from
stdin and emits the tagged JSON shape expected by `toml-test`, which makes it a
useful bridge for future compliance runs against the upstream TOML corpus.

The `toml-test` integration is split in two:

- `scripts/run_toml_test.sh` builds the decoder and runs the official upstream
  compliance suite through the `toml-test` runner.
- `scripts/sync_toml_test_corpus.sh` copies the upstream embedded test corpus
  into `tests/upstream/toml-test/<version>/` for direct local inspection or
  custom harnesses.

Both scripts expect `toml-test` to be installed locally:

```bash
go install github.com/toml-lang/toml-test/v2/cmd/toml-test@latest
```

The package also ships a `toml-bench` binary plus `scripts/run_bench.sh`:

- `toml-bench` performs repeated `scan` or `parse` runs over one or more input
  files and prints a checksum so the work is not
  optimized away. It reads a small manifest from stdin rather than using
  positional CLI arguments.
- `run_bench.sh` builds the release binary, runs it over the upstream valid
  corpus when available, and also generates a larger synthetic TOML fixture for
  a less toy-sized parser workload.

Example custom runs:

```bash
library/toml/scripts/run_bench.sh scan 500 path/to/file.toml
library/toml/scripts/run_bench.sh parse 500 path/to/file.toml
```

## Design Direction

- make `parse(text)` the package's best default rather than an allocator-shaped API
- keep `scan(text)` as the lower-level escape hatch
- keep the package usable outside `std`
- let this package serve as a small official `craft` example
- use it to dogfood `std`, `craft`, and compiler package-boundary behavior
