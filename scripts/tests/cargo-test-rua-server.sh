#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../.."




if current_limit="$(ulimit -Sn 2>/dev/null)" \
    && [[ "$current_limit" =~ ^[0-9]+$ ]] \
    && (( current_limit < 4096 )); then
    if ! ulimit -Sn 4096 2>/dev/null; then
        echo "warning: could not raise open-file soft limit above $current_limit" >&2
    fi
fi

CARGO_OFFLINE=0
SUITE=all
while [[ $# -gt 0 ]]; do
    case "$1" in
        --offline) CARGO_OFFLINE=1; shift ;;
        --suite)
            [[ $# -ge 2 ]] || { echo "--suite requires a value" >&2; exit 2; }
            SUITE="$2"; shift 2 ;;
        *) echo "usage: $0 [--offline] [--suite all|contracts|product-hooks|terminal|endpoints|infra|sync]" >&2; exit 2 ;;
    esac
done
case "$SUITE" in
    all|contracts|product-hooks|terminal|endpoints|infra|sync) ;;
    *) echo "unknown server test suite: $SUITE" >&2; exit 2 ;;
esac

run_cargo_test() {
    if [[ $CARGO_OFFLINE -eq 1 ]]; then
        cargo test --offline "$@"
    else
        cargo test "$@"
    fi
}

TEST_STATUS=0
run_suite() {
    if ! run_cargo_test "$@"; then
        TEST_STATUS=1
    fi
}

CARGO_MODE=""
if [[ $CARGO_OFFLINE -eq 1 ]]; then
    CARGO_MODE=" --offline"
fi

TARGET_ARGS=(--lib --bins)
CONTRACT_SYNC_FOUND=0
ENDPOINTS_FOUND=0
INFRA_FOUND=0
while IFS= read -r target; do
    [[ -z "$target" ]] && continue
    case "$target" in
        contract_sync) CONTRACT_SYNC_FOUND=1 ;;
        endpoints) ENDPOINTS_FOUND=1 ;;
        infra) INFRA_FOUND=1 ;;
        contract_product_hooks_http|contract_terminal_http) ;;
        *) TARGET_ARGS+=(--test "$target") ;;
    esac
done < <(
    awk -F'"' '
        /^\[\[test\]\]$/ { in_test = 1; next }
        /^\[/ { in_test = 0 }
        in_test && /^name *= *"/ { print $2; in_test = 0 }
    ' crates/rua-server/Cargo.toml
)

if [[ $CONTRACT_SYNC_FOUND -ne 1 ]]; then
    echo "contract_sync test target is missing from crates/rua-server/Cargo.toml" >&2
    exit 2
fi
if [[ $ENDPOINTS_FOUND -ne 1 ]]; then
    echo "endpoints test target is missing from crates/rua-server/Cargo.toml" >&2
    exit 2
fi
if [[ $INFRA_FOUND -ne 1 ]]; then
    echo "infra test target is missing from crates/rua-server/Cargo.toml" >&2
    exit 2
fi

if [[ "$SUITE" == all || "$SUITE" == contracts ]]; then
    echo "==> cargo test$CARGO_MODE -p rua-server ${TARGET_ARGS[*]}"
    run_suite -p rua-server "${TARGET_ARGS[@]}"
fi

for entry in 'product-hooks:contract_product_hooks_http' 'terminal:contract_terminal_http'; do
    if [[ "$SUITE" == all || "$SUITE" == "${entry%%:*}" ]]; then
        echo "==> cargo test$CARGO_MODE -p rua-server --test ${entry#*:}"
        run_suite -p rua-server --test "${entry#*:}"
    fi
done

for target in endpoints infra; do
    if [[ "$SUITE" == all || "$SUITE" == "$target" ]]; then
        echo "==> cargo test$CARGO_MODE -p rua-server --test $target -- --test-threads=1"
        run_suite -p rua-server --test "$target" -- --test-threads=1
    fi
done

if [[ "$SUITE" == all || "$SUITE" == sync ]]; then
    ISOLATED_SYNC_TEST="platform_lifecycle_http::mobile_grace_closes_complete_device_network_and_foreground_rebinds_it"
    echo "==> cargo test$CARGO_MODE -p rua-server --test contract_sync $ISOLATED_SYNC_TEST -- --exact"
    run_suite -p rua-server --test contract_sync "$ISOLATED_SYNC_TEST" -- --exact
    echo "==> cargo test$CARGO_MODE -p rua-server --test contract_sync -- --skip $ISOLATED_SYNC_TEST --test-threads=1"
    run_suite -p rua-server --test contract_sync -- --skip "$ISOLATED_SYNC_TEST" --test-threads=1
fi

exit "$TEST_STATUS"
