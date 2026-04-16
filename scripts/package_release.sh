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

detect_host_target() {
    local os_name arch os

    os_name=$(uname -s | tr '[:upper:]' '[:lower:]')
    case "${os_name}" in
        linux)
            os="linux-gnu"
            ;;
        darwin)
            os="apple-darwin"
            ;;
        *)
            echo "unsupported-${os_name}"
            return
            ;;
    esac

    arch=$(uname -m)
    case "${arch}" in
        x86_64|amd64)
            arch="x86_64"
            ;;
        aarch64|arm64)
            arch="aarch64"
            ;;
        *)
            echo "unsupported-${arch}-${os}"
            return
            ;;
    esac

    echo "${arch}-${os}"
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
HOST_TARGET="$(detect_host_target)"

if [[ "${HOST_TARGET}" == unsupported-* ]]; then
    echo "Unsupported host for Unix packaging: ${HOST_TARGET}" >&2
    exit 1
fi

if [[ "${TARGET}" != "${HOST_TARGET}" ]]; then
    echo "Target label '${TARGET}' does not match the current host '${HOST_TARGET}'." >&2
    echo "This Unix packaging script is host-native only and packages binaries from target/release." >&2
    echo "Run it on a matching host machine or teach the script an explicit cross-target build path first." >&2
    exit 1
fi

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
