#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ALLOWED_FIXTURE="${ROOT}/tools/craft/fixtures/release-policy/allowed"
ALLOWED_EXCEPTION_FIXTURE="${ROOT}/tools/craft/fixtures/release-policy/allowed-exception"
BLOCKED_FIXTURE="${ROOT}/tools/craft/fixtures/release-policy/blocked"

CURRENT_KERN_VERSION="$(
    sed -n '/^\[workspace\.package\]/,/^\[/{s/^version = "\(.*\)"$/\1/p}' "${ROOT}/Cargo.toml" \
        | head -n 1
)"

if [[ -z "${CURRENT_KERN_VERSION}" ]]; then
    echo "failed to resolve current workspace version from ${ROOT}/Cargo.toml" >&2
    exit 1
fi

TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "${TMP_ROOT}"' EXIT

prepare_fixture() {
    local source_dir="$1"
    local dest_dir="${TMP_ROOT}/$(basename "${source_dir}")"
    cp -r "${source_dir}" "${dest_dir}"
    sed -i "s/^kern = \".*\"$/kern = \"${CURRENT_KERN_VERSION}\"/" "${dest_dir}/Craft.toml"
    printf '%s\n' "${dest_dir}"
}

ALLOWED_PATH="$(prepare_fixture "${ALLOWED_FIXTURE}")"
ALLOWED_EXCEPTION_PATH="$(prepare_fixture "${ALLOWED_EXCEPTION_FIXTURE}")"
BLOCKED_PATH="$(prepare_fixture "${BLOCKED_FIXTURE}")"

echo "Running craft release policy allow fixture..."
cargo run -p craft -- check --release "${ALLOWED_PATH}"

echo "Running craft release policy allow-exception fixture..."
cargo run -p craft -- check --release "${ALLOWED_EXCEPTION_PATH}"

echo "Running craft release policy block fixture..."
LOG_FILE="$(mktemp)"
if cargo run -p craft -- check --release "${BLOCKED_PATH}" >"${LOG_FILE}" 2>&1; then
    cat "${LOG_FILE}"
    rm -f "${LOG_FILE}"
    echo "craft release policy fixture unexpectedly passed: ${BLOCKED_PATH}" >&2
    exit 1
fi
if ! grep -q "release source policy rejected" "${LOG_FILE}"; then
    cat "${LOG_FILE}"
    rm -f "${LOG_FILE}"
    echo "craft release policy fixture failed for an unexpected reason" >&2
    exit 1
fi
rm -f "${LOG_FILE}"

echo "craft release policy fixtures passed"
