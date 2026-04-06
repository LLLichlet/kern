#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

MODE="${1:-all}"

SMOKE_TESTS=(
  anonymous_aggregates
  atomics
  regressions
  stdlib
  traits
)

HOSTED_TESTS=(
  collections
  filesystem
)

run_group() {
  local label="$1"
  shift

  echo "Running ${label} suite..."
  for test_name in "$@"; do
    cargo test -p kernc_cli --test "${test_name}"
  done
}

case "${MODE}" in
  smoke)
    run_group "smoke" "${SMOKE_TESTS[@]}"
    ;;
  hosted)
    run_group "hosted" "${HOSTED_TESTS[@]}"
    ;;
  all)
    run_group "smoke" "${SMOKE_TESTS[@]}"
    run_group "hosted" "${HOSTED_TESTS[@]}"
    ;;
  *)
    echo "Unknown test mode: ${MODE}" >&2
    echo "Expected one of: smoke, hosted, all" >&2
    exit 1
    ;;
esac
