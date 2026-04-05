#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
PACKAGE_DIR="$ROOT_DIR/library/toml"
CRAFT_BIN="${CRAFT_BIN:-$ROOT_DIR/target/debug/craft}"
TOML_TEST_BIN="${TOML_TEST_BIN:-$HOME/go/bin/toml-test}"
PROFILE="${PROFILE:-dev}"
DECODER_PATH="$PACKAGE_DIR/.craft/build/$PROFILE/target/out/toml-0.1.0/bin/toml-test-decoder"

if [[ ! -x "$CRAFT_BIN" ]]; then
    echo "craft not found at: $CRAFT_BIN" >&2
    exit 1
fi

if [[ ! -x "$TOML_TEST_BIN" ]]; then
    echo "toml-test not found at: $TOML_TEST_BIN" >&2
    echo "Install it with:" >&2
    echo "  go install github.com/toml-lang/toml-test/v2/cmd/toml-test@latest" >&2
    exit 1
fi

"$CRAFT_BIN" build "$PACKAGE_DIR"

if [[ ! -x "$DECODER_PATH" ]]; then
    echo "decoder not found at: $DECODER_PATH" >&2
    exit 1
fi

exec "$TOML_TEST_BIN" test -decoder "$DECODER_PATH" "$@"
