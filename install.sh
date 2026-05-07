#!/bin/sh
set -eu

DEFAULT_GITHUB_REPO="softfault/kern"
DEFAULT_VERSION="v0.7.5"

info() {
    printf '%s\n' "$1"
}

fail() {
    printf 'Error: %s\n' "$1" >&2
    exit 1
}

need_tool() {
    command -v "$1" >/dev/null 2>&1 || fail "required tool \`$1\` was not found in PATH."
}

print_help() {
    cat <<'EOF'
Install a Kern SDK release archive.

Usage:
  install.sh [options]

Options:
  --version <tag>       Release tag; defaults to the latest GitHub release.
  --target <target>     Host target label; defaults to the current host target.
  --archive <path>      Install from a local SDK archive instead of GitHub.
  --dest <path>         Installation directory; defaults to ~/.kern.
  --github-repo <repo>  GitHub repository for release downloads.
  --no-path             Do not mutate shell PATH configuration.
  --help                Show this help text.
EOF
}

detect_host_target() {
    system="$(uname -s)"
    machine="$(uname -m)"

    case "$machine" in
        x86_64|amd64) arch="x86_64" ;;
        arm64|aarch64) arch="aarch64" ;;
        *) fail "unsupported architecture: $machine" ;;
    esac

    case "$system" in
        Linux) printf '%s\n' "${arch}-linux-gnu" ;;
        Darwin) printf '%s\n' "${arch}-apple-darwin" ;;
        *)
            fail "unsupported operating system for install.sh: $system"
            ;;
    esac
}

infer_version_from_archive_name() {
    name="$1"
    target="$2"
    prefix="kern-"
    suffix="-$target.tar.gz"

    case "$name" in
        "$prefix"*"$suffix")
            version="${name#$prefix}"
            version="${version%$suffix}"
            printf '%s\n' "$version"
            return 0
            ;;
    esac

    return 1
}

fetch_latest_version() {
    github_repo="$1"
    latest_url="https://github.com/${github_repo}/releases/latest"
    resolved_url="$(
        curl -fsSLI -o /dev/null -w '%{url_effective}' "$latest_url" 2>/dev/null || true
    )"

    case "$resolved_url" in
        */releases/tag/*)
            printf '%s\n' "${resolved_url##*/}"
            ;;
        *)
            printf '%s\n' ""
            ;;
    esac
}

download_release_archive() {
    github_repo="$1"
    version="$2"
    archive_name="$3"
    dest="$4"
    url="https://github.com/${github_repo}/releases/download/${version}/${archive_name}"
    info "=> Downloading Kern ${version}..."
    curl -fsSL "$url" -o "$dest" || fail "download failed for \`$url\`"
}

extract_archive() {
    archive_path="$1"
    extract_root="$2"

    mkdir -p "$extract_root"
    tar -xf "$archive_path" -C "$extract_root"

    sdk_root=""
    sdk_count=0
    for path in "$extract_root"/*; do
        [ -d "$path" ] || continue
        sdk_root="$path"
        sdk_count=$((sdk_count + 1))
    done

    [ "$sdk_count" -eq 1 ] || fail "expected exactly one SDK root in \`$archive_path\`"
    printf '%s\n' "$sdk_root"
}

json_string_field() {
    field="$1"
    path="$2"
    sed -n "s/.*\"${field}\"[[:space:]]*:[[:space:]]*\"\\([^\"]*\\)\".*/\\1/p" "$path" | head -n 1
}

