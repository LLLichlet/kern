#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
PKG="$ROOT/library/toml"
CRAFT="$ROOT/target/debug/craft"
BIN="$PKG/.craft/build/release/target/out/toml-0.1.0/bin/toml-bench"
CORPUS_DIR="$PKG/tests/upstream/toml-test/v2.1.0/tests/valid"
LARGE_FIXTURE="${TMPDIR:-/tmp}/kern_toml_bench_large.toml"
TIME_BIN="/usr/bin/time"

if [[ ! -x "$TIME_BIN" ]]; then
  TIME_BIN="time"
fi

build_release() {
  "$CRAFT" build library/toml --release
}

generate_large_fixture() {
  {
    cat <<'EOF'
title = "kern-toml-bench"
version = 1
enabled = true
ratio = 1.25
tags = ["kern", "toml", "bench", "release"]

[package]
name = "kern.toml"
license = "MIT"
edition = "2026"
metadata = { owner = "kern", stage = "bench", docs = true }
EOF

    for i in $(seq 0 63); do
      printf '\n[table_%03d]\n' "$i"
      printf 'name = "module_%03d"\n' "$i"
      printf 'enabled = true\n'
      printf 'count = %d\n' "$i"
      printf 'ratio = %d.125\n' "$i"
      printf 'created = "2026-04-05T12:34:56Z"\n'
      printf 'tags = ["alpha", "beta", "gamma", "idx-%03d"]\n' "$i"
      printf 'meta = { owner = "team", stage = "prod", idx = %d, path = "src/%03d" }\n' "$i" "$i"
      printf '\n[table_%03d.nested]\n' "$i"
      printf 'path = "src/%03d"\n' "$i"
      printf 'deps = ["std", "toml", "bench"]\n'
      printf 'ports = [80, 443, %d]\n' $((1000 + i))
    done
  } > "$LARGE_FIXTURE"
}

run_case() {
  local label="$1"
  local mode="$2"
  local iterations="$3"
  shift 3
  echo
  echo "==> $label"
  {
    printf '%s\n' "$mode"
    printf '%s\n' "$iterations"
    printf '%s\n' "$@"
  } | "$TIME_BIN" -f 'elapsed=%es user=%Us sys=%Ss maxrss=%MKB' "$BIN"
}

build_release
generate_large_fixture

if [[ $# -gt 0 ]]; then
  if [[ $# -lt 3 ]]; then
    echo "usage: run_bench.sh <scan|parse> <iterations> <file> [<file>...]"
    exit 1
  fi
  run_case "custom" "$@"
  exit 0
fi

if [[ -d "$CORPUS_DIR" ]]; then
  mapfile -t CORPUS_FILES < <(find "$CORPUS_DIR" -type f -name '*.toml' | sort)
else
  CORPUS_FILES=()
fi

if [[ ${#CORPUS_FILES[@]} -gt 0 ]]; then
  run_case "scan upstream corpus" scan 50 "${CORPUS_FILES[@]}"
  run_case "parse upstream corpus" parse 50 "${CORPUS_FILES[@]}"
else
  echo "warning: upstream toml-test corpus not found at $CORPUS_DIR"
fi

run_case "scan synthetic large fixture" scan 100 "$LARGE_FIXTURE"
run_case "parse synthetic large fixture" parse 100 "$LARGE_FIXTURE"
