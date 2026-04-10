#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

MODE="${1:-all}"
CRAFT_BIN="${CRAFT_BIN:-${ROOT}/target/debug/craft}"
NET_PROJECT="${NET_PROJECT:-${ROOT}/incubator/net}"
PROFILE="${PROFILE:-release}"
RUNS="${RUNS:-5}"
BENCH_ITERS="${BENCH_ITERS:-}"
PERF_ITERS="${PERF_ITERS:-2000000}"
PERF_FREQ="${PERF_FREQ:-400}"
PERF_REPORT_LINES="${PERF_REPORT_LINES:-40}"
KEEP_TMP="${KEEP_TMP:-0}"

usage() {
  cat <<'EOF'
Usage:
  scripts/run_net_http_parse_bench.sh [build|micro|perf|all]

Environment:
  CRAFT_BIN=...    Craft binary to run (defaults to target/debug/craft)
  NET_PROJECT=...  Net workspace root (defaults to incubator/net)
  PROFILE=release  Build profile used for the benchmark binary
  RUNS=5           Number of microbench executions
  BENCH_ITERS=...  Iteration count passed to bench_parse for micro runs
  PERF_ITERS=...   Iteration count passed to bench_parse for perf runs
  PERF_FREQ=400    Sampling frequency for perf record
  PERF_REPORT_LINES=40
                   Number of report lines printed after perf record
  KEEP_TMP=0       Keep copied benchmark workspace when set to 1

The script copies the net workspace to /tmp, runs release builds with craft
timings enabled, then repeatedly executes or profiles the http_parse bench
example. This keeps build state isolated and makes optimization rounds
comparable.
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

TMP_ROOT="$(mktemp -d /tmp/net-http-parse-bench-XXXXXX)"

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
  echo "==> http_parse: cold ${PROFILE} build"
  time_cmd "${CRAFT_BIN}" build --project-path "${project_dir}" --profile "${PROFILE}" --verbose --timings
  echo
  echo "==> http_parse: warm ${PROFILE} build"
  time_cmd "${CRAFT_BIN}" build --project-path "${project_dir}" --profile "${PROFILE}" --verbose --timings
  echo
}

find_bench_binary() {
  local project_dir="$1"
  find "${project_dir}/.craft/build/${PROFILE}/target/out" -path '*/example/bench_parse' -type f | sort | head -n 1
}

run_micro_bench() {
  local project_dir="$1"

  echo "==> http_parse: ensure ${PROFILE} bench binary"
  time_cmd "${CRAFT_BIN}" build --project-path "${project_dir}" --profile "${PROFILE}" --verbose --timings
  echo

  local bench_bin
  bench_bin="$(find_bench_binary "${project_dir}")"
  if [[ -z "${bench_bin}" || ! -x "${bench_bin}" ]]; then
    echo "failed to locate bench_parse binary under ${project_dir}/.craft" >&2
    exit 1
  fi

  for (( i = 1; i <= RUNS; i += 1 )); do
    echo "==> http_parse: microbench run ${i}/${RUNS}"
    if [[ -n "${BENCH_ITERS}" ]]; then
      time_cmd "${bench_bin}" "${BENCH_ITERS}"
    else
      time_cmd "${bench_bin}"
    fi
    echo
  done
}

run_perf_bench() {
  local project_dir="$1"

  echo "==> http_parse: ensure ${PROFILE} bench binary"
  time_cmd "${CRAFT_BIN}" build --project-path "${project_dir}" --profile "${PROFILE}" --verbose --timings
  echo

  local bench_bin
  bench_bin="$(find_bench_binary "${project_dir}")"
  if [[ -z "${bench_bin}" || ! -x "${bench_bin}" ]]; then
    echo "failed to locate bench_parse binary under ${project_dir}/.craft" >&2
    exit 1
  fi

  local perf_data="${TMP_ROOT}/http_parse.perf.data"
  echo "==> http_parse: perf record (${PERF_ITERS} iterations, freq=${PERF_FREQ})"
  perf record -F "${PERF_FREQ}" -g -o "${perf_data}" -- "${bench_bin}" "${PERF_ITERS}"
  echo
  echo "==> http_parse: perf report (${PERF_REPORT_LINES} lines)"
  perf report --stdio -i "${perf_data}" | head -n "${PERF_REPORT_LINES}"
  echo
  echo "perf data: ${perf_data}"
}

PROJECT_DIR="$(prepare_workspace)"

case "${MODE}" in
  build)
    run_build_bench "${PROJECT_DIR}"
    ;;
  micro)
    run_micro_bench "${PROJECT_DIR}"
    ;;
  perf)
    run_perf_bench "${PROJECT_DIR}"
    ;;
  all)
    run_build_bench "${PROJECT_DIR}"
    run_micro_bench "${PROJECT_DIR}"
    run_perf_bench "${PROJECT_DIR}"
    ;;
  *)
    echo "Unknown benchmark mode: ${MODE}" >&2
    echo "Expected one of: build, micro, perf, all" >&2
    exit 1
    ;;
esac
