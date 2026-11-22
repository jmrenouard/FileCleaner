#!/usr/bin/env bash
# FileCleaner v1.1 Live Docker E2E Verification Harness
#
# Exercises all 6 DBA PostgreSQL WAL/Backup + MinIO S3 archival scenarios against
# a real PostgreSQL 17 engine (compatible with `jmrenouard/multi-db-docker-env`)
# and an ephemeral pinned MinIO S3-compatible container.
#
# Usage:
#   bash scripts/e2e-dba-s3-lab.sh
#
# Optional environment overrides:
#   FC_E2E_BIN            Path to compiled `filecleaner` binary
#   FC_E2E_PG_IMAGE       PostgreSQL image (default: postgres:17-bookworm)
#   FC_E2E_MINIO_IMAGE    MinIO image (default: pgsty/minio:RELEASE.2026-08-04T00-00-00Z)
#   FC_E2E_TMP_ROOT       Host temp root shared with Docker (default: /var/tmp)

set -euo pipefail
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

FC_E2E_BIN="${FC_E2E_BIN:-${CARGO_TARGET_DIR:-/tmp/filecleaner-target}/debug/filecleaner}"
PG_IMAGE="${FC_E2E_PG_IMAGE:-postgres:17-bookworm}"
MINIO_IMAGE="${FC_E2E_MINIO_IMAGE:-pgsty/minio:RELEASE.2026-08-04T00-00-00Z}"
LAB_ID="fc-e2e-$$-$(date +%s)"
PG_CONTAINER="${LAB_ID}-pg17"
MINIO_CONTAINER="${LAB_ID}-minio"
LAB_NET="${LAB_ID}-net"
# Use /var/tmp by default (persistent rootfs shared across Docker Desktop WSL2 mount namespaces).
TMP_ROOT="${FC_E2E_TMP_ROOT:-/var/tmp}"
mkdir -p "$TMP_ROOT"
WORK_DIR="$(mktemp -d "${TMP_ROOT}/${LAB_ID}.XXXXXX")"

# Ephemeral lab-only credentials (never production secrets; discarded on cleanup).
export FC_E2E_S3_ACCESS_KEY="fce2e${LAB_ID//-/}"
export FC_E2E_S3_SECRET_KEY="fce2esecret$(date +%s)$$"
PG_USER="postgres"
PG_DB="labdb"
PG_PASS="pg_e2e_ephemeral_$$"
BUCKET="fc-e2e-pg-archive"

WRITER_PID=""
CORRUPT_MOCK_PID=""

cleanup() {
    local rc=$?
    if [[ -n "$WRITER_PID" ]]; then
        kill "$WRITER_PID" >/dev/null 2>&1 || true
        wait "$WRITER_PID" >/dev/null 2>&1 || true
    fi
    if [[ -n "$CORRUPT_MOCK_PID" ]]; then
        kill "$CORRUPT_MOCK_PID" >/dev/null 2>&1 || true
        wait "$CORRUPT_MOCK_PID" >/dev/null 2>&1 || true
    fi
    docker rm -f "$PG_CONTAINER" >/dev/null 2>&1 || true
    docker rm -f "$MINIO_CONTAINER" >/dev/null 2>&1 || true
    docker network rm "$LAB_NET" >/dev/null 2>&1 || true
    rm -rf "$WORK_DIR" >/dev/null 2>&1 || true
    exit "$rc"
}
trap cleanup EXIT INT TERM

log() {
    printf '[e2e-dba-s3] %s\n' "$*"
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || { echo "ERROR: required command '$1' not found" >&2; exit 1; }
}

require_cmd docker
require_cmd python3
require_cmd gzip

if [[ ! -x "$FC_E2E_BIN" ]]; then
    log "Building filecleaner binary in /tmp/filecleaner-target..."
    (cd "$REPO_ROOT" && CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/filecleaner-target}" cargo build --bin filecleaner)
    FC_E2E_BIN="${CARGO_TARGET_DIR:-/tmp/filecleaner-target}/debug/filecleaner"
