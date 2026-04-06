#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

# Avoid AppleDouble metadata files when packaging on macOS runners.
export COPYFILE_DISABLE=1

VERSION=${1:-"dev"}
TARGET=${2:-"x86_64-linux-gnu"}
SKIP_BUILD=${3:-""}
DIST_NAME="kern-${VERSION}-${TARGET}"
TARBALL="${DIST_NAME}.tar.gz"

if [ "${SKIP_BUILD}" != "--skip-build" ]; then
    echo "Building release binaries..."
    cargo build --release -p kernc_cli --bin kernc
    cargo build --release -p craft
    cargo build --release -p kern-lsp
fi

echo "Packaging ${DIST_NAME}..."
rm -rf "${DIST_NAME}" "${TARBALL}"
mkdir -p "${DIST_NAME}/bin" "${DIST_NAME}/lib/kern"

cp target/release/kernc "${DIST_NAME}/bin/"
cp target/release/craft "${DIST_NAME}/bin/"
cp target/release/kern-lsp "${DIST_NAME}/bin/"
cp -r library/std "${DIST_NAME}/lib/kern/"
cp README.md LICENSE "${DIST_NAME}/"
tar -czf "${TARBALL}" "${DIST_NAME}"

echo "Successfully packaged: ${TARBALL}"
