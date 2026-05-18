#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
KERN_INSTALL_SH_SOURCE_ONLY=1 . "$ROOT/install.sh"

tmp="${TMPDIR:-/tmp}/kern-install-sh-test.$$"
cleanup() {
    rm -rf "$tmp"
}
trap cleanup EXIT INT TERM

mkdir -p "$tmp/sdk/manifest" "$tmp/sdk/toolchain/host/bin"
printf 'clang\n' >"$tmp/sdk/toolchain/host/bin/clang"
printf 'lld\n' >"$tmp/sdk/toolchain/host/bin/ld.lld"
python_bin="$(find_python3)"
clang_size="$("$python_bin" -c 'import os, sys; print(os.path.getsize(sys.argv[1]))' "$tmp/sdk/toolchain/host/bin/clang")"
lld_size="$("$python_bin" -c 'import os, sys; print(os.path.getsize(sys.argv[1]))' "$tmp/sdk/toolchain/host/bin/ld.lld")"
clang_sha="$("$python_bin" -c 'import hashlib, sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' "$tmp/sdk/toolchain/host/bin/clang")"
lld_sha="$("$python_bin" -c 'import hashlib, sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' "$tmp/sdk/toolchain/host/bin/ld.lld")"

cat >"$tmp/sdk/manifest/sdk.json" <<EOF
{
  "schema_version": 1,
  "host_target": "x86_64-linux-gnu",
  "toolchain": {
    "bundled": true,
    "components": {
      "clang": {
        "path": "toolchain/host/bin/clang",
        "kind": "file",
        "sha256": "$clang_sha",
        "size": $clang_size
      },
      "lld": {
        "path": "toolchain/host/bin/ld.lld",
        "kind": "file",
        "sha256": "$lld_sha",
        "size": $lld_size
      },
      "bin_dir": {
        "path": "toolchain/host/bin",
        "kind": "directory",
        "sha256": null,
        "size": null
      }
    }
  }
}
EOF

validate_manifest_components "$tmp/sdk" "x86_64-linux-gnu" "$tmp/sdk/manifest/sdk.json"

printf 'tampered\n' >"$tmp/sdk/toolchain/host/bin/clang"
if validate_manifest_components "$tmp/sdk" "x86_64-linux-gnu" "$tmp/sdk/manifest/sdk.json" >/dev/null 2>&1; then
    fail "manifest validation accepted a tampered component"
fi

cat >"$tmp/sdk/manifest/sdk.json" <<EOF
{
  "schema_version": 1,
  "host_target": "x86_64-linux-gnu",
  "toolchain": {
    "bundled": true,
    "components": {
      "clang": {
        "path": "toolchain/host/bin/clang",
        "kind": "file"
      }
    }
  }
}
EOF

if validate_manifest_components "$tmp/sdk" "x86_64-linux-gnu" "$tmp/sdk/manifest/sdk.json" >/dev/null 2>&1; then
    fail "manifest validation accepted missing lld component"
fi

printf '%s\n' "install.sh manifest validation tests passed"
