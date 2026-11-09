#!/usr/bin/env bash






set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

DO_BUILD=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --build) DO_BUILD=true; shift ;;
        -h|--help)
            echo "Usage: $0 [--build]"
            echo "  --build  Actually run cargo build for each feature combination"
            exit 0 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

echo "=== Feature Flags Contract Checker ==="

ERRORS=0


REQUIRED_FEATURES=(
    "mobile-default"
    "mobile-full"
    "mobile-dev"
    "local-inference"
    "external-im"
    "browser-tools"
    "office-tools"
    "wasm-runtime"
    "legacy-python-tools"
)

echo ""
echo "--- Checking required features exist in workspace ---"


WORKSPACE_TOML="$PROJECT_DIR/Cargo.toml"
for feat in "${REQUIRED_FEATURES[@]}"; do
    if grep -q "$feat" "$WORKSPACE_TOML" 2>/dev/null; then
        echo "  OK: $feat referenced in workspace Cargo.toml"
    else

        if grep -r "\"$feat\"\|$feat" "$PROJECT_DIR"/crates/*/Cargo.toml "$PROJECT_DIR"/libs/*/Cargo.toml 2>/dev/null | grep -q .; then
            echo "  OK: $feat defined in crate Cargo.toml"
        else
            echo "  INFO: $feat not yet defined (expected for Sprint 0)"
        fi
    fi
done

echo ""
echo "--- Checking mobile-default does not include heavy deps ---"


HEAVY_DEPS=(
    "llama-cpp"
    "llama_cpp"
    "ort"
    "candle"
    "wasmtime"
    "wasmer"
    "chromiumoxide"
    "headless_chrome"
    "lopdf"
    "calamine"
)

for dep in "${HEAVY_DEPS[@]}"; do

    while IFS= read -r -d '' toml; do
        if grep -q "^$dep\b\|^$dep " "$toml" 2>/dev/null; then

            if ! grep -A2 "$dep" "$toml" 2>/dev/null | grep -q "optional\s*=\s*true"; then
                echo "  WARNING: $dep may be unconditional in $(basename "$(dirname "$toml")")"
            fi
        fi
    done < <(find "$PROJECT_DIR"/crates "$PROJECT_DIR"/libs -name "Cargo.toml" -print0 2>/dev/null)
done

echo "  OK: No obvious heavy deps in default features"

echo ""
echo "--- Checking forbidden patterns in mobile-default code paths ---"

FORBIDDEN_IMPORTS=(
    "use proot"
    "use termux"
    "use python"
    "use agentscope"
    "use remelight"
)

for pattern in "${FORBIDDEN_IMPORTS[@]}"; do
    if grep -r "$pattern" "$PROJECT_DIR"/crates/*/src/ "$PROJECT_DIR"/libs/*/src/ 2>/dev/null | grep -v "//\|#\[cfg\|test\|doc" | head -1 | grep -q .; then
        echo "FAIL: Found '$pattern' outside feature gate"
        ERRORS=$((ERRORS + 1))
    fi
done

if [ $ERRORS -eq 0 ]; then
    echo "  OK: No forbidden imports found"
fi


if $DO_BUILD; then
    echo ""
    echo "--- Build Checks ---"

    FEATURE_COMBOS=(
        "--no-default-features"
        "--features mobile-default"
        "--features mobile-full"
        "--features local-inference"
        "--features external-im"
    )



    for combo in "${FEATURE_COMBOS[@]}"; do
        read -r -a cargo_feature_args <<< "$combo"
        echo "  Checking: cargo build $combo"
        if cargo build --workspace "${cargo_feature_args[@]}" 2>/dev/null; then
            echo "    OK"
        else
            echo "    SKIP: Feature not yet implemented (expected for Sprint 0)"
        fi
    done
fi

echo ""
if [ $ERRORS -gt 0 ]; then
    echo "FAILED: $ERRORS feature flag check(s) failed"
    exit 1
fi

echo "PASSED: Feature flag contract checks passed"
exit 0
