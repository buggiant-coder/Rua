#!/usr/bin/env bash











set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
cd "$REPO_ROOT"

RUN_TEST=0
INCLUDE_DEPS=0
REF=""

for arg in "$@"; do
    case "$arg" in
        --test) RUN_TEST=1 ;;
        --deps) INCLUDE_DEPS=1 ;;
        -*) echo "unknown flag: $arg" >&2; exit 2 ;;
        *) REF="$arg" ;;
    esac
done

FULL_CHECK=0
GIT_ROOT=$(git rev-parse --show-toplevel 2>/dev/null || true)
if [[ -n "$GIT_ROOT" ]]; then
    GIT_ROOT=$(cd "$GIT_ROOT" && pwd -P)
fi
if [[ "$GIT_ROOT" != "$REPO_ROOT" ]]; then
    echo "Git checkout unavailable; running full Rust validation."
    FULL_CHECK=1
elif [[ -z "$REF" ]] && ! REF=$(git merge-base HEAD main 2>/dev/null || git rev-parse --verify HEAD~1 2>/dev/null); then
    echo "Git comparison baseline unavailable; running full Rust validation."
    FULL_CHECK=1
elif ! git rev-parse --verify "$REF^{commit}" >/dev/null 2>&1; then
    echo "Git comparison baseline '$REF' unavailable; running full Rust validation."
    FULL_CHECK=1
fi

if [[ $FULL_CHECK -eq 0 ]]; then
    if ! CHANGED_FILES=$(
        {
            git diff --name-only "$REF"...HEAD &&
            git diff --name-only --cached &&
            git diff --name-only &&
            git ls-files --others --exclude-standard '*.rs'
        } | sort -u
    ); then
        echo "Git diff unavailable; running full Rust validation."
        FULL_CHECK=1
    elif [[ -z "$CHANGED_FILES" ]]; then
        echo "no changed files vs $REF — nothing to clippy"
        exit 0
    fi
fi

if [[ $FULL_CHECK -eq 1 ]]; then
    CRATE_DIRS=$(
        cargo metadata --no-deps --format-version 1 --offline | python3 -c '
import json, os, sys
metadata = json.load(sys.stdin)
members = set(metadata["workspace_members"])
for package in metadata["packages"]:
    if package["id"] in members:
        print(os.path.relpath(os.path.dirname(package["manifest_path"])))
'
    )
    if [[ -f console/src-tauri/Cargo.toml ]]; then
        CRATE_DIRS=$(printf '%s\n%s\n' "$CRATE_DIRS" console/src-tauri | sort -u)
    fi
else
    CRATE_DIRS=$(
        while IFS= read -r f; do
            [[ -z "$f" ]] && continue
            if [[ "$f" != *.rs && "$f" != */Cargo.toml ]]; then
                continue
            fi
            [[ -e "$f" ]] || continue
            dir=$(dirname "$f")
            while [[ "$dir" != "." && "$dir" != "/" ]]; do
                if [[ -f "$dir/Cargo.toml" ]] && grep -q '^\[package\]' "$dir/Cargo.toml" 2>/dev/null; then
                    echo "$dir"
                    break
                fi
                dir=$(dirname "$dir")
            done
        done <<< "$CHANGED_FILES" | sort -u
    )
fi

if [[ -z "$CRATE_DIRS" ]]; then
    echo "changed files touch no crate (docs/scripts only) — nothing to clippy"
    exit 0
fi




STANDALONE_DIRS=$(
    while IFS= read -r dir; do
        [[ -z "$dir" ]] && continue
        if grep -q '^\[workspace\]' "$dir/Cargo.toml" 2>/dev/null; then
            echo "$dir"
        fi
    done <<< "$CRATE_DIRS"
)

WORKSPACE_CRATE_DIRS=$(
    while IFS= read -r dir; do
        [[ -z "$dir" ]] && continue
        if ! grep -q '^\[workspace\]' "$dir/Cargo.toml" 2>/dev/null; then
            echo "$dir"
        fi
    done <<< "$CRATE_DIRS"
)