validate_sdk_root() {
    sdk_root="$1"
    expected_target="$2"
    manifest_path="$sdk_root/manifest/sdk.json"

    [ -f "$manifest_path" ] || fail "SDK manifest \`$manifest_path\` is missing"
    host_target="$(json_string_field "host_target" "$manifest_path")"
    [ "$host_target" = "$expected_target" ] || fail "SDK host target mismatch in \`$manifest_path\`"

    for binary in kernc craft kern-lsp; do
        [ -f "$sdk_root/bin/$binary" ] || fail "SDK binary \`$binary\` is missing from \`$sdk_root\`"
    done

    [ -f "$sdk_root/lib/kern/craft/init.rn" ] || fail "SDK craft script modules are missing"
    [ -d "$sdk_root/toolchain/host/bin" ] || fail "SDK toolchain layout is incomplete"

    verify_toolchain_component "$sdk_root/toolchain/host/bin/clang" "$expected_target"
    case "$expected_target" in
        *linux-gnu)
            verify_toolchain_component "$sdk_root/toolchain/host/bin/ld.lld" "$expected_target"
            ;;
        *apple-darwin)
            verify_toolchain_component "$sdk_root/toolchain/host/bin/ld64.lld" "$expected_target"
            ;;
    esac
}

verify_toolchain_component() {
    tool_path="$1"
    target="$2"
    [ -f "$tool_path" ] || fail "SDK bundled runtime tool \`$tool_path\` is missing"

    if output="$("$tool_path" --version 2>&1)"; then
        info "=> Verified $(basename "$tool_path"): $output"
        return 0
    fi

    case "$target" in
        *linux-gnu)
            output="$output
The bundled Linux runtime tool did not start. The SDK archive is missing a required shared-library dependency."
            ;;
        *apple-darwin)
            output="$output
The bundled macOS runtime tool did not start. The SDK archive likely has a broken dylib load command or missing bundled dylib."
            ;;
    esac

    fail "failed to start bundled runtime tool \`$tool_path\`:
$output"
}

