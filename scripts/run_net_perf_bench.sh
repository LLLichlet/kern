#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

MODE="${1:-all}"
CRAFT_BIN="${CRAFT_BIN:-${ROOT}/target/debug/craft}"
NET_PROJECT="${NET_PROJECT:-${ROOT}/incubator/net}"
PROFILE="${PROFILE:-release}"
KEEP_TMP="${KEEP_TMP:-0}"

usage() {
  cat <<'EOF'
Usage:
  scripts/run_net_perf_bench.sh [build|test|all]

Environment:
  CRAFT_BIN=...    Craft binary to run (defaults to target/debug/craft)
  NET_PROJECT=...  Net workspace root (defaults to incubator/net)
  PROFILE=release  Build profile for build mode
  KEEP_TMP=0       Keep copied benchmark workspace when set to 1

The script copies the net workspace to /tmp, then runs cold and warm craft
invocations with timings enabled. This avoids contaminating the source tree's
incremental state and gives a repeatable baseline for perf work.
EOF
}

if [[ "${MODE}" == "--help" || "${MODE}" == "-h" ]]; then
  usage
  exit 0
fi

if [[ ! -x "${CRAFT_BIN}" ]]; then
  echo "building craft debug binary..." >&2
  cargo build -p craft --quiet
fi

if [[ ! -f "${NET_PROJECT}/Craft.toml" ]]; then
  echo "missing Craft.toml under ${NET_PROJECT}" >&2
  exit 1
fi

TMP_ROOT="$(mktemp -d /tmp/net-bench-XXXXXX)"

cleanup() {
  if (( KEEP_TMP == 0 )); then
    rm -rf "${TMP_ROOT}"
  else
    echo "kept benchmark workspace at ${TMP_ROOT}"
  fi
}

trap cleanup EXIT

time_cmd() {
  if [[ -x /usr/bin/time ]]; then
    /usr/bin/time -f '%E real' "$@"
  else
    "$@"
  fi
}

prepare_workspace() {
  local dest="${TMP_ROOT}/net"
  rsync -a --exclude .git --exclude .craft "${NET_PROJECT}/" "${dest}/"
  printf '%s\n' "${dest}"
}

run_build_bench() {
  local project_dir="$1"
  echo "==> net: cold ${PROFILE} build"
  time_cmd "${CRAFT_BIN}" build --project-path "${project_dir}" --profile "${PROFILE}" --verbose --timings
  echo
  echo "==> net: warm ${PROFILE} build"
  time_cmd "${CRAFT_BIN}" build --project-path "${project_dir}" --profile "${PROFILE}" --verbose --timings
  echo
}

run_test_bench() {
  local project_dir="$1"
  echo "==> net: cold test"
  time_cmd "${CRAFT_BIN}" test --project-path "${project_dir}" --verbose --timings
  echo
  echo "==> net: warm test"
  time_cmd "${CRAFT_BIN}" test --project-path "${project_dir}" --verbose --timings
  echo
}

PROJECT_DIR="$(prepare_workspace)"

case "${MODE}" in
  build)
    run_build_bench "${PROJECT_DIR}"
    ;;
  test)
    run_test_bench "${PROJECT_DIR}"
    ;;
  all)
    run_build_bench "${PROJECT_DIR}"
    run_test_bench "${PROJECT_DIR}"
    ;;
  *)
    echo "Unknown benchmark mode: ${MODE}" >&2
    echo "Expected one of: build, test, all" >&2
    exit 1
    ;;
esac