CRATES=$(
    while IFS= read -r dir; do
        [[ -z "$dir" ]] && continue
        awk -F'"' '/^\[package\]/{p=1} p && /^name *=/{print $2; exit}' "$dir/Cargo.toml"
    done <<< "$WORKSPACE_CRATE_DIRS" | sort -u
)


if [[ $FULL_CHECK -eq 0 && $INCLUDE_DEPS -eq 1 && -n "$CRATES" ]]; then
    if ! command -v jq >/dev/null; then
        echo "--deps requires jq" >&2; exit 2
    fi
    META=$(cargo metadata --no-deps --format-version 1 \
        | jq -r '.packages[] | "\(.name)\t\(.dependencies[].name)"')

    while :; do
        new=$(
            while IFS=$'\t' read -r pkg dep; do
                if grep -qx "$dep" <<< "$CRATES" && ! grep -qx "$pkg" <<< "$CRATES"; then
                    echo "$pkg"
                fi
            done <<< "$META" | sort -u
        )
        [[ -z "$new" ]] && break
        CRATES=$(printf '%s\n%s\n' "$CRATES" "$new" | sort -u)
    done
fi

ARGS=()
while IFS= read -r c; do
    [[ -n "$c" ]] && ARGS+=(-p "$c")
done <<< "$CRATES"

CRATE_LIST=$(echo "$CRATES" | tr '\n' ' ')
if [[ -n "$CRATES" ]]; then
    echo "==> touched workspace crates: $CRATE_LIST"
    echo "==> cargo clippy ${ARGS[*]} --all-targets --offline -- -D warnings"
    cargo clippy "${ARGS[@]}" --all-targets --offline -- -D warnings
fi

while IFS= read -r dir; do
    [[ -z "$dir" ]] && continue
    echo "==> touched standalone crate: $dir"
    echo "==> cargo clippy --manifest-path $dir/Cargo.toml --all-targets --offline -- -D warnings"
    cargo clippy --manifest-path "$dir/Cargo.toml" --all-targets --offline -- -D warnings
done <<< "$STANDALONE_DIRS"

if [[ $RUN_TEST -eq 1 ]]; then
    if [[ -n "$CRATES" ]] && command -v cargo-nextest >/dev/null; then




        TEST_ARGS=()
        SERVER_TOUCHED=0
        while IFS= read -r c; do
            [[ -z "$c" ]] && continue
            if [[ "$c" == "rua-server" ]]; then
                SERVER_TOUCHED=1
            else
                TEST_ARGS+=(-p "$c")
            fi
        done <<< "$CRATES"

        status=0
        if [[ ${#TEST_ARGS[@]} -gt 0 ]]; then
            echo "==> cargo nextest run ${TEST_ARGS[*]} --offline --no-fail-fast"
            set +e
            cargo nextest run "${TEST_ARGS[@]}" --offline --no-fail-fast
            status=$?
            set -e
        fi
        if [[ $SERVER_TOUCHED -eq 1 ]]; then
            echo "==> scripts/tests/cargo-test-rua-server.sh --offline"
            set +e
            bash scripts/tests/cargo-test-rua-server.sh --offline
            server_status=$?
            set -e
            if [[ $server_status -ne 0 ]]; then
                status=$server_status
            fi
        fi
        if [[ $status -ne 0 ]]; then
            echo "==> touched test gate failed; rerun the failing command shown above"
            exit "$status"
        fi
    elif [[ -n "$CRATES" ]]; then
        echo "==> nextest not installed, falling back to cargo test"
        echo "    install: cargo install cargo-nextest --locked"
        cargo test "${ARGS[@]}" --offline
    fi

    while IFS= read -r dir; do
        [[ -z "$dir" ]] && continue
        echo "==> cargo test --manifest-path $dir/Cargo.toml --offline"
        cargo test --manifest-path "$dir/Cargo.toml" --offline
    done <<< "$STANDALONE_DIRS"
fi