fi

# Pick free loopback host ports for MinIO and fault-injection mock server.
read -r MINIO_PORT MOCK_PORT <<<"$(python3 - <<'PY'
import socket
ports = []
for _ in range(2):
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    ports.append(s.getsockname()[1])
    s.close()
print(ports[0], ports[1])
PY
)"

BASE_DIR="${WORK_DIR}/pg_base"
WAL_DIR="${WORK_DIR}/pg_wal"
AUDIT_LOG="${WORK_DIR}/audit.jsonl"
RESTORE_DIR="${WORK_DIR}/restore"
mkdir -p "$BASE_DIR" "$WAL_DIR" "$RESTORE_DIR"
chmod 777 "$WAL_DIR"

log "Starting isolated Docker network, PostgreSQL 17 (archive_mode=on), and MinIO (${MINIO_IMAGE})..."
docker network create "$LAB_NET" >/dev/null

docker run -d --name "$MINIO_CONTAINER" --network "$LAB_NET" \
    -p "${MINIO_PORT}:9000" \
    -e "MINIO_ROOT_USER=${FC_E2E_S3_ACCESS_KEY}" \
    -e "MINIO_ROOT_PASSWORD=${FC_E2E_S3_SECRET_KEY}" \
    "$MINIO_IMAGE" server /data >/dev/null

docker run -d --name "$PG_CONTAINER" --network "$LAB_NET" \
    -e "POSTGRES_USER=${PG_USER}" \
    -e "POSTGRES_PASSWORD=${PG_PASS}" \
    -e "POSTGRES_DB=${PG_DB}" \
    -v "${WAL_DIR}:/wal_archive" \
    "$PG_IMAGE" \
    postgres \
    -c wal_level=replica \
    -c archive_mode=on \
    -c "archive_command=cp \"%p\" \"/wal_archive/%f.tmp\" && chmod 0644 \"/wal_archive/%f.tmp\" && mv \"/wal_archive/%f.tmp\" \"/wal_archive/%f\"" \
    -c max_wal_senders=5 >/dev/null

# Wait for MinIO and PostgreSQL health (fail-fast if either does not come up).
MINIO_READY=0
for _ in $(seq 1 40); do
    if python3 -c "import urllib.request; urllib.request.urlopen('http://127.0.0.1:${MINIO_PORT}/minio/health/live', timeout=2)" >/dev/null 2>&1; then
        MINIO_READY=1
        break
    fi
    sleep 1
done
[[ "$MINIO_READY" -eq 1 ]] || { echo "ERROR: MinIO container did not become healthy on port ${MINIO_PORT}" >&2; docker logs "$MINIO_CONTAINER" >&2 || true; exit 1; }

# Probe TCP (127.0.0.1:5432) rather than the Unix socket so we wait for the final
# post-initdb PostgreSQL process (initdb runs with listen_addresses='' and restarts).
PG_READY=0
for _ in $(seq 1 40); do
    if docker exec -e "PGPASSWORD=${PG_PASS}" "$PG_CONTAINER" pg_isready -h 127.0.0.1 -p 5432 -U "$PG_USER" -d "$PG_DB" >/dev/null 2>&1 \
        && [[ "$(docker exec -e "PGPASSWORD=${PG_PASS}" "$PG_CONTAINER" psql -h 127.0.0.1 -U "$PG_USER" -d "$PG_DB" -tAc "SELECT 1" 2>/dev/null || true)" == "1" ]]; then
        PG_READY=1
        break
    fi
    sleep 1
done
[[ "$PG_READY" -eq 1 ]] || { echo "ERROR: PostgreSQL container '$PG_CONTAINER' did not become ready" >&2; docker logs "$PG_CONTAINER" >&2 || true; exit 1; }

