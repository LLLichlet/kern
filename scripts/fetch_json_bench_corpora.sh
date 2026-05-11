#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

DEST_DIR="${DEST_DIR:-${ROOT}/../json-kern/bench/corpus}"
SRC_REPO="${SRC_REPO:-https://github.com/ibireme/yyjson_benchmark.git}"
SRC_REF="${SRC_REF:-master}"

usage() {
  cat <<'EOF'
Usage:
  scripts/fetch_json_bench_corpora.sh

Environment:
  DEST_DIR=...   Destination directory for copied corpus files
  SRC_REPO=...   Source repository containing benchmark corpus data
  SRC_REF=...    Git branch or tag to checkout

This script clones the benchmark-data repository into a temporary directory and
copies the standard corpus files expected by json-kern:

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

if ! command -v git >/dev/null 2>&1; then
  echo "git is required to fetch benchmark corpora" >&2
  exit 1
fi

mkdir -p "${DEST_DIR}"
TMP_ROOT="$(mktemp -d /tmp/json-corpus-fetch-XXXXXX)"
trap 'rm -rf "${TMP_ROOT}"' EXIT

SRC_DIR="${TMP_ROOT}/src"

echo "==> clone corpus source"
git clone --depth=1 --branch "${SRC_REF}" "${SRC_REPO}" "${SRC_DIR}"

copy_one() {
  local name="$1"
  local src=""
  local dst="${DEST_DIR}/${name}"

  if [[ -f "${SRC_DIR}/data/json/${name}" ]]; then
    src="${SRC_DIR}/data/json/${name}"
  elif [[ -f "${SRC_DIR}/data/${name}" ]]; then
    src="${SRC_DIR}/data/${name}"
  fi

  if [[ -z "${src}" ]]; then
    echo "missing expected corpus file in source repo: ${name}" >&2
    exit 1
  fi

  cp "${src}" "${dst}"
  printf 'copied %s -> %s\n' "${src}" "${dst}"
}

copy_one twitter.json
copy_one github_events.json
copy_one citm_catalog.json
copy_one canada.json

echo
echo "json benchmark corpora ready under ${DEST_DIR}"
