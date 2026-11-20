#!/usr/bin/env bash
# FileCleaner Pre-Delete Hook Example: Verify Checksum Manifest Before Eviction
#
# Verifies that the candidate matches its expected byte size and companion `.sha256`
# manifest file before permitting archival or deletion.
#
# Exit codes:
#   0  - Checksum verified (or non-strict manifest absent); proceed with pipeline.
#   42 - Manifest missing in --strict mode or contains "HOLD"; veto file cleanly.
#   1  - Checksum or byte-size mismatch detected (data corruption guard).

set -euo pipefail

compute_sha256() {
    local file_path="$1"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$file_path" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$file_path" | awk '{print $1}'
    elif command -v openssl >/dev/null 2>&1; then
        openssl dgst -sha256 "$file_path" | awk '{print $NF}'
    else
        echo "hook error: no SHA-256 tool found (sha256sum, shasum, or openssl)" >&2
        return 1
    fi
}

STRICT_MODE=0
if [[ "${1:-}" == "--strict" ]]; then
    STRICT_MODE=1
    shift
fi

TARGET_PATH="${FILECLEANER_PATH:-${1:-}}"
EXPECTED_SIZE="${FILECLEANER_SIZE:-${2:-}}"

if [[ -z "$TARGET_PATH" ]] || [[ ! -f "$TARGET_PATH" ]]; then
    echo "hook error: missing or unreadable target '${TARGET_PATH}'" >&2
    exit 1
fi

# Reject unexpected size drift between FileCleaner scan metadata and current disk bytes.
if [[ -n "$EXPECTED_SIZE" ]]; then
    ACTUAL_SIZE="$(wc -c < "$TARGET_PATH" | tr -d '[:space:]')"
    if [[ "$ACTUAL_SIZE" != "$EXPECTED_SIZE" ]]; then
        echo "hook error: size drift on '${TARGET_PATH}' (expected ${EXPECTED_SIZE}, got ${ACTUAL_SIZE})" >&2
        exit 1
    fi
fi

MANIFEST="${TARGET_PATH}.sha256"
if [[ ! -f "$MANIFEST" ]]; then
    if [[ "$STRICT_MODE" -eq 1 ]]; then
        echo "hook veto: manifest '${MANIFEST}' not yet ready; deferring '${TARGET_PATH}'" >&2
        exit 42
    fi
    exit 0
fi

MANIFEST_TOKEN="$(awk '{print $1}' "$MANIFEST" | head -n 1)"
if [[ "$MANIFEST_TOKEN" == "HOLD" ]]; then
    echo "hook veto: manifest '${MANIFEST}' marks file HOLD" >&2
    exit 42
fi

ACTUAL_SUM="$(compute_sha256 "$TARGET_PATH")"
if [[ "$ACTUAL_SUM" != "$MANIFEST_TOKEN" ]]; then
    echo "hook error: SHA-256 mismatch on '${TARGET_PATH}' (expected ${MANIFEST_TOKEN}, got ${ACTUAL_SUM})" >&2
    exit 1
fi

exit 0