copy_sdk_contents() {
    sdk_root="$1"
    install_root="$2"

    info "=> Installing SDK into $install_root..."
    install_parent="$(dirname "$install_root")"
    install_name="$(basename "$install_root")"
    staging_root="$install_parent/.${install_name}.installing.$$"
    backup_root="$install_parent/.${install_name}.previous.$$"

    mkdir -p "$install_parent"
    rm -rf "$staging_root" "$backup_root"
    mkdir -p "$staging_root"
    for child in "$sdk_root"/*; do
        if ! cp -R "$child" "$staging_root/"; then
            rm -rf "$staging_root"
            fail "failed to stage SDK contents for installation"
        fi
    done

    if [ -e "$install_root" ]; then
        if ! mv "$install_root" "$backup_root"; then
            rm -rf "$staging_root"
            fail "failed to move existing installation at \`$install_root\` aside"
        fi
    fi

    if ! mv "$staging_root" "$install_root"; then
        if [ -e "$backup_root" ]; then
            mv "$backup_root" "$install_root"
        fi
        fail "failed to replace existing installation at \`$install_root\`"
    fi

    rm -rf "$backup_root"
}

verify_binary() {
    binary_path="$1"
    target="$2"
    [ -f "$binary_path" ] || fail "installed binary \`$binary_path\` is missing"

    if output="$("$binary_path" --version 2>&1)"; then
        info "=> Verified $(basename "$binary_path"): $output"
        return 0
    fi

    case "$target" in
        *linux-gnu)
            output="$output
The host tool still failed after installation. This often means missing shared libraries or an older glibc baseline."
            ;;
        *apple-darwin)
            output="$output
macOS could not start the installed tool. Inspect local loader and security-policy behavior manually if needed."
            ;;
    esac

    fail "failed to start \`$binary_path\` after installation:
$output"
}

select_unix_rc_file() {
    shell_name="$(basename "${SHELL:-}")"
    home_dir="${HOME:?HOME is not set}"
    case "$shell_name" in
        zsh) printf '%s\n' "$home_dir/.zshrc" ;;
        bash) printf '%s\n' "$home_dir/.bashrc" ;;
        *) printf '%s\n' "$home_dir/.profile" ;;
    esac
}

configure_path() {
    install_bin="$1"
    rc_file="$(select_unix_rc_file)"
    touch "$rc_file"

    if grep -F "$install_bin" "$rc_file" >/dev/null 2>&1; then
        info "$install_bin is already in your PATH."
        return 0
    fi

    {
        printf '\n# Kern Programming Language\n'
        printf 'export PATH="%s:$PATH"\n' "$install_bin"
    } >>"$rc_file"
    info "Added $install_bin to your PATH in $rc_file."
}

VERSION=""
TARGET=""
ARCHIVE=""
DEST=""
GITHUB_REPO="$DEFAULT_GITHUB_REPO"
NO_PATH=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version)
            [ "$#" -ge 2 ] || fail "--version requires a value"
            VERSION="$2"
            shift 2
            ;;
        --target)
            [ "$#" -ge 2 ] || fail "--target requires a value"
            TARGET="$2"
            shift 2
            ;;
        --archive)
            [ "$#" -ge 2 ] || fail "--archive requires a value"
            ARCHIVE="$2"
            shift 2
            ;;
        --dest)
            [ "$#" -ge 2 ] || fail "--dest requires a value"
            DEST="$2"
            shift 2
            ;;
        --github-repo)
            [ "$#" -ge 2 ] || fail "--github-repo requires a value"
            GITHUB_REPO="$2"
            shift 2
            ;;
        --no-path)
            NO_PATH=1
            shift
            ;;
        --help|-h)
            print_help
            exit 0
            ;;
        *)
            fail "unknown argument: $1"
            ;;
    esac
done

need_tool curl
need_tool tar

HOST_TARGET="$(detect_host_target)"
if [ -z "$TARGET" ]; then
    TARGET="$HOST_TARGET"
fi
[ "$TARGET" = "$HOST_TARGET" ] || fail "target \`$TARGET\` does not match the current host \`$HOST_TARGET\`"

if [ -z "$DEST" ]; then
    DEST="${HOME:?HOME is not set}/.kern"
fi

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/kern-install.XXXXXX")"
cleanup() {
    rm -rf "$TEMP_ROOT"
}
trap cleanup EXIT INT TERM

ARCHIVE_PATH=""
if [ -n "$ARCHIVE" ]; then
    ARCHIVE_PATH="$ARCHIVE"
    [ -f "$ARCHIVE_PATH" ] || fail "archive \`$ARCHIVE_PATH\` does not exist"
    if [ -z "$VERSION" ]; then
        VERSION="$(infer_version_from_archive_name "$(basename "$ARCHIVE_PATH")" "$TARGET" || true)"
    fi
else
    if [ -z "$VERSION" ]; then
        VERSION="$(fetch_latest_version "$GITHUB_REPO")"
    fi
    if [ -z "$VERSION" ]; then
        VERSION="$DEFAULT_VERSION"
    fi
    ARCHIVE_NAME="kern-${VERSION}-${TARGET}.tar.gz"
    ARCHIVE_PATH="$TEMP_ROOT/$ARCHIVE_NAME"
    download_release_archive "$GITHUB_REPO" "$VERSION" "$ARCHIVE_NAME" "$ARCHIVE_PATH"
fi

[ -n "$VERSION" ] || fail "failed to resolve release version"

EXTRACT_ROOT="$TEMP_ROOT/extract"
info "=> Extracting toolchain..."
SDK_ROOT="$(extract_archive "$ARCHIVE_PATH" "$EXTRACT_ROOT")"
validate_sdk_root "$SDK_ROOT" "$TARGET"
copy_sdk_contents "$SDK_ROOT" "$DEST"

INSTALL_BIN="$DEST/bin"
info "=> Verifying installed tools..."
verify_binary "$INSTALL_BIN/kernc" "$TARGET"
verify_binary "$INSTALL_BIN/craft" "$TARGET"
verify_binary "$INSTALL_BIN/kern-lsp" "$TARGET"

if [ "$NO_PATH" -eq 0 ]; then
    info "=> Configuring PATH..."
    configure_path "$INSTALL_BIN"
fi

info "Kern ${VERSION} toolchain installed successfully!"