# Pure-Python stdlib SigV4 helper for MinIO bucket creation and Scenario 6 object download/verification.
S3_HELPER="${WORK_DIR}/s3_helper.py"
cat <<'PY' > "$S3_HELPER"
import datetime, hashlib, hmac, os, sys, urllib.request

def sign_and_request(method, endpoint, bucket, key="", extra_headers=None, out_file=None):
    ak = os.environ["FC_E2E_S3_ACCESS_KEY"]
    sk = os.environ["FC_E2E_S3_SECRET_KEY"]
    region = "us-east-1"
    host = endpoint.replace("http://", "").replace("https://", "").rstrip("/")
    canonical_uri = f"/{bucket}" + (f"/{key.lstrip('/')}" if key else "")
    now = datetime.datetime.now(datetime.timezone.utc)
    amz_date = now.strftime("%Y%m%dT%H%M%SZ")
    date_stamp = now.strftime("%Y%m%d")
    empty_hash = hashlib.sha256(b"").hexdigest()
    hdrs = {"host": host, "x-amz-content-sha256": empty_hash, "x-amz-date": amz_date}
    if extra_headers:
        for k, v in extra_headers.items():
            hdrs[k.lower()] = v
    sorted_keys = sorted(hdrs.keys())
    canon_hdrs = "".join(f"{k}:{hdrs[k]}\n" for k in sorted_keys)
    signed_hdrs = ";".join(sorted_keys)
    canon_req = f"{method}\n{canonical_uri}\n\n{canon_hdrs}\n{signed_hdrs}\n{empty_hash}"
    scope = f"{date_stamp}/{region}/s3/aws4_request"
    sts = f"AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{hashlib.sha256(canon_req.encode()).hexdigest()}"
    def h(k, m): return hmac.new(k, m.encode(), hashlib.sha256).digest()
    k_sign = h(h(h(h(("AWS4" + sk).encode(), date_stamp), region), "s3"), "aws4_request")
    sig = hmac.new(k_sign, sts.encode(), hashlib.sha256).hexdigest()
    req = urllib.request.Request(f"{endpoint.rstrip('/')}{canonical_uri}", method=method)
    for k in sorted_keys:
        if k != "host":
            req.add_header(k, hdrs[k])
    req.add_header("Authorization", f"AWS4-HMAC-SHA256 Credential={ak}/{scope}, SignedHeaders={signed_hdrs}, Signature={sig}")
    with urllib.request.urlopen(req, timeout=15) as resp:
        data = resp.read()
        if out_file:
            with open(out_file, "wb") as f:
                f.write(data)
        return resp.status, dict(resp.headers), len(data)

cmd = sys.argv[1]
if cmd == "mkbucket":
    st, _, _ = sign_and_request("PUT", sys.argv[2], sys.argv[3])
    assert st in (200, 204), f"mkbucket status={st}"
elif cmd == "head":
    st, hdrs, _ = sign_and_request("HEAD", sys.argv[2], sys.argv[3], sys.argv[4], {"x-amz-checksum-mode": "ENABLED"})
    ck = hdrs.get("x-amz-checksum-sha256") or hdrs.get("X-Amz-Checksum-Sha256")
    assert st == 200 and ck, f"HEAD failed status={st} hdrs={hdrs}"
    print(ck)
elif cmd == "get":
    st, _, n = sign_and_request("GET", sys.argv[2], sys.argv[3], sys.argv[4], out_file=sys.argv[5])
    assert st == 200 and n > 0, f"GET failed status={st} len={n}"
PY

MINIO_ENDPOINT="http://127.0.0.1:${MINIO_PORT}"
python3 "$S3_HELPER" mkbucket "$MINIO_ENDPOINT" "$BUCKET"

