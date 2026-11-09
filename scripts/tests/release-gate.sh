#!/usr/bin/env bash






set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

REPORT_FILE=""
TOUCHED_REF="main"
ANDROID_APK=""
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

while [[ $# -gt 0 ]]; do
    case $1 in
        --report) REPORT_FILE="$2"; shift 2 ;;
        --ref) TOUCHED_REF="${2:?missing ref}"; shift 2 ;;
        --android-apk) ANDROID_APK="${2:?missing APK}"; shift 2 ;;
        -h|--help)
            echo "Usage: $0 [--ref REF] [--report FILE] [--android-apk APK]"
            echo ""
            echo "Runs Rust validation and product release contracts. On failure, writes a report"
            echo "to FILE (or stdout if --report is omitted)."
            echo "Rust checks use Git diffs when available, otherwise all workspace and Desktop crates."
            echo "Android script contracts always run; --android-apk additionally verifies a built APK (SDK required)."
            exit 0 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

TOTAL=0
PASSED=0
FAILED=0
RESULTS=""

run_gate() {
    local name="$1"
    shift
    TOTAL=$((TOTAL + 1))
    echo "--- [$name] ---"
    local output_file
    output_file=$(mktemp "${TMPDIR:-/tmp}/rua-release-gate.XXXXXX")
    set +e
    "$@" 2>&1 | tee "$output_file"
    local status=${PIPESTATUS[0]}
    set -e
    if [[ $status -eq 0 ]]; then
        PASSED=$((PASSED + 1))
        RESULTS="${RESULTS}\n  PASS: $name"
        echo "  PASS"
    else
        FAILED=$((FAILED + 1))
        local last_line
        last_line=$(tail -1 "$output_file")
        RESULTS="${RESULTS}\n  FAIL: $name"
        RESULTS="${RESULTS}\n        ${last_line}"
        echo "  FAIL"
    fi
    rm -f "$output_file"
    echo ""
}

echo "========================================="
echo "  Release Readiness Gate"
echo "  $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "========================================="
echo ""


# Match the repository's local verification contract; the touched gate checks
# standalone Desktop crates too. CI may run its independent workspace matrix.
run_gate "touched-clippy-and-tests" "$SCRIPT_DIR/clippy-touched.sh" "$TOUCHED_REF" --test
run_gate "cargo fmt" cargo fmt --all -- --check
run_gate "desktop fmt" cargo fmt --manifest-path console/src-tauri/Cargo.toml -- --check
run_gate "feature-flags" bash "$SCRIPT_DIR/check-feature-flags.sh"
run_gate "config-roundtrip" bash "$SCRIPT_DIR/check-config-roundtrip.sh"
run_gate "threat-model" bash "$SCRIPT_DIR/check-threat-model-coverage.sh"
run_gate "desktop-signing-contract" bash "$SCRIPT_DIR/../release/check-desktop-signing-config.sh"
run_gate "desktop-release-contract" python3 "$SCRIPT_DIR/check-desktop-release-contract.py"
run_gate "desktop-automation-staging" python3 "$SCRIPT_DIR/check-desktop-automation-staging.py"
run_gate "android-release-contract" python3 "$SCRIPT_DIR/check-android-release-contract.py"
if [[ -n "$ANDROID_APK" ]]; then
    run_gate "android-apk-payload" python3 "$SCRIPT_DIR/../android/report-native-apk.py" \
        --apk "$ANDROID_APK" --out "$REPO_ROOT/target/android-release-gate-payload.json"
fi

echo "========================================="
echo "  Results: $PASSED/$TOTAL passed"
echo "========================================="

REPORT="Release Gate Report — $(date -u +%Y-%m-%dT%H:%M:%SZ)
Total: $TOTAL  Passed: $PASSED  Failed: $FAILED
$(echo -e "$RESULTS")"

if [ $FAILED -gt 0 ]; then
    echo ""
    echo "FAILED: $FAILED gate(s) did not pass."
    echo ""
    echo -e "$RESULTS"

    if [ -n "$REPORT_FILE" ]; then
        mkdir -p "$(dirname "$REPORT_FILE")"
        echo "$REPORT" > "$REPORT_FILE"
        echo ""
        echo "Report written to: $REPORT_FILE"
    fi
    exit 1
fi

echo ""
echo "AUTOMATED GATES PASSED — device E2E and signed platform acceptance are recorded separately."

if [ -n "$REPORT_FILE" ]; then
    mkdir -p "$(dirname "$REPORT_FILE")"
    echo "$REPORT" > "$REPORT_FILE"
    echo "Report written to: $REPORT_FILE"
fi

exit 0
