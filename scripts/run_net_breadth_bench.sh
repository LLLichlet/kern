#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

MODE="${1:-all}"
CRAFT_BIN="${CRAFT_BIN:-${ROOT}/target/debug/craft}"
NET_PROJECT="${NET_PROJECT:-${ROOT}/incubator/net}"
PROFILE="${PROFILE:-release}"
KEEP_TMP="${KEEP_TMP:-0}"
BENCH_ITERS="${BENCH_ITERS:-200000}"

usage() {
  cat <<'EOF'
Usage:
  scripts/run_net_breadth_bench.sh [build|run|all]

Environment:
  CRAFT_BIN=...    Craft binary to run (defaults to target/debug/craft)
  NET_PROJECT=...  Net workspace root (defaults to incubator/net)
  PROFILE=release  Build profile for the copied workspace
  BENCH_ITERS=...  Iteration count passed to http_parse bench_parse
  KEEP_TMP=0       Keep copied benchmark workspace when set to 1

The script copies net to /tmp, runs cold and warm workspace builds, then
executes a representative set of root/package binaries and examples. The goal
is to keep build-speed work grounded in more than one package hotspot.
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

TMP_ROOT="$(mktemp -d /tmp/net-breadth-bench-XXXXXX)"

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
  echo "==> net breadth: cold ${PROFILE} build"
  time_cmd "${CRAFT_BIN}" build --project-path "${project_dir}" --profile "${PROFILE}" --verbose --timings
  echo
  echo "==> net breadth: warm ${PROFILE} build"
  time_cmd "${CRAFT_BIN}" build --project-path "${project_dir}" --profile "${PROFILE}" --verbose --timings
  echo
}

find_artifact() {
  local project_dir="$1"
  local suffix="$2"
  find "${project_dir}/.craft/build/${PROFILE}/target/out" -type f -path "*${suffix}" | sort | head -n 1
}

print_size() {
  local path="$1"
  if stat --format='%s' "${path}" >/dev/null 2>&1; then
    printf '%s bytes\n' "$(stat --format='%s' "${path}")"
  else
    wc -c < "${path}"
  fi
}

run_target() {
  local label="$1"
  local path="$2"
  shift 2

  if [[ -z "${path}" || ! -x "${path}" ]]; then
    echo "missing artifact for ${label}" >&2
    exit 1
  fi

  echo "==> ${label}"
  printf 'artifact  %s\n' "${path}"
  printf 'size      %s\n' "$(print_size "${path}")"
  time_cmd "${path}" "$@"
  echo
}

run_runtime_bench() {
  local project_dir="$1"

  echo "==> net breadth: ensure ${PROFILE} artifacts (--examples)"
  time_cmd "${CRAFT_BIN}" build --project-path "${project_dir}" --profile "${PROFILE}" --examples --verbose --timings
  echo

  local scenario_report
  local gateway_trace
  local hello_client
  local hello_server
  local hello_compact
  local bench_json
  local bench_parse
  local hello_upgrade
  local hello_handshake
  local echo_session

  scenario_report="$(find_artifact "${project_dir}" '/net-0.1.0/bin/scenario_report')"
  gateway_trace="$(find_artifact "${project_dir}" '/net-0.1.0/example/gateway_trace')"
  hello_client="$(find_artifact "${project_dir}" '/http_client-0.1.0/example/hello_client')"
  hello_server="$(find_artifact "${project_dir}" '/http_server-0.1.0/example/hello_server')"
  hello_compact="$(find_artifact "${project_dir}" '/json-0.1.0/example/hello_compact')"
  bench_json="$(find_artifact "${project_dir}" '/json-0.1.0/example/bench_json')"
  bench_parse="$(find_artifact "${project_dir}" '/http_parse-0.1.0/example/bench_parse')"
  hello_upgrade="$(find_artifact "${project_dir}" '/websocket-0.1.0/example/hello_upgrade')"
  hello_handshake="$(find_artifact "${project_dir}" '/websocket_client-0.1.0/example/hello_handshake')"
  echo_session="$(find_artifact "${project_dir}" '/websocket_server-0.1.0/example/echo_session')"

  run_target "root scenario_report" "${scenario_report}"
  run_target "root gateway_trace" "${gateway_trace}"
  run_target "http_client hello_client" "${hello_client}"
  run_target "http_server hello_server" "${hello_server}"
  run_target "json hello_compact" "${hello_compact}"
  run_target "json bench_json (${BENCH_ITERS} iters)" "${bench_json}" "${BENCH_ITERS}"
  run_target "http_parse bench_parse (${BENCH_ITERS} iters)" "${bench_parse}" "${BENCH_ITERS}"
  run_target "websocket hello_upgrade" "${hello_upgrade}"
  run_target "websocket_client hello_handshake" "${hello_handshake}"
  run_target "websocket_server echo_session" "${echo_session}"
}

PROJECT_DIR="$(prepare_workspace)"

case "${MODE}" in
  build)
    run_build_bench "${PROJECT_DIR}"
    ;;
  run)
    run_runtime_bench "${PROJECT_DIR}"
    ;;
  all)
    run_build_bench "${PROJECT_DIR}"
    run_runtime_bench "${PROJECT_DIR}"
    ;;
  *)
    echo "Unknown benchmark mode: ${MODE}" >&2
    echo "Expected one of: build, run, all" >&2
    exit 1
    ;;
esac