# ---------------------------------------------------------------------------
# Scenario 1: Real PostgreSQL WAL & pg_dump Generation
# ---------------------------------------------------------------------------
log "Scenario 1: Generating real 16 MiB PostgreSQL WAL segments and pg_dump base backups..."
docker exec "$PG_CONTAINER" psql -U "$PG_USER" -d "$PG_DB" -v ON_ERROR_STOP=1 -c "
CREATE TABLE fc_e2e_ledger (id SERIAL PRIMARY KEY, label TEXT NOT NULL, amount NUMERIC NOT NULL);
INSERT INTO fc_e2e_ledger (label, amount)
SELECT 'tx-' || g, (g * 10.50)::NUMERIC FROM generate_series(1, 250) AS g;
SELECT pg_switch_wal();
INSERT INTO fc_e2e_ledger (label, amount) VALUES ('after-switch-1', 99.95);
SELECT pg_switch_wal();
INSERT INTO fc_e2e_ledger (label, amount) VALUES ('after-switch-2', 199.95);
SELECT pg_switch_wal();
INSERT INTO fc_e2e_ledger (label, amount) VALUES ('after-switch-3', 299.95);
SELECT pg_switch_wal();
" >/dev/null

for _ in $(seq 1 20); do
    WAL_COUNT="$(find "$WAL_DIR" -maxdepth 1 -type f -regextype posix-extended -regex '.*/[0-9A-F]{24}$' | wc -l)"
    if [[ "$WAL_COUNT" -ge 4 ]]; then
        break
    fi
    sleep 1
done
WAL_COUNT="$(find "$WAL_DIR" -maxdepth 1 -type f -regextype posix-extended -regex '.*/[0-9A-F]{24}$' | wc -l)"
[[ "$WAL_COUNT" -ge 4 ]] || { echo "ERROR: expected >= 4 real WAL segments, found $WAL_COUNT" >&2; docker logs --tail 30 "$PG_CONTAINER" >&2 || true; exit 1; }

# Add protected PostgreSQL timeline .history and .partial markers inside WAL spool.
printf '1\t0/2000000\te2e timeline marker\n' > "${WAL_DIR}/00000002.history"
dd if=/dev/zero of="${WAL_DIR}/0000000100000000000000FF.partial" bs=1024 count=64 status=none

TODAY_STAMP="$(date -u +%Y%m%d_%H%M%S)"
DUMP_OLD="${BASE_DIR}/pg_20250101_020000.sql"
DUMP_MID="${BASE_DIR}/pg_20250102_020000.sql"
DUMP_FRESH="${BASE_DIR}/pg_${TODAY_STAMP}.sql"
docker exec "$PG_CONTAINER" pg_dump -U "$PG_USER" -d "$PG_DB" > "$DUMP_FRESH"
cp "$DUMP_FRESH" "$DUMP_MID"
cp "$DUMP_FRESH" "$DUMP_OLD"
touch -d "2025-01-01 02:00:00 UTC" "$DUMP_OLD"
touch -d "2025-01-02 02:00:00 UTC" "$DUMP_MID"

# Make WAL segments idle (> 2s) and distinctly ordered so retention picks oldest deterministically.
# Run touch inside the PG container so ownership of postgres-archived WAL files never blocks mtime updates.
mapfile -t WAL_LIST < <(find "$WAL_DIR" -maxdepth 1 -type f -regextype posix-extended -regex '.*/[0-9A-F]{24}$' | sort)
idx=0
for w in "${WAL_LIST[@]}"; do
    w_name="$(basename "$w")"
    docker exec "$PG_CONTAINER" touch -d "@$((1735689600 + idx * 86400))" "/wal_archive/${w_name}"
    idx=$((idx + 1))
done
OLDEST_WAL_NAME="$(basename "${WAL_LIST[0]}")"

