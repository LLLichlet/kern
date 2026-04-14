# JSON Benchmarks

`examples/bench_json.rn` now supports both the built-in sample payload and an
external corpus file.

Usage:

```text
bench_json [iterations] [mode] [path|-]
```

- omit `mode` to run the full suite
- omit `path` to use the built-in sample document
- pass `-` as `path` to read JSON from stdin

Examples:

```text
bench_json 120000 clone_arena
bench_json 2000 clone_doc bench/corpus/twitter.json
bench_json 2000 parse bench/corpus/twitter.json
bench_json 2000 materialize bench/corpus/citm_catalog.json
bench_json 2000 lookup_indexed_doc bench/corpus/github_events.json
cat bench/corpus/canada.json | bench_json 2000 parse -
```

Repository runner:

```text
scripts/fetch_json_bench_corpora.sh
scripts/run_json_corpus_bench.sh
scripts/run_json_corpus_bench.sh twitter.json
scripts/run_json_corpus_bench.sh /abs/path/to/custom.json
```

Typical flow:

```text
scripts/fetch_json_bench_corpora.sh
scripts/run_json_corpus_bench.sh
```

Each benchmark line reports:

- `input`: sample, file path, or `<stdin>`
- `bytes`: input bytes per iteration
- `ops_per_sec`: document operations per second
- `bytes_per_sec`: throughput on the selected corpus

Suggested standard corpus names:

- `bench/corpus/twitter.json`
- `bench/corpus/github_events.json`
- `bench/corpus/citm_catalog.json`
- `bench/corpus/canada.json`

Those names intentionally match common JSON benchmark corpora so Kern numbers
can later be compared against mature libraries on the same inputs.

The fetch script clones `yyjson_benchmark` into a temporary directory and copies
those four standard files into `bench/corpus/`.

Mode guidance:

- default full-suite and corpus script: `parse`, `clone_doc`, `materialize`, `render_borrowed`, `render_owned`
- default object-root add-ons: `clone_indexed_doc`
- explicit low-level diagnostic/stress modes only: `clone_gpa`, `clone_arena`, `build_indexed`, `build_indexed_arena`, `lookup_indexed`
- sample/schema-specific high-level modes: `lookup_borrowed`, `lookup_owned`, `lookup_indexed_doc`

`lookup_indexed_doc` is intentionally not part of the default corpus script,
because it measures a schema-specific nested lookup path and only produces
meaningful numbers on matching object documents.

`clone_doc` is the pure high-level owning-document benchmark. `materialize`
keeps its broader library-style pipeline semantics and includes owned
construction plus `compact_size()` and representative typed field access.
