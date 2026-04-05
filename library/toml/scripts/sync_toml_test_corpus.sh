#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
CORPUS_ROOT="$ROOT_DIR/library/toml/tests/upstream/toml-test"
TOML_TEST_BIN="${TOML_TEST_BIN:-$HOME/go/bin/toml-test}"
TOML_VERSION="${TOML_VERSION:-latest}"

if [[ ! -x "$TOML_TEST_BIN" ]]; then
    echo "toml-test not found at: $TOML_TEST_BIN" >&2
    echo "Install it with:" >&2
    echo "  go install github.com/toml-lang/toml-test/v2/cmd/toml-test@latest" >&2
    exit 1
fi

TARGET_DIR="$CORPUS_ROOT/$TOML_VERSION"
rm -rf "$TARGET_DIR"
mkdir -p "$CORPUS_ROOT"

exec "$TOML_TEST_BIN" copy -toml "$TOML_VERSION" "$TARGET_DIR"