# ---------------------------------------------------------------------------
# Scenario 2: Active Writer In-Use Protection
# ---------------------------------------------------------------------------
log "Scenario 2: Verifying active writer protection (check-in-use exit 10 + skip_in_use)..."
# Place BUSY_DUMP as the oldest filename date so retention selects it and in-use guard must protect it.
BUSY_DUMP="${BASE_DIR}/pg_20241229_020000.sql"
cp "$DUMP_FRESH" "$BUSY_DUMP"
touch -d "2024-12-29 02:00:00 UTC" "$BUSY_DUMP"
touch "${BUSY_DUMP}.tmp"
python3 - "$BUSY_DUMP" <<'PY' &
import fcntl, sys, time
with open(sys.argv[1], "r+b") as f:
    fcntl.flock(f.fileno(), fcntl.LOCK_EX)
    time.sleep(60)
PY
WRITER_PID=$!
sleep 0.4

set +e
"$FC_E2E_BIN" check-in-use "$BUSY_DUMP" >/dev/null 2>&1
IN_USE_RC=$?
set -e
[[ "$IN_USE_RC" -eq 10 ]] || { echo "ERROR: expected check-in-use exit 10, got $IN_USE_RC" >&2; exit 1; }

# Render live E2E configuration from canonical embedded template.
E2E_CFG="${WORK_DIR}/e2e_dba_s3.toml"
cat <<EOF > "$E2E_CFG"
# BEGIN_FILECLEANER_E2E_CONFIG
dry_run = false

[audit]
log_path = "${AUDIT_LOG}"
jsonl = true

[s3]
bucket = "${BUCKET}"
region = "us-east-1"
endpoint = "${MINIO_ENDPOINT}"
force_path_style = true
allow_insecure_http = true
overwrite = false
verify_mode = "checksum_sha256"
access_key_env = "FC_E2E_S3_ACCESS_KEY"
secret_key_env = "FC_E2E_S3_SECRET_KEY"
timeout = "30s"
max_retries = 1
multipart_part_size = "16MiB"

[[rules]]
name = "pg-base-backups"
path = "${BASE_DIR}"
include_globs = ["pg_*.sql"]
time_basis = "filename_date"
filename_date_regex = 'pg_(\d{8}_\d{6})'
filename_date_format = "%Y%m%d_%H%M%S"
max_files = 2
actions = ["compress", "upload_s3", "delete"]
compression = "gzip"

[rules.s3]
bucket = "${BUCKET}"
region = "us-east-1"
endpoint = "${MINIO_ENDPOINT}"
prefix = "postgres/base"
force_path_style = true
allow_insecure_http = true
overwrite = false
verify_mode = "checksum_sha256"
access_key_env = "FC_E2E_S3_ACCESS_KEY"
secret_key_env = "FC_E2E_S3_SECRET_KEY"

[rules.backup_guard]
enabled = true
min_keep_files = 2
min_valid_backup_bytes = "1KiB"
require_newer_within = "36h"
freeze_on_empty_valid = true

[rules.in_use]
enabled = true
check_sidecar_locks = true
check_os_locks = true
check_proc_fd = true
min_idle_age = "1s"
stability_window = "0s"

[[rules]]
name = "pg-wal-archive"
path = "${WAL_DIR}"
include_regex = '^[0-9A-F]{24}$'
exclude_globs = ["*.history", "*.partial", "*.ready", "*.done", "*.tmp", "archive_status/**"]
min_file_size = "16MiB"
time_basis = "modified"
max_files = 2
actions = ["compress", "upload_s3", "delete"]
compression = "lz4"

[rules.s3]
bucket = "${BUCKET}"
region = "us-east-1"
endpoint = "${MINIO_ENDPOINT}"
prefix = "postgres/wal"
force_path_style = true
allow_insecure_http = true
overwrite = false
verify_mode = "checksum_sha256"
access_key_env = "FC_E2E_S3_ACCESS_KEY"
secret_key_env = "FC_E2E_S3_SECRET_KEY"

[rules.backup_guard]
enabled = true
min_keep_files = 2
min_valid_backup_bytes = "16MiB"
freeze_on_empty_valid = true

