#!/bin/bash
set -euo pipefail

DEFAULT_VERSION="v0.7.0"
TMP_ROOT=""

info() {
    echo "$@"
}

warn() {
    echo "Warning: $*" >&2
}

fail() {
    echo "Error: $*" >&2
    exit 1
}

cleanup() {
    if [ -n "${TMP_ROOT}" ] && [ -d "${TMP_ROOT}" ]; then
        rm -rf "${TMP_ROOT}"
    fi
}

require_tool() {
    command -v "$1" >/dev/null 2>&1 || fail "Required tool \`$1\` was not found in PATH."
}

fetch_latest_version() {
    curl -fsSL "https://api.github.com/repos/softfault/kern/releases/latest" \
        | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' \
        | head -n 1
}

detect_unix_target() {
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
            fail "Unsupported OS: ${os_name}"
            ;;
    esac

    arch=$(uname -m)
    case "${arch}" in
        aarch64|arm64)
            arch="aarch64"
            ;;
        x86_64|amd64)
            arch="x86_64"
            ;;
        *)
            fail "Unsupported architecture: ${arch}"
            ;;
    esac

    echo "${arch}-${os}"
}

select_rc_file() {
    local shell_name

    shell_name=$(basename "${SHELL:-}")
    case "${shell_name}" in
        zsh)
            echo "${HOME}/.zshrc"
            ;;
        bash)
            echo "${HOME}/.bashrc"
            ;;
        *)
            echo "${HOME}/.profile"
            ;;
    esac
}

report_linux_failure() {
    local binary_path="$1"

    if command -v ldd >/dev/null 2>&1; then
        local ldd_output
        ldd_output=$(ldd "${binary_path}" 2>&1 || true)
        if printf '%s\n' "${ldd_output}" | grep -Fq "not found"; then
            warn "Shared-library resolution failed for ${binary_path}:"
            printf '%s\n' "${ldd_output}" | grep -F "not found" >&2 || true
            warn "Install the missing runtime libraries for your distro, then rerun '${binary_path} --version'."
            warn "Common package names include libstdc++, zlib, and zstd."
            return
        fi
    fi

    warn "The binary still failed after the dynamic loader resolved its libraries."
    warn "This commonly means the release archive was built against a newer glibc baseline than this machine provides."
    warn "If this host is on an older distro, build Kern from source on the target machine."
}

report_darwin_failure() {
    local binary_path="$1"

    warn "macOS could not start ${binary_path}."
    if command -v otool >/dev/null 2>&1; then
        warn "Inspect linked host libraries with: otool -L ${binary_path}"
    fi
    warn "If the binary is blocked by local security policy, rerun it manually once to inspect the macOS error dialog/output."
}

verify_binary() {
    local target="$1"
    local binary_name="$2"
    local binary_path="${KERN_BIN}/${binary_name}"
    local output

    [ -x "${binary_path}" ] || fail "Installed binary ${binary_path} is missing or not executable."

    if output=$("${binary_path}" --version 2>&1); then
        info "=> Verified ${binary_name}: ${output}"
        return 0
    fi

    warn "Failed to start ${binary_name} after installation."
    printf '%s\n' "${output}" >&2

    case "${target}" in
        *linux-gnu)
            report_linux_failure "${binary_path}"
            ;;
        *apple-darwin)
            report_darwin_failure "${binary_path}"
            ;;
    esac

    warn "Installed files remain in ${KERN_HOME}, but the toolchain is not ready to use yet."
    exit 1
}

configure_path() {
    local rc_file

    rc_file=$(select_rc_file)
    touch "${rc_file}"

    info "=> Configuring PATH..."
    if ! grep -Fqs "${KERN_BIN}" "${rc_file}"; then
        {
            echo ""
            echo "# Kern Programming Language"
            echo "export PATH=\"${KERN_BIN}:\$PATH\""
        } >> "${rc_file}"
        info "Added ${KERN_BIN} to your PATH in ${rc_file}."
        info "Please run 'source ${rc_file}' or restart your terminal to apply changes."
    else
        info "${KERN_BIN} is already in your PATH."
    fi
}

main() {
    local latest_version version target dist_name tarball download_url archive_path extract_root

    require_tool curl
    require_tool tar
    require_tool uname

    info "Welcome to the Kern Programming Language Installer!"
    info "=> Fetching latest version info from GitHub..."

    latest_version="$(fetch_latest_version || true)"
    if [ -z "${latest_version}" ]; then
        warn "Failed to fetch the latest version from GitHub."
        warn "Falling back to ${DEFAULT_VERSION}."
        version="${DEFAULT_VERSION}"
    else
        version="${latest_version}"
    fi

    target="$(detect_unix_target)"
    dist_name="kern-${version}-${target}"
    tarball="${dist_name}.tar.gz"
    download_url="https://github.com/softfault/kern/releases/download/${version}/${tarball}"

    KERN_HOME="${HOME}/.kern"
    KERN_BIN="${KERN_HOME}/bin"
    export KERN_HOME KERN_BIN

    info "=> Preparing to install Kern ${version} toolchain for ${target}..."
    info "=> Creating installation directory at ${KERN_HOME}..."
    mkdir -p "${KERN_HOME}"

    TMP_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/kern-install-XXXXXX")
    archive_path="${TMP_ROOT}/${tarball}"
    extract_root="${TMP_ROOT}/extract"
    mkdir -p "${extract_root}"

    info "=> Downloading Kern ${version}..."
    curl -fL -# -o "${archive_path}" "${download_url}" \
        || fail "Download failed. Verify the release exists for ${target} and try again."

    info "=> Extracting toolchain..."
    tar -xzf "${archive_path}" -C "${extract_root}"
    cp -R "${extract_root}/${dist_name}/." "${KERN_HOME}/"

    info "=> Verifying installed tools..."
    verify_binary "${target}" "kernc"
    verify_binary "${target}" "craft"
    verify_binary "${target}" "kern-lsp"

    configure_path

    echo ""
    info "Kern ${version} toolchain installed successfully!"
    info "Run 'kernc --version', 'craft --version', and 'kern-lsp --version' to verify your shell PATH."
}

trap cleanup EXIT
main "$@"
