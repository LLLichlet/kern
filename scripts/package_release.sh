#!/bin/bash
set -e

VERSION=${1:-"dev"}
TARGET=${2:-"x86_64-linux-gnu"}
SKIP_BUILD=${3:-""}
DIST_NAME="kern-${VERSION}-${TARGET}"
TARBALL="${DIST_NAME}.tar.gz"

if [ "${SKIP_BUILD}" != "--skip-build" ]; then
    echo "Building release binary..."
    cargo build --release
fi

echo "Packaging ${DIST_NAME}..."
rm -rf "${DIST_NAME}" "${TARBALL}"
mkdir -p "${DIST_NAME}/bin" "${DIST_NAME}/lib/kern"

cp target/release/kernc "${DIST_NAME}/bin/"
cp -r library/std "${DIST_NAME}/lib/kern/"
cp README.md LICENSE "${DIST_NAME}/"
tar -czf "${TARBALL}" "${DIST_NAME}"

echo "Successfully packaged: ${TARBALL}"
