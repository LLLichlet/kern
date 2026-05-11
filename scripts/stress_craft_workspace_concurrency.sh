#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

PROJECT_INPUT="${1:-${ROOT}/examples}"
ROUNDS="${ROUNDS:-8}"
JOBS="${JOBS:-2}"
KEEP_SUCCESS="${KEEP_SUCCESS:-0}"

usage() {
    cat <<'EOF'
Usage:
  scripts/stress_craft_workspace_concurrency.sh [PROJECT_PATH]

Environment:
  ROUNDS=8         Number of concurrent rounds to run
  JOBS=2           Number of isolated workspace copies to test per round
  KEEP_SUCCESS=0   Keep successful temporary workspaces when set to 1

The script clones the selected project into isolated /tmp directories and
runs `cargo run -q -p craft -- test --project-path <copy>/Craft.toml`
concurrently. On failure it preserves the failing workspace copy and prints the
captured log path.
EOF
}

if [[ "${PROJECT_INPUT}" == "--help" || "${PROJECT_INPUT}" == "-h" ]]; then
    usage
    exit 0
fi

if [[ -f "${PROJECT_INPUT}" ]]; then
    SOURCE_ROOT="$(cd "$(dirname "${PROJECT_INPUT}")" && pwd)"
else
    SOURCE_ROOT="$(cd "${PROJECT_INPUT}" && pwd)"
fi

if [[ ! -f "${SOURCE_ROOT}/Craft.toml" ]]; then
    echo "expected Craft.toml under ${SOURCE_ROOT}" >&2
    exit 1
fi

if ! [[ "${ROUNDS}" =~ ^[0-9]+$ ]] || (( ROUNDS < 1 )); then
    echo "ROUNDS must be a positive integer" >&2
    exit 1
fi

if ! [[ "${JOBS}" =~ ^[0-9]+$ ]] || (( JOBS < 2 )); then
    echo "JOBS must be an integer >= 2" >&2
    exit 1
fi

prepare_copy() {
    local round="$1"
    local job="$2"
    local dir
    dir="$(mktemp -d "/tmp/craft-race-r${round}-j${job}-XXXXXX")"
    rsync -a --exclude .git --exclude .craft "${SOURCE_ROOT}/" "${dir}/"
    printf '%s\n' "${dir}"
}

cleanup_copy() {
    local dir="$1"
    if (( KEEP_SUCCESS == 0 )); then
        rm -rf "${dir}"
    fi
}

run_round() {
    local round="$1"
    local -a dirs=()
    local -a pids=()
    local -a logs=()
    local -a statuses=()
    local job

    for (( job = 1; job <= JOBS; job += 1 )); do
        local dir
        dir="$(prepare_copy "${round}" "${job}")"
        local log="${dir}/craft-test.log"
        dirs+=("${dir}")
        logs+=("${log}")
        (
            cargo run -q -p craft -- test --project-path "${dir}/Craft.toml" >"${log}" 2>&1
        ) &
        pids+=("$!")
    done

    local failed=0
    for (( job = 0; job < JOBS; job += 1 )); do
        if wait "${pids[${job}]}"; then
            statuses+=("0")
        else
            statuses+=("$?")
            failed=1
        fi
    done

    printf 'round=%s' "${round}"
    for (( job = 0; job < JOBS; job += 1 )); do
        printf ' job%s=%s' "$((job + 1))" "${statuses[${job}]}"
    done
    printf '\n'

    if (( failed != 0 )); then
        for (( job = 0; job < JOBS; job += 1 )); do
            if [[ "${statuses[${job}]}" != "0" ]]; then
                echo "failure workspace: ${dirs[${job}]}" >&2
                echo "failure log: ${logs[${job}]}" >&2
                sed -n '1,220p' "${logs[${job}]}" >&2
            fi
        done
        exit 1
    fi

    for dir in "${dirs[@]}"; do
        cleanup_copy "${dir}"
    done
}

echo "workspace=${SOURCE_ROOT}"
echo "rounds=${ROUNDS} jobs=${JOBS} keep_success=${KEEP_SUCCESS}"

for (( round = 1; round <= ROUNDS; round += 1 )); do
    run_round "${round}"
done

echo "craft workspace concurrency stress passed"
