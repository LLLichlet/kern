# Corpus Layout

This directory is intentionally not populated in-repo because benchmark corpora
are large.

Expected file names:

- `twitter.json`
- `github_events.json`
- `citm_catalog.json`
- `canada.json`

Those names match common JSON benchmark corpora so the same files can be used
for Kern and external-library comparisons.

Populate them with:

```text
scripts/fetch_json_bench_corpora.sh
```

With the files in place, run:

```text
scripts/run_json_corpus_bench.sh
```

Or target a specific corpus:

```text
scripts/run_json_corpus_bench.sh twitter.json
scripts/run_json_corpus_bench.sh /abs/path/to/custom.json
```
