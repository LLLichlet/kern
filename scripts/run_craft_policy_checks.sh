#!/bin/bash
set -euo pipefail

ALLOWED_FIXTURE="tools/craft/fixtures/release-policy/allowed"
ALLOWED_EXCEPTION_FIXTURE="tools/craft/fixtures/release-policy/allowed-exception"
BLOCKED_FIXTURE="tools/craft/fixtures/release-policy/blocked"

echo "Running craft release policy allow fixture..."
cargo run -p craft -- check --release "${ALLOWED_FIXTURE}"

echo "Running craft release policy allow-exception fixture..."
cargo run -p craft -- check --release "${ALLOWED_EXCEPTION_FIXTURE}"

echo "Running craft release policy block fixture..."
LOG_FILE="$(mktemp)"
if cargo run -p craft -- check --release "${BLOCKED_FIXTURE}" >"${LOG_FILE}" 2>&1; then
    cat "${LOG_FILE}"
    rm -f "${LOG_FILE}"
    echo "craft release policy fixture unexpectedly passed: ${BLOCKED_FIXTURE}" >&2
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
