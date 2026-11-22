#!/usr/bin/env bash
# FileCleaner Pre-Delete Hook Example: PostgreSQL PITR & Replica WAL Safety Gate
#
# Prevents premature archival/deletion of any WAL segment required by the oldest
# retained PostgreSQL base backup or an active streaming standby replica.
#
# Environment variables provided by FileCleaner:
#   FILECLEANER_PATH      Full path of candidate WAL segment
#   FILECLEANER_NAME      Candidate basename (expected 24 uppercase hex characters)
#   FILECLEANER_RULE      Rule name evaluating the candidate
#   FILECLEANER_DRY_RUN   "true" or "false"
#
# Optional operator boundary controls (at least one recommended in production):
#   FILECLEANER_PG_MIN_KEEP_WAL       24-hex WAL segment name required by retained base backups
#   FILECLEANER_PG_MIN_KEEP_WAL_FILE  File containing the 24-hex boundary WAL segment name
#   FILECLEANER_PG_REPLICA_WAL        24-hex WAL segment name currently replayed by lagging standby
#   FILECLEANER_PG_REPLICA_WAL_FILE   File containing the standby's 24-hex WAL segment name
#
# Exit codes:
#   0  - Candidate WAL segment strictly predates all protected boundaries; proceed.
#   42 - Benign veto: timeline/partial/hold file or segment >= protected boundary; skip cleanly.
#   1  - Operational error: missing candidate or unreadable/malformed boundary marker (fail-closed).

set -euo pipefail
export LC_ALL=C

is_valid_wal_name() {
    [[ "$1" =~ ^[0-9A-F]{24}$ ]]
}

read_wal_boundary() {
    local direct_val="$1"
    local file_path="$2"
    local label="$3"

    if [[ -n "$direct_val" ]]; then
        if ! is_valid_wal_name "$direct_val"; then
            echo "hook error: malformed $label boundary '$direct_val' (expected 24 uppercase hex chars)" >&2
            exit 1
        fi
        printf '%s' "$direct_val"
        return 0
    fi

    if [[ -n "$file_path" ]]; then
        if [[ ! -r "$file_path" ]]; then
            echo "hook error: unreadable $label boundary file '$file_path'" >&2
            exit 1
        fi
        local content
        content="$(head -n 1 "$file_path" | tr -d '[:space:]')"
        if ! is_valid_wal_name "$content"; then
            echo "hook error: malformed $label boundary in '$file_path': '$content'" >&2
            exit 1
        fi
        printf '%s' "$content"
        return 0
    fi

    printf ''
}

TARGET_PATH="${FILECLEANER_PATH:?FILECLEANER_PATH must be set}"
TARGET_NAME="${FILECLEANER_NAME:-$(basename "$TARGET_PATH")}"
RULE_NAME="${FILECLEANER_RULE:-pg-wal-archive}"

if [[ ! -f "$TARGET_PATH" ]]; then
    echo "hook error: candidate '$TARGET_PATH' does not exist" >&2
    exit 1
fi

# Never allow deletion of timeline history, partial segments, backup labels, or held files.
if [[ -e "${TARGET_PATH}.hold" ]] || [[ -e "${TARGET_PATH}.backup" ]]; then
    echo "hook veto: hold/backup marker present for '$TARGET_NAME' (rule '$RULE_NAME')" >&2
    exit 42
fi

if ! is_valid_wal_name "$TARGET_NAME"; then
    echo "hook veto: non-WAL artifact '$TARGET_NAME' protected (rule '$RULE_NAME')" >&2
    exit 42
fi

BASE_BOUNDARY="$(read_wal_boundary "${FILECLEANER_PG_MIN_KEEP_WAL:-}" "${FILECLEANER_PG_MIN_KEEP_WAL_FILE:-}" "base-backup")"
REPLICA_BOUNDARY="$(read_wal_boundary "${FILECLEANER_PG_REPLICA_WAL:-}" "${FILECLEANER_PG_REPLICA_WAL_FILE:-}" "replica")"

# On matching timelines, lexicographical comparison of 24-hex WAL names equals LSN order.
# Across timelines, any segment with timeline >= boundary timeline is conservatively vetoed.
check_boundary() {
    local boundary="$1"
    local label="$2"
    if [[ -z "$boundary" ]]; then
        return 0
    fi
    local cand_tl="${TARGET_NAME:0:8}"
    local bnd_tl="${boundary:0:8}"
    if [[ "$cand_tl" > "$bnd_tl" ]] || [[ ! "$TARGET_NAME" < "$boundary" ]]; then
        echo "hook veto: WAL '$TARGET_NAME' >= $label boundary '$boundary' (rule '$RULE_NAME')" >&2
        exit 42
    fi
}

check_boundary "$BASE_BOUNDARY" "base-backup"
check_boundary "$REPLICA_BOUNDARY" "replica"

exit 0
