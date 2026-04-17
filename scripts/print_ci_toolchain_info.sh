#!/bin/bash
set -euo pipefail

print_cmd_path() {
    local name="$1"
    if command -v "${name}" >/dev/null 2>&1; then
        printf '%s: %s\n' "${name}" "$(command -v "${name}")"
    else
        printf '%s: <missing>\n' "${name}"
    fi
}

print_first_line() {
    local label="$1"
    shift
    if "$@" >/tmp/kern-ci-toolchain-info.$$ 2>&1; then
        printf '%s: %s\n' "${label}" "$(head -n 1 /tmp/kern-ci-toolchain-info.$$)"
    else
        printf '%s: %s\n' "${label}" "$(head -n 1 /tmp/kern-ci-toolchain-info.$$ 2>/dev/null || printf '<failed>')"
    fi
    rm -f /tmp/kern-ci-toolchain-info.$$
}

echo "runner_os: $(uname -s)"
echo "runner_arch: $(uname -m)"
echo "LLVM_SYS_211_PREFIX: ${LLVM_SYS_211_PREFIX:-<unset>}"
echo "CC: ${CC:-<unset>}"
echo "CXX: ${CXX:-<unset>}"
if [[ -n "${LLVM_SYS_211_PREFIX:-}" ]]; then
    echo "prefix clang: ${LLVM_SYS_211_PREFIX}/bin/clang"
    echo "prefix clang++: ${LLVM_SYS_211_PREFIX}/bin/clang++"
    echo "prefix ld.lld: ${LLVM_SYS_211_PREFIX}/bin/ld.lld"
    echo "prefix ld64.lld: ${LLVM_SYS_211_PREFIX}/bin/ld64.lld"
fi

print_cmd_path cc
print_cmd_path clang
print_cmd_path clang++
print_cmd_path ld
print_cmd_path ld.lld
print_cmd_path llvm-config

if [[ -n "${CC:-}" && -x "${CC}" ]]; then
    print_first_line "CC --version" "${CC}" --version
fi
if [[ -n "${CXX:-}" && -x "${CXX}" ]]; then
    print_first_line "CXX --version" "${CXX}" --version
fi
if command -v cc >/dev/null 2>&1; then
    print_first_line "cc --version" cc --version
fi
if command -v clang >/dev/null 2>&1; then
    print_first_line "clang --version" clang --version
fi
if command -v clang++ >/dev/null 2>&1; then
    print_first_line "clang++ --version" clang++ --version
fi
if command -v ld >/dev/null 2>&1; then
    print_first_line "ld -v" ld -v
fi
if command -v ld.lld >/dev/null 2>&1; then
    print_first_line "ld.lld --version" ld.lld --version
fi
if command -v llvm-config >/dev/null 2>&1; then
    print_first_line "llvm-config --version" llvm-config --version
fi
if [[ -n "${LLVM_SYS_211_PREFIX:-}" && -x "${LLVM_SYS_211_PREFIX}/bin/ld.lld" ]]; then
    print_first_line "prefix ld.lld --version" "${LLVM_SYS_211_PREFIX}/bin/ld.lld" --version
fi
if [[ -n "${LLVM_SYS_211_PREFIX:-}" && -x "${LLVM_SYS_211_PREFIX}/bin/ld64.lld" ]]; then
    print_first_line "prefix ld64.lld --version" "${LLVM_SYS_211_PREFIX}/bin/ld64.lld" --version
fi

if [[ "$(uname -s)" == "Darwin" ]]; then
    print_first_line "xcrun --find ld" xcrun --find ld
    print_first_line "xcrun --show-sdk-path" xcrun --show-sdk-path
fi