[rules.in_use]
enabled = true
check_sidecar_locks = true
check_os_locks = true
check_proc_fd = true
min_idle_age = "1s"
stability_window = "0s"
# END_FILECLEANER_E2E_CONFIG
EOF

"$FC_E2E_BIN" validate "$E2E_CFG" >/dev/null
"$FC_E2E_BIN" plan -c "$E2E_CFG" --json >/dev/null

# ---------------------------------------------------------------------------
# Scenario 3: Verified MinIO S3 Archival (Base Backups + 16 MiB WAL Segments)
# ---------------------------------------------------------------------------
log "Scenario 3: Running live FileCleaner compress -> upload_s3 (SHA-256) -> delete against MinIO..."
"$FC_E2E_BIN" run -c "$E2E_CFG" --fail-on-freeze

[[ ! -e "$DUMP_OLD" ]] || { echo "ERROR: archived dump $DUMP_OLD should be deleted locally" >&2; exit 1; }
[[ -e "$DUMP_FRESH" ]] || { echo "ERROR: newest backup $DUMP_FRESH must remain on floor" >&2; exit 1; }
[[ -e "$BUSY_DUMP" ]] || { echo "ERROR: in-use dump $BUSY_DUMP must remain untouched" >&2; exit 1; }
[[ ! -e "${WAL_DIR}/${OLDEST_WAL_NAME}" ]] || { echo "ERROR: archived WAL $OLDEST_WAL_NAME should be deleted locally" >&2; exit 1; }
[[ -e "${WAL_DIR}/00000002.history" ]] || { echo "ERROR: timeline .history marker must remain untouched" >&2; exit 1; }
[[ -e "${WAL_DIR}/0000000100000000000000FF.partial" ]] || { echo "ERROR: .partial WAL marker must remain untouched" >&2; exit 1; }

# Verify MinIO exposes x-amz-checksum-sha256 on both archived base dump and WAL object.
python3 "$S3_HELPER" head "$MINIO_ENDPOINT" "$BUCKET" "postgres/base/pg_20250101_020000.sql.gz" >/dev/null
python3 "$S3_HELPER" head "$MINIO_ENDPOINT" "$BUCKET" "postgres/wal/${OLDEST_WAL_NAME}.lz4" >/dev/null

# Release active writer lock cleanly.
kill "$WRITER_PID" >/dev/null 2>&1 || true
wait "$WRITER_PID" >/dev/null 2>&1 || true
WRITER_PID=""
rm -f "${BUSY_DUMP}.tmp"

# ---------------------------------------------------------------------------
# Scenario 4: Fail-Closed Fault Injection (MinIO Down + Checksum Mismatch)
# ---------------------------------------------------------------------------
log "Scenario 4: Injecting MinIO outage and corrupt-checksum server (asserting exit 1 & 100% local retention)..."
FAULT_DUMP="${BASE_DIR}/pg_20241231_020000.sql"
cp "$DUMP_FRESH" "$FAULT_DUMP"
touch -d "2024-12-31 02:00:00 UTC" "$FAULT_DUMP"

docker stop "$MINIO_CONTAINER" >/dev/null
set +e
"$FC_E2E_BIN" run -c "$E2E_CFG" --rule pg-base-backups >/dev/null 2>&1
OUTAGE_RC=$?
set -e
[[ "$OUTAGE_RC" -eq 1 ]] || { echo "ERROR: expected exit 1 during MinIO outage, got $OUTAGE_RC" >&2; exit 1; }
[[ -e "$FAULT_DUMP" || -e "${FAULT_DUMP}.gz" ]] || { echo "ERROR: backup lost during MinIO outage!" >&2; exit 1; }

