#!/usr/bin/env bash


set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

THREAT_MODEL="$PROJECT_DIR/docs/security/threat-model.md"

echo "=== Threat Model Coverage Checker ==="

if [ ! -f "$THREAT_MODEL" ]; then
    echo "ERROR: Threat model not found at $THREAT_MODEL"
    echo "The version-controlled threat model is required for release checks."
    exit 1
fi


REQUIRED_THREATS=(
    "loopback API"
    "WebView"
    "token leakage"
    "MCP command"
    "skill upload"
    "archive traversal"
    "media download"
    "path traversal"
    "secret redaction"
    "approval replay"
    "cron.*tool"
    "IM spoofing"
    "local network"
    "Android.*exported"
    "Desktop Host.*origin"
)

ERRORS=0

echo ""
echo "--- Checking required threat coverage ---"

for threat in "${REQUIRED_THREATS[@]}"; do
    if grep -qi "$threat" "$THREAT_MODEL" 2>/dev/null; then
        echo "  OK: '$threat' covered"
    else
        echo "  FAIL: '$threat' not found in threat model"
        ERRORS=$((ERRORS + 1))
    fi
done

echo ""
echo "--- Checking threat entries have required fields ---"


REQUIRED_FIELDS=("mitigation" "owner" "test\|fixture" "release.*gate\|gate")

for field in "${REQUIRED_FIELDS[@]}"; do
    count=$(grep -ci "$field" "$THREAT_MODEL" 2>/dev/null || echo "0")
    if [ "$count" -gt 0 ]; then
        echo "  OK: '$field' appears $count time(s)"
    else
        echo "  FAIL: '$field' not found in threat model"
        ERRORS=$((ERRORS + 1))
    fi
done

echo ""
if [ $ERRORS -gt 0 ]; then
    echo "FAILED: $ERRORS threat model check(s) failed"
    exit 1
fi

echo "PASSED: Threat model coverage adequate"
exit 0
