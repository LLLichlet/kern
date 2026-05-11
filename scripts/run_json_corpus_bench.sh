#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

CRAFT_BIN="${CRAFT_BIN:-${ROOT}/target/debug/craft}"
JSON_PROJECT="${JSON_PROJECT:-${ROOT}/../json-kern}"
PROFILE="${PROFILE:-release}"
JSON_BENCH_ITERS="${JSON_BENCH_ITERS:-2000}"
JSON_CORPUS_DIR="${JSON_CORPUS_DIR:-${JSON_PROJECT}/bench/corpus}"
JSON_CORE_MODES="${JSON_CORE_MODES:-parse clone_doc materialize render_borrowed render_owned}"
JSON_OBJECT_MODES="${JSON_OBJECT_MODES:-clone_indexed_doc}"

usage() {
  cat <<'EOF'
Usage:
  scripts/run_json_corpus_bench.sh [corpus-name...]

Environment:
  CRAFT_BIN=...         Craft binary to run
  JSON_PROJECT=...      json package root (defaults to ../json-kern)
  PROFILE=release       Build profile for the benchmark example
  JSON_BENCH_ITERS=...  Iteration count passed to bench_json
  JSON_CORPUS_DIR=...   Directory containing corpus files
  JSON_CORE_MODES=...   Space-separated corpus-safe modes
  JSON_OBJECT_MODES=... Extra modes run only when the corpus root is an object

When no corpus names are passed, the script looks for the standard corpus files:
  twitter.json
  github_events.json
  citm_catalog.json
  canada.json
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

if [[ ! -x "${CRAFT_BIN}" ]]; then
  echo "building craft debug binary..." >&2
  cargo build -p craft --quiet
fi

if [[ ! -f "${JSON_PROJECT}/Craft.toml" ]]; then
  echo "missing json project at ${JSON_PROJECT}" >&2
  exit 1
fi

time_cmd() {
  if [[ -x /usr/bin/time ]]; then
    /usr/bin/time -f '%E real' "$@"
  else
    "$@"
  fi
}

build_bench() {
  echo "==> json corpus bench: ensure ${PROFILE} bench_json example"
  time_cmd "${CRAFT_BIN}" build --project-path "${JSON_PROJECT}" --profile "${PROFILE}" --examples
  echo
}

find_bench_bin() {
  find "${JSON_PROJECT}/.craft/build/${PROFILE}/target/out" -type f -path '*/json-0.1.0/example/bench_json' | sort | head -n 1
}

first_non_ws_char() {
  awk 'match($0, /[^[:space:]]/) { print substr($0, RSTART, 1); exit }' "$1"
}

run_mode() {
  local bench_bin="$1"
  local mode="$2"
  local corpus_path="$3"

  echo "==> ${mode} :: ${corpus_path}"
  "${bench_bin}" "${JSON_BENCH_ITERS}" "${mode}" "${corpus_path}"
  echo
}

run_corpus() {
  local bench_bin="$1"
  local corpus_path="$2"

  if [[ ! -f "${corpus_path}" ]]; then
    echo "skip missing corpus: ${corpus_path}" >&2
    return 0
  fi

  local root_char
  root_char="$(first_non_ws_char "${corpus_path}")"

  for mode in ${JSON_CORE_MODES}; do
    run_mode "${bench_bin}" "${mode}" "${corpus_path}"
  done

  if [[ "${root_char}" == "{" ]]; then
    for mode in ${JSON_OBJECT_MODES}; do
      run_mode "${bench_bin}" "${mode}" "${corpus_path}"
    done
  else
    echo "skip object-root modes for ${corpus_path}: root=${root_char:-<empty>}"
    echo
  fi
}

build_bench
BENCH_BIN="$(find_bench_bin)"

if [[ -z "${BENCH_BIN}" || ! -x "${BENCH_BIN}" ]]; then
  echo "failed to locate bench_json example output" >&2
  exit 1
fi

if (( $# > 0 )); then
  for name in "$@"; do
    if [[ "${name}" == */* ]]; then
      run_corpus "${BENCH_BIN}" "${name}"
    else
      run_corpus "${BENCH_BIN}" "${JSON_CORPUS_DIR}/${name}"
    fi
  done
  exit 0
fi

run_corpus "${BENCH_BIN}" "${JSON_CORPUS_DIR}/twitter.json"
run_corpus "${BENCH_BIN}" "${JSON_CORPUS_DIR}/github_events.json"
run_corpus "${BENCH_BIN}" "${JSON_CORPUS_DIR}/citm_catalog.json"
run_corpus "${BENCH_BIN}" "${JSON_CORPUS_DIR}/canada.json"