# Start corrupt-checksum loopback server on MOCK_PORT and verify fail-closed IntegrityMismatch.
python3 - "$MOCK_PORT" <<'PY' &
import http.server, sys
class CorruptS3(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_HEAD(self):
        if getattr(self.server, "uploaded", 0) > 0:
            self.send_response(200)
            self.send_header("Content-Length", str(self.server.uploaded))
            self.send_header("ETag", '"bad"')
            self.send_header("x-amz-checksum-sha256", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
            self.end_headers()
        else:
            self.send_response(404); self.send_header("Content-Length", "0"); self.end_headers()
    def do_PUT(self):
        n = int(self.headers.get("Content-Length", "0"))
        self.rfile.read(n)
        self.server.uploaded = n
        self.send_response(200); self.send_header("Content-Length", "0"); self.send_header("ETag", '"bad"'); self.end_headers()
srv = http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), CorruptS3)
srv.uploaded = 0
srv.serve_forever()
PY
CORRUPT_MOCK_PID=$!
MOCK_READY=0
for _ in $(seq 1 30); do
    if python3 -c "import socket; s = socket.create_connection(('127.0.0.1', ${MOCK_PORT}), timeout=0.5); s.close()" >/dev/null 2>&1; then
        MOCK_READY=1
        break
    fi
    sleep 0.1
done
[[ "$MOCK_READY" -eq 1 ]] || { echo "ERROR: corrupt-checksum mock server did not start on port ${MOCK_PORT}" >&2; exit 1; }

CORRUPT_CFG="${WORK_DIR}/corrupt_s3.toml"
sed "s|${MINIO_ENDPOINT}|http://127.0.0.1:${MOCK_PORT}|g" "$E2E_CFG" > "$CORRUPT_CFG"
FAULT_DUMP_2="${BASE_DIR}/pg_20241230_020000.sql"
cp "$DUMP_FRESH" "$FAULT_DUMP_2"
touch -d "2024-12-30 02:00:00 UTC" "$FAULT_DUMP_2"

CORRUPT_LOG="${WORK_DIR}/corrupt_stderr.log"
set +e
"$FC_E2E_BIN" run -c "$CORRUPT_CFG" --rule pg-base-backups >"$CORRUPT_LOG" 2>&1
CORRUPT_RC=$?
set -e
[[ "$CORRUPT_RC" -eq 1 ]] || { echo "ERROR: expected exit 1 on checksum mismatch, got $CORRUPT_RC" >&2; cat "$CORRUPT_LOG" >&2; exit 1; }
grep -qiE "integrity|checksum|mismatch" "$CORRUPT_LOG" || { echo "ERROR: expected checksum mismatch diagnostic, got:" >&2; cat "$CORRUPT_LOG" >&2; exit 1; }
[[ -e "$FAULT_DUMP_2" || -e "${FAULT_DUMP_2}.gz" ]] || { echo "ERROR: backup lost on checksum mismatch!" >&2; exit 1; }

kill "$CORRUPT_MOCK_PID" >/dev/null 2>&1 || true
wait "$CORRUPT_MOCK_PID" >/dev/null 2>&1 || true
CORRUPT_MOCK_PID=""

docker start "$MINIO_CONTAINER" >/dev/null
MINIO_RESTARTED=0
for _ in $(seq 1 30); do
    if python3 -c "import urllib.request; urllib.request.urlopen('${MINIO_ENDPOINT}/minio/health/live', timeout=2)" >/dev/null 2>&1; then
        MINIO_RESTARTED=1
        break
    fi
    sleep 1
