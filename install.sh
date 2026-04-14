#!/bin/bash
set -e

echo "Welcome to the Kern Programming Language Installer!"

# 动态获取 GitHub 上最新的 Release Tag
echo "=> Fetching latest version info from GitHub..."
LATEST_VERSION=$(curl -s https://api.github.com/repos/softfault/kern/releases/latest | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')

if [ -z "$LATEST_VERSION" ]; then
    echo "Warning: Failed to fetch latest version from GitHub."
    echo "Falling back to default fallback version."
    VERSION="v0.7.0"
else
    VERSION="$LATEST_VERSION"
fi

echo "=> Preparing to install Kern ${VERSION} toolchain..."

# 动态探测操作系统
OS_NAME=$(uname -s | tr '[:upper:]' '[:lower:]')
if [ "$OS_NAME" = "linux" ]; then
    OS="linux-gnu"
elif [ "$OS_NAME" = "darwin" ]; then
    OS="apple-darwin"
else
    echo "Unsupported OS: $OS_NAME"
    exit 1
fi

# 动态探测 CPU 架构
ARCH=$(uname -m)
if [ "$ARCH" = "aarch64" ] || [ "$ARCH" = "arm64" ]; then
    ARCH="aarch64"
elif [ "$ARCH" = "x86_64" ] || [ "$ARCH" = "amd64" ]; then
    ARCH="x86_64"
else
    echo "Unsupported Architecture: $ARCH"
    exit 1
fi

# 拼装 Target Triple
TARGET="${ARCH}-${OS}"
DIST_NAME="kern-${VERSION}-${TARGET}"
TARBALL="${DIST_NAME}.tar.gz"

DOWNLOAD_URL="https://github.com/softfault/kern/releases/download/${VERSION}/${TARBALL}"

KERN_HOME="${HOME}/.kern"
KERN_BIN="${KERN_HOME}/bin"

echo "=> Creating installation directory at ${KERN_HOME}..."
mkdir -p "${KERN_HOME}"

echo "=> Downloading Kern ${VERSION} for ${TARGET}..."
curl -L -# -o "/tmp/${TARBALL}" "${DOWNLOAD_URL}"

echo "=> Extracting toolchain..."
tar -xzf "/tmp/${TARBALL}" -C "/tmp"
# 把解压出来的 bin 和 lib 平移到 ~/.kern 目录
cp -r "/tmp/${DIST_NAME}/"* "${KERN_HOME}/"
rm -rf "/tmp/${TARBALL}" "/tmp/${DIST_NAME}"

echo "=> Configuring PATH..."
RC_FILE=""
if [ -n "$BASH_VERSION" ]; then
    RC_FILE="${HOME}/.bashrc"
elif [ -n "$ZSH_VERSION" ]; then
    RC_FILE="${HOME}/.zshrc"
else
    RC_FILE="${HOME}/.profile"
fi

if ! grep -q "${KERN_BIN}" "${RC_FILE}"; then
    echo "" >> "${RC_FILE}"
    echo "# Kern Programming Language" >> "${RC_FILE}"
    echo "export PATH=\"${KERN_BIN}:\$PATH\"" >> "${RC_FILE}"
    echo "Added ${KERN_BIN} to your PATH in ${RC_FILE}."
    echo "Please run 'source ${RC_FILE}' or restart your terminal to apply changes."
else
    echo "${KERN_BIN} is already in your PATH."
fi

echo ""
echo "Kern ${VERSION} toolchain installed successfully!"
echo "Run 'kernc --version', 'craft --version', and 'kern-lsp --version' (or launch it via your editor) to verify."
