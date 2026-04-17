#!/bin/bash
set -euo pipefail

# CI-only wrapper for ThinLTO coverage. This keeps the test/build environment
# pinned to the same LLVM toolchain used for codegen without imposing that
# toolchain on end-user installs.

if [[ -z "${LLVM_SYS_211_PREFIX:-}" ]]; then
    echo "LLVM_SYS_211_PREFIX is not set" >&2
    exit 1
fi

LLVM_CLANGXX="${LLVM_SYS_211_PREFIX}/bin/clang++"
if [[ ! -x "${LLVM_CLANGXX}" ]]; then
    echo "missing clang++ at ${LLVM_CLANGXX}" >&2
    exit 1
fi

linker_flag=()
compile_only=false
for arg in "$@"; do
    case "${arg}" in
        -c|-E|-S|-M|-MM)
            compile_only=true
            ;;
    esac
done

if [[ "${compile_only}" == false ]]; then
    if [[ "$(uname -s)" == "Darwin" && -x "${LLVM_SYS_211_PREFIX}/bin/ld64.lld" ]]; then
        linker_flag=("-fuse-ld=${LLVM_SYS_211_PREFIX}/bin/ld64.lld")
    elif [[ -x "${LLVM_SYS_211_PREFIX}/bin/ld.lld" ]]; then
        linker_flag=("-fuse-ld=${LLVM_SYS_211_PREFIX}/bin/ld.lld")
    fi
fi

exec "${LLVM_CLANGXX}" "${linker_flag[@]}" "$@"