done
[[ "$MINIO_RESTARTED" -eq 1 ]] || { echo "ERROR: MinIO container did not recover after restart" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Scenario 5: Dead-Man's-Switch Staleness Freeze (0-byte Failed Dump Stub)
# ---------------------------------------------------------------------------
log "Scenario 5: Verifying dead-man's-switch freeze (--fail-on-freeze exit 3) on 0-byte stub..."
mv "$DUMP_FRESH" "${WORK_DIR}/fresh_saved.sql"
: > "$DUMP_FRESH" # 0-byte failed cron dump stub
STALE_EXTRA="${BASE_DIR}/pg_20241228_020000.sql"
cp "${WORK_DIR}/fresh_saved.sql" "$STALE_EXTRA"
touch -d "2024-12-28 02:00:00 UTC" "$STALE_EXTRA"
set +e
"$FC_E2E_BIN" run -c "$E2E_CFG" --rule pg-base-backups --fail-on-freeze >/dev/null 2>&1
FREEZE_RC=$?
set -e
[[ "$FREEZE_RC" -eq 3 ]] || { echo "ERROR: expected exit 3 on staleness freeze, got $FREEZE_RC" >&2; exit 1; }
[[ -e "$DUMP_MID" && -e "$STALE_EXTRA" ]] || { echo "ERROR: dump deleted during active freeze!" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Scenario 6: Download from MinIO, Verify SHA-256 vs audit.jsonl & Restore to PostgreSQL
# ---------------------------------------------------------------------------
log "Scenario 6: Downloading archived dump & WAL from MinIO, verifying SHA-256, and restoring PostgreSQL..."
DL_DUMP_GZ="${RESTORE_DIR}/pg_20250101_020000.sql.gz"
DL_WAL_LZ4="${RESTORE_DIR}/${OLDEST_WAL_NAME}.lz4"
python3 "$S3_HELPER" get "$MINIO_ENDPOINT" "$BUCKET" "postgres/base/pg_20250101_020000.sql.gz" "$DL_DUMP_GZ"
python3 "$S3_HELPER" get "$MINIO_ENDPOINT" "$BUCKET" "postgres/wal/${OLDEST_WAL_NAME}.lz4" "$DL_WAL_LZ4"

python3 - "$AUDIT_LOG" "$DL_DUMP_GZ" "$DL_WAL_LZ4" <<'PY'
import hashlib, json, os, sys
audit_path, dump_gz, wal_lz4 = sys.argv[1:4]
expected = {}
with open(audit_path, "r", encoding="utf-8") as f:
    for line in f:
        rec = json.loads(line)
        if rec.get("event") == "action_exec" and not rec.get("dry_run") and rec.get("details"):
            uri = rec["details"].get("s3_uri", "")
            if uri and rec.get("sha256"):
                expected[uri] = rec["sha256"]

def sha256_file(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()

dump_sha = sha256_file(dump_gz)
wal_sha = sha256_file(wal_lz4)
wal_name = os.path.basename(wal_lz4)
assert any(u.endswith("/pg_20250101_020000.sql.gz") and s == dump_sha for u, s in expected.items()), f"dump sha mismatch: {dump_sha} vs {expected}"
assert any(u.endswith(f"/{wal_name}") and s == wal_sha for u, s in expected.items()), f"wal sha mismatch for {wal_name}: {wal_sha} vs {expected}"
print(f"Verified MinIO SHA-256 matches audit.jsonl: dump={dump_sha[:16]}... wal={wal_sha[:16]}...")
PY

gzip -dc "$DL_DUMP_GZ" > "${RESTORE_DIR}/restored.sql"
docker exec "$PG_CONTAINER" psql -U "$PG_USER" -d postgres -v ON_ERROR_STOP=1 -c "CREATE DATABASE fc_e2e_restore;" >/dev/null
docker exec -i "$PG_CONTAINER" psql -U "$PG_USER" -d fc_e2e_restore -v ON_ERROR_STOP=1 < "${RESTORE_DIR}/restored.sql" >/dev/null
ROW_COUNT="$(docker exec "$PG_CONTAINER" psql -U "$PG_USER" -d fc_e2e_restore -tAc "SELECT count(*) FROM fc_e2e_ledger;")"
[[ "$ROW_COUNT" == "253" ]] || { echo "ERROR: expected 253 restored rows, got '$ROW_COUNT'" >&2; exit 1; }

log "ALL 6 LIVE DOCKER E2E SCENARIOS PASSED (PostgreSQL 17 + MinIO S3 SHA-256 + Full Restore: ${ROW_COUNT} rows)!"
