#!/usr/bin/env bash




set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

echo "=== Config Roundtrip Checker ==="


ERRORS=0
for filter in roundtrip round_trips desktop_automation; do
    if ! cargo test -p rua-config --test store "$filter"; then
        ERRORS=$((ERRORS + 1))
    fi
done

FIXTURE_DIR="$PROJECT_DIR/fixtures/config"

echo ""
echo "--- Checking config fixtures ---"

for fixture in "$FIXTURE_DIR"/*.json; do
    if [ ! -f "$fixture" ]; then continue; fi
    filename=$(basename "$fixture")


    if ! python3 -c "import json; json.load(open('$fixture'))" 2>/dev/null; then
        echo "FAIL: $filename is not valid JSON"
        ERRORS=$((ERRORS + 1))
        continue
    fi

    echo "  OK: $filename"
done


UNKNOWN_FIELDS_FIXTURE="$FIXTURE_DIR/config-with-unknown-fields.json"
if [ -f "$UNKNOWN_FIELDS_FIXTURE" ]; then
    echo "  OK: Unknown fields fixture present"
else
    echo "  INFO: No unknown fields fixture at $UNKNOWN_FIELDS_FIXTURE (will be needed for Batch 1)"
fi


SECRET_FIXTURE="$FIXTURE_DIR/config-with-secrets.json"
if [ -f "$SECRET_FIXTURE" ]; then

    if python3 -c "
import json, sys
with open('$SECRET_FIXTURE') as f:
    data = json.load(f)
# Walk all string values looking for redacted markers
def check(obj, path=''):
    if isinstance(obj, dict):
        for k, v in obj.items():
            check(v, f'{path}.{k}')
    elif isinstance(obj, list):
        for i, v in enumerate(obj):
            check(v, f'{path}[{i}]')
    elif isinstance(obj, str):
        if obj in ('***', '****', '[REDACTED]') or (len(obj) > 2 and all(c == '*' for c in obj)):
            print(f'WARNING: {path} appears redacted: {obj}')
            sys.exit(1)
check(data)
" 2>/dev/null; then
        echo "  OK: Secret fixture contains real values (not redacted)"
    else
        echo "  FAIL: Secret fixture contains redacted values (***)"
        ERRORS=$((ERRORS + 1))
    fi
else
    echo "  INFO: No secret fixture at $SECRET_FIXTURE (will be needed for Batch 1)"
fi

echo ""
if [ $ERRORS -gt 0 ]; then
    echo "FAILED: $ERRORS roundtrip check(s) failed"
    exit 1
fi

echo "PASSED: All config roundtrip checks passed"
exit 0
