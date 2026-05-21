#!/bin/sh
set -eu

DEFAULT_GITHUB_REPO="kern-project/kern"
DEFAULT_VERSION="v0.8.1"

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
Bootstrap kernup and install a Kern SDK release archive.

Usage:
  install.sh [options]

Options:
  --version <tag>       Release tag; defaults to the latest GitHub release.
  --target <target>     Host target label; defaults to the current host target.
  --kernup <path>       Use a local kernup binary instead of downloading one.
  --archive <path>      Pass a local SDK archive to kernup install.
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
        *) fail "unsupported operating system for install.sh: $system" ;;
    esac
}

is_nixos_host() {
    [ -e /etc/NIXOS ] && return 0

    if [ -r /etc/os-release ] && grep -Eq '^ID=nixos$|^ID_LIKE=.*\bnixos\b' /etc/os-release; then
        return 0
    fi

    return 1
}

warn_if_nixos_host() {
    if ! is_nixos_host; then
        return 0
    fi

    info "=> Detected NixOS."
    info "=> If you manage your toolchain through Nix, prefer the flake/overlay flow documented in docs/nix.md."
    info "=> This installer will continue with the regular ~/.kern SDK installation."
}

fetch_latest_version() {
    github_repo="$1"
    latest_url="https://github.com/${github_repo}/releases/latest"
    resolved_url="$(
        curl -fsSLI -o /dev/null -w '%{url_effective}' "$latest_url" 2>/dev/null || true
    )"

    case "$resolved_url" in
        */releases/tag/*) printf '%s\n' "${resolved_url##*/}" ;;
        *) printf '%s\n' "" ;;
    esac
}

download_file() {
    url="$1"
    dest="$2"
    curl -fsSL "$url" -o "$dest" || fail "download failed for \`$url\`"
}

extract_single_root() {
    archive_path="$1"
    extract_root="$2"

    mkdir -p "$extract_root"
    tar -xf "$archive_path" -C "$extract_root"

    root=""
    count=0
    for path in "$extract_root"/*; do
        [ -d "$path" ] || continue
        root="$path"
        count=$((count + 1))
    done

    [ "$count" -eq 1 ] || fail "expected exactly one root in \`$archive_path\`"
    printf '%s\n' "$root"
}

main() {
VERSION=""
VERSION_SPECIFIED=0
TARGET=""
KERNUP=""
ARCHIVE=""
DEST=""
GITHUB_REPO="$DEFAULT_GITHUB_REPO"
NO_PATH=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version)
            [ "$#" -ge 2 ] || fail "--version requires a value"
            VERSION="$2"
            VERSION_SPECIFIED=1
            shift 2
            ;;
        --target)
            [ "$#" -ge 2 ] || fail "--target requires a value"
            TARGET="$2"
            shift 2
            ;;
        --kernup)
            [ "$#" -ge 2 ] || fail "--kernup requires a value"
            KERNUP="$2"
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
        *) fail "unknown argument: $1" ;;
    esac
done

HOST_TARGET="$(detect_host_target)"
if [ -z "$TARGET" ]; then
    TARGET="$HOST_TARGET"
fi
[ "$TARGET" = "$HOST_TARGET" ] || fail "target \`$TARGET\` does not match the current host \`$HOST_TARGET\`"
warn_if_nixos_host

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/kern-install.XXXXXX")"
cleanup() {
    rm -rf "$TEMP_ROOT"
}
trap cleanup EXIT INT TERM

if [ -n "$KERNUP" ]; then
    [ -f "$KERNUP" ] || fail "kernup binary \`$KERNUP\` does not exist"
    [ -x "$KERNUP" ] || chmod +x "$KERNUP" 2>/dev/null || true
    [ -x "$KERNUP" ] || fail "kernup binary \`$KERNUP\` is not executable"
    KERNUP_BIN="$KERNUP"
else
    need_tool curl
    need_tool tar

    if [ -z "$VERSION" ]; then
        VERSION="$(fetch_latest_version "$GITHUB_REPO")"
    fi
    if [ -z "$VERSION" ]; then
        VERSION="$DEFAULT_VERSION"
    fi

    KERNUP_ARCHIVE="kernup-${VERSION}-${TARGET}.tar.gz"
    KERNUP_ARCHIVE_PATH="$TEMP_ROOT/$KERNUP_ARCHIVE"
    KERNUP_URL="https://github.com/${GITHUB_REPO}/releases/download/${VERSION}/${KERNUP_ARCHIVE}"

    info "=> Downloading Kern installer ${VERSION}..."
    download_file "$KERNUP_URL" "$KERNUP_ARCHIVE_PATH"

    KERNUP_ROOT="$(extract_single_root "$KERNUP_ARCHIVE_PATH" "$TEMP_ROOT/kernup")"
    KERNUP_BIN="$KERNUP_ROOT/kernup"
    [ -x "$KERNUP_BIN" ] || chmod +x "$KERNUP_BIN" 2>/dev/null || true
    [ -f "$KERNUP_BIN" ] || fail "kernup binary is missing from \`$KERNUP_ARCHIVE\`"
fi

set -- install --target "$TARGET" --github-repo "$GITHUB_REPO"
if [ "$VERSION_SPECIFIED" -eq 1 ] || [ -z "$ARCHIVE" ]; then
    if [ -n "$VERSION" ]; then
        set -- "$@" --version "$VERSION"
    fi
fi
if [ -n "$ARCHIVE" ]; then
    set -- "$@" --archive "$ARCHIVE"
fi
if [ -n "$DEST" ]; then
    set -- "$@" --dest "$DEST"
fi
if [ "$NO_PATH" -eq 1 ]; then
    set -- "$@" --no-path
fi

exec "$KERNUP_BIN" "$@"
}

if [ "${KERN_INSTALL_SH_SOURCE_ONLY:-0}" -ne 1 ]; then
    main "$@"
fi
