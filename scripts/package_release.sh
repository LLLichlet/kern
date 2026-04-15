#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

usage() {
    cat <<'EOF'
Usage:
  scripts/package_release.sh [VERSION] [TARGET] [--skip-build]

Arguments:
  VERSION       Archive version label, defaults to "dev"
  TARGET        Target triple label in the archive name
  --skip-build  Reuse existing release binaries instead of rebuilding
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    usage
    exit 0
fi

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
cp -r library/base "${DIST_NAME}/lib/kern/"
cp -r library/rt "${DIST_NAME}/lib/kern/"
cp -r library/sys "${DIST_NAME}/lib/kern/"
cp -r library/std "${DIST_NAME}/lib/kern/"
cp README.md LICENSE "${DIST_NAME}/"
tar -czf "${TARBALL}" "${DIST_NAME}"

echo "Successfully packaged: ${TARBALL}"
