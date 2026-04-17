#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOCAL_INSTALLER="${ROOT}/ops/install.py"
REMOTE_INSTALLER_URL="https://raw.githubusercontent.com/softfault/kern/main/ops/install.py"

if ! command -v python3 >/dev/null 2>&1; then
    echo "Error: python3 was not found in PATH." >&2
    exit 1
fi

if [[ -f "${LOCAL_INSTALLER}" ]]; then
    exec python3 "${LOCAL_INSTALLER}" "$@"
fi

if ! command -v curl >/dev/null 2>&1; then
    echo "Error: curl was not found in PATH." >&2
    exit 1
fi

TMP_INSTALLER="$(mktemp "${TMPDIR:-/tmp}/kern-install-XXXXXX.py")"
trap 'rm -f "${TMP_INSTALLER}"' EXIT
curl -fsSL "${REMOTE_INSTALLER_URL}" -o "${TMP_INSTALLER}"
exec python3 "${TMP_INSTALLER}" "$@"
