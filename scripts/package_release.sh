#!/bin/bash
set -e

# 如果没有传参，默认为 dev 版本
VERSION=${1:-"dev"}
TARGET=${2:-"x86_64-linux-gnu"}
DIST_NAME="kern-${VERSION}-${TARGET}"
TARBALL="${DIST_NAME}.tar.gz"

echo "Building release binary..."
cargo build --release

echo "Packaging ${DIST_NAME}..."
# 按照标准 Linux 目录结构组织
mkdir -p "${DIST_NAME}/bin" "${DIST_NAME}/lib/kern"

# 拷贝二进制
cp target/release/kernc "${DIST_NAME}/bin/"

# 拷贝标准库
cp -r library/std "${DIST_NAME}/lib/kern/"

# 拷贝文档
cp README.md LICENSE "${DIST_NAME}/"

# 压缩打包
tar -czf "${TARBALL}" "${DIST_NAME}"

echo "Successfully packaged: ${TARBALL}"