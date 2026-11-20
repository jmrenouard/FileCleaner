#!/usr/bin/env bash
# FileCleaner Pre-Delete Hook Example: Mock S3 Offsite Archival
#
# Environment variables provided by FileCleaner:
#   FILECLEANER_PATH      Full path of candidate file
#   FILECLEANER_NAME      Candidate basename
#   FILECLEANER_DIR       Parent directory path
#   FILECLEANER_SIZE      File size in bytes
#   FILECLEANER_MTIME_ISO Candidate modification time in RFC 3339 UTC
#   FILECLEANER_RULE      Rule name evaluating the candidate
#   FILECLEANER_ACTION    Pipeline summary (e.g., compress(Gzip)->move)
#   FILECLEANER_DRY_RUN   "true" or "false"
#
# Exit codes:
#   0  - Offsite mock upload verified; allow FileCleaner action pipeline to proceed.
#   42 - Candidate has an active .hold marker or veto flag; skip file cleanly.
#   1+ - Operational upload or checksum verification error.

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

TARGET_PATH="${FILECLEANER_PATH:?FILECLEANER_PATH must be set}"
TARGET_NAME="${FILECLEANER_NAME:-$(basename "$TARGET_PATH")}"
RULE_NAME="${FILECLEANER_RULE:-unknown}"
DRY_RUN="${FILECLEANER_DRY_RUN:-false}"
MOCK_BUCKET_DIR="${FILECLEANER_S3_MOCK_DIR:-}"

# Support graceful per-file veto via companion .hold marker or explicit env toggle.
if [[ "${FILECLEANER_MOCK_VETO:-0}" == "1" ]] || [[ -e "${TARGET_PATH}.hold" ]]; then
    echo "hook veto: holding '${TARGET_PATH}' for rule '${RULE_NAME}'" >&2
    exit 42
fi

if [[ ! -f "$TARGET_PATH" ]]; then
    echo "hook error: candidate '${TARGET_PATH}' does not exist" >&2
    exit 1
fi

if [[ "$DRY_RUN" == "true" ]]; then
    echo "dry-run: would archive '${TARGET_NAME}' (${FILECLEANER_SIZE:-0} bytes) for rule '${RULE_NAME}'"
    exit 0
fi

if [[ -n "$MOCK_BUCKET_DIR" ]]; then
    DEST_DIR="${MOCK_BUCKET_DIR}/${RULE_NAME}"
    mkdir -p "$DEST_DIR"
    DEST_FILE="${DEST_DIR}/${TARGET_NAME}"
    TMP_FILE="${DEST_FILE}.s3-tmp.$$"

    cp -p "$TARGET_PATH" "$TMP_FILE"
    SRC_SUM="$(compute_sha256 "$TARGET_PATH")"
    DST_SUM="$(compute_sha256 "$TMP_FILE")"

    if [[ "$SRC_SUM" != "$DST_SUM" ]]; then
        rm -f "$TMP_FILE"
        echo "hook error: sha256 mismatch uploading '${TARGET_PATH}'" >&2
        exit 1
    fi

    mv -f "$TMP_FILE" "$DEST_FILE"
fi

exit 0
