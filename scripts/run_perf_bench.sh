#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

MODE="${1:-all}"
CRAFT_BIN="${CRAFT_BIN:-${ROOT}/target/debug/craft}"
TOML_PROJECT="${TOML_PROJECT:-/home/lenovo/toml}"
WORKSPACE_PROJECT="${WORKSPACE_PROJECT:-}"

if [[ ! -x "${CRAFT_BIN}" ]]; then
  echo "building craft debug binary..." >&2
  cargo build -p craft --quiet
fi

TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "${TMP_ROOT}"' EXIT

time_cmd() {
  if [[ -x /usr/bin/time ]]; then
    /usr/bin/time -f '%E real' "$@"
  else
    "$@"
  fi
}

prepare_hello_project() {
  local dest="$1"
  mkdir -p "${dest}/src"
  cat > "${dest}/Craft.toml" <<'EOF'
[package]
name = "hello-bench"
version = "0.1.0"
kern = "0.7.0"

[profile.release]
opt = 3

[[bin]]
name = "hello-bench"
root = "src/main.rn"
EOF
  cat > "${dest}/src/main.rn" <<'EOF'
use std.io;

fn main() i32 {
    io.println("hello, {}!", .{"bench",});
    return 0;
}
EOF
}

copy_project() {
  local source_dir="$1"
  local dest_dir="$2"
  rsync -a --exclude .git --exclude .craft "${source_dir}/" "${dest_dir}/"
}

run_release_bench() {
  local label="$1"
  local project_dir="$2"

  echo "==> ${label}: cold release build"
  time_cmd "${CRAFT_BIN}" build --project-path "${project_dir}" --profile release --verbose --timings
  echo
  echo "==> ${label}: warm release build"
  time_cmd "${CRAFT_BIN}" build --project-path "${project_dir}" --profile release --verbose --timings
  echo
}

run_hello() {
  local project_dir="${TMP_ROOT}/hello"
  prepare_hello_project "${project_dir}"
  run_release_bench "hello" "${project_dir}"
}

run_toml() {
  if [[ ! -f "${TOML_PROJECT}/Craft.toml" ]]; then
    echo "skipping toml benchmark: missing project at ${TOML_PROJECT}" >&2
    return 0
  fi

  local project_dir="${TMP_ROOT}/toml"
  copy_project "${TOML_PROJECT}" "${project_dir}"
  run_release_bench "toml" "${project_dir}"
}

run_workspace() {
  if [[ -z "${WORKSPACE_PROJECT}" ]]; then
    if [[ "${MODE}" == "workspace" ]]; then
      echo "WORKSPACE_PROJECT is required for workspace benchmarks" >&2
      exit 1
    fi
    echo "skipping workspace benchmark: set WORKSPACE_PROJECT=/path/to/workspace" >&2
    return 0
  fi

  if [[ ! -f "${WORKSPACE_PROJECT}/Craft.toml" ]]; then
    echo "workspace benchmark project is missing Craft.toml: ${WORKSPACE_PROJECT}" >&2
    exit 1
  fi

  local project_dir="${TMP_ROOT}/workspace"
  copy_project "${WORKSPACE_PROJECT}" "${project_dir}"
  run_release_bench "workspace" "${project_dir}"
}

case "${MODE}" in
  hello)
    run_hello
    ;;
  toml)
    run_toml
    ;;
  workspace)
    run_workspace
    ;;
  all)
    run_hello
    run_toml
    run_workspace
    ;;
  *)
    echo "Unknown benchmark mode: ${MODE}" >&2
    echo "Expected one of: hello, toml, workspace, all" >&2
    exit 1
    ;;
esac
