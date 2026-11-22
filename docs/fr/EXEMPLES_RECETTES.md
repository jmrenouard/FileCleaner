<div align="center">
  <img src="../assets/header-banner.svg" alt="FileCleaner — Exemples & Recettes de Production" width="100%" />
</div>

# FileCleaner v1.0.1 — Exemples & Recettes de Production

> **Éditions linguistiques** : [English (`docs/en/COOKBOOK_EXAMPLES.md`)](../en/COOKBOOK_EXAMPLES.md) · **Français**  
> **Documents associés** : [README](../../README.fr.md) · [HOWTO 5 minutes](HOWTO.md) · [Guide utilisateur](GUIDE_UTILISATEUR.md) · [Spécifications formelles](SPECIFICATIONS.md)

Chaque configuration présentée dans ces recettes respecte strictement `src/config.rs` et les invariants `Config::validate()` (`INV-CFG-01`..`INV-CFG-14`). Validez toujours vos adaptations avec `filecleaner validate` et `filecleaner plan` avant toute exécution réelle.

---

## Table des matières

1. [Recette 1 : Rétention nocturne PostgreSQL & MySQL (Homme-mort + `Move` inter-FS SHA-256 vers NAS)](#1-recette-1--rétention-nocturne-postgresql--mysql)
2. [Recette 2 : Rotation de journaux applicatifs & Nginx (`CopyTruncate` opt-in vs `RenameByDate` + `Lz4`/`Brotli`)](#2-recette-2--rotation-de-journaux-applicatifs--nginx)
3. [Recette 3 : File d'entrée ETL haut débit (Fichiers annexes, `flock`, fenêtre de stabilité & veto code `42`)](#3-recette-3--file-dentrée-etl-haut-débit)
4. [Recette 4 : Déploiement démon multi-règles (Mode `Hybrid`, PID/État/Stop & alertes strictes)](#4-recette-4--déploiement-démon-multi-règles)
5. [Recette 5 : Archivage S3 / MinIO des sauvegardes de base et WAL PostgreSQL (`upload_s3` + Garde PITR + Labo Docker E2E)](#5-recette-5--archivage-s3--minio-des-sauvegardes-de-base-et-wal-postgresql)

---

## 1. Recette 1 : Rétention nocturne PostgreSQL & MySQL

### Objectif opérationnel

Protéger les répertoires locaux de dumps (`/var/backups/postgres` et `/var/backups/mysql`) tout en archivant les sauvegardes plus anciennes vers un montage NAS froid (`/mnt/cold-nas/...`) :

- Extraire l'horodatage directement depuis le **nom de fichier** (`time_basis = "filename_date"`), indépendamment de toute dérive de `mtime` lors d'un `scp`/`rsync`.
- Écarter les dumps corrompus ou vides de `0` octet (`min_valid_backup_bytes = "1MiB"`) afin qu'ils ne puissent jamais valider la fraîcheur.
- Conserver en toutes circonstances les **7 dernières sauvegardes valides locales** (`min_keep_files = 7`).
- **Interrupteur homme-mort** : si aucune sauvegarde valide n'a moins de `36h` (`require_newer_within = "36h"`), geler immédiatement toute éviction ($E = \emptyset$).
- Compresser les sauvegardes éligibles en **gzip** pure-Rust puis les déplacer (`move`) par sous-dossiers de date (`%Y/%m`) sur le NAS avec double lecture et vérification SHA-256 inter-systèmes de fichiers.

### Configuration (`/etc/filecleaner/db-backups.toml`)

```toml
dry_run = false

[audit]
log_path = "/var/log/filecleaner/db-backups-audit.jsonl"
jsonl = true

[[rules]]
name = "postgres-nightly"
path = "/var/backups/postgres"
recursive = false
follow_symlinks = false
include_globs = ["pg_*.sql", "pg_*.dump"]
exclude_globs = ["pinned/**"]
time_basis = "filename_date"
filename_date_regex = 'pg_(\d{8}_\d{6})'
filename_date_format = "%Y%m%d_%H%M%S"
max_files = 14
max_age = "14d"
max_total_size = "100GiB"
combine_mode = "any"
eviction_order = "oldest_first"
actions = ["compress", "move"]
compression = "gzip"
compress_remove_source = true
compress_overwrite = false
move_to = "/mnt/cold-nas/postgres"
move_date_subdir = "%Y/%m"
move_overwrite = false

[rules.backup_guard]
enabled = true
min_keep_files = 7
min_valid_backup_bytes = "1MiB"
require_newer_within = "36h"
freeze_on_empty_valid = true

[rules.in_use]
enabled = true
check_sidecar_locks = true
sidecar_suffixes = [".lock", ".part", ".tmp", ".writing"]
check_os_locks = true
check_proc_fd = true
min_idle_age = "5m"
stability_window = "5s"

[[rules]]
name = "mysql-nightly"
path = "/var/backups/mysql"
recursive = false
follow_symlinks = false
include_globs = ["mysql_*.sql", "mysql_*.xbstream"]
time_basis = "filename_date"
filename_date_regex = 'mysql_(\d{4}-\d{2}-\d{2}_\d{2}-\d{2}-\d{2})'
filename_date_format = "%Y-%m-%d_%H-%M-%S"
max_files = 10
max_total_size = "80GiB"
combine_mode = "any"
eviction_order = "oldest_first"
actions = ["compress", "move"]
compression = "gzip"
compress_remove_source = true
move_to = "/mnt/cold-nas/mysql"
move_date_subdir = "%Y/%m"

[rules.backup_guard]
enabled = true
min_keep_files = 7
min_valid_backup_bytes = "1MiB"
require_newer_within = "36h"
freeze_on_empty_valid = true

[rules.in_use]
enabled = true
min_idle_age = "5m"
stability_window = "5s"
```

### Commandes de contrôle et d'exécution

```bash
# 1. Validation statique
filecleaner validate -c /etc/filecleaner/db-backups.toml

# 2. Plan d'exécution déterministe en lecture seule
filecleaner plan -c /etc/filecleaner/db-backups.toml --rule postgres-nightly

# 3. Passe cron / systemd oneshot avec alerte stricte sur gel ou contrainte (code 3)
filecleaner run -c /etc/filecleaner/db-backups.toml --fail-on-freeze --strict-constraints
```

### Sorties CLI et journal JSONL attendues

**Scénario A — Passe nominale (code `0`)** :

```text
configuration valid: 2 rule(s) loaded from /etc/filecleaner/db-backups.toml
```

```json
{"ts":"2026-11-20T04:00:02.118Z","event":"file_decision","run_id":"run-18204-1","rule":"postgres-nightly","path":"/var/backups/postgres/pg_20261105_020000.sql","size_bytes":524288000,"resolved_ts":"2026-11-05T02:00:00Z","decision":"acted","in_use_reasons":null,"actions":["Compress(Gzip)","Move"],"dest_path":"/mnt/cold-nas/postgres/2026/11/pg_20261105_020000.sql.gz","sha256":"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08","dry_run":false,"duration_ms":3410,"error":null}
```

**Scénario B — Échec silencieux du dump nocturne (ébauche de `0` octet + dernière sauvegarde > `36h`, code `3`)** :

Même si un fichier vide `pg_20261120_020000.sql` de `0` octet existe, `min_valid_backup_bytes = "1MiB"` l'écarte de $\mathcal{F}_{\text{valid}}$. La dernière sauvegarde valide datant de plus de `36h`, `BackupGuard` gèle toute éviction et `--fail-on-freeze` renvoie le code de sortie `3` :

```json
{"ts":"2026-11-20T15:00:00.012Z","event":"backup_freeze","run_id":"run-19102-1","rule":"postgres-nightly","path":null,"size_bytes":null,"resolved_ts":null,"decision":"freeze","in_use_reasons":null,"actions":[],"dest_path":null,"sha256":null,"dry_run":false,"duration_ms":4,"error":"newest valid backup (2026-11-18T02:00:00Z) exceeds require_newer_within=36h; rule frozen"}
```

---

## 2. Recette 2 : Rotation de journaux applicatifs & Nginx

### Objectif opérationnel

Traiter deux cas concrets dans une seule configuration YAML (`/etc/filecleaner/logs-rotation.yaml`) :

1. **Stratégie A (`app-logs-copy-truncate`)** : Service Java/Python ancien écrivant en continu sur un descripteur `O_APPEND` (`app.log`) sans réouverture sur `SIGHUP`/`SIGUSR1`. Utilise **`copy_truncate` en activation explicite** avec archivage **LZ4** rapide et `check_os_locks: false` / `check_proc_fd: false`.
2. **Stratégie B (`nginx-reopened-logs`)** : Journaux Nginx déjà basculés via `nginx -s reopen` (`access.log.1`, `error.log.1`), utilisant **`rename_by_date` + compression haute densité `brotli`** sans fenêtre de perte de troncature.

> **Avertissement de compromis (`CopyTruncate`)** : `CopyTruncate` copie le contenu de `src` vers `<archive>.fc-tmp` puis appelle `file.set_len(0)` sans bloquer l'écrivain externe ; tout octet écrit entre la fin de lecture de la copie et `set_len(0)` est perdu. Privilégiez toujours la **Stratégie B** (`rename_by_date` + `compress` après réouverture `SIGUSR1`) dès que l'application le permet.

### Configuration (`/etc/filecleaner/logs-rotation.yaml`)

```yaml
dry_run: false

audit:
  log_path: /var/log/filecleaner/logs-audit.jsonl
  jsonl: true

rules:
  # Strategy A: Explicit opt-in CopyTruncate for non-cooperating open O_APPEND logs
  - name: app-logs-copy-truncate
    path: /var/log/legacy-app
    recursive: false
    include_globs:
      - "*.log"
    min_file_size: 100MiB
    time_basis: modified
    max_age: 1d
    max_total_size: 2GiB
    combine_mode: any
    eviction_order: largest_first
    in_use:
      enabled: true
      check_sidecar_locks: true
      # Disable OS handle/FD probes because legacy-app intentionally holds app.log open
      check_os_locks: false
      check_proc_fd: false
      min_idle_age: 5s
      stability_window: 0s
    actions:
      - copy_truncate
    copy_truncate_to: /var/log/legacy-app/rotated
    compression: lz4

  # Strategy B: Cooperating Nginx logs -> RenameByDate + Brotli compression
  - name: nginx-reopened-logs
    path: /var/log/nginx
    recursive: true
    max_depth: 2
    include_globs:
      - "*.log.[0-9]*"
      - "*.log.rotated"
    exclude_globs:
      - "archive/**"
    time_basis: modified
    max_files: 30
    max_age: 14d
    max_total_size: 10GiB
    combine_mode: any
    eviction_order: oldest_first
    in_use:
      enabled: true
      check_os_locks: true
      check_proc_fd: true
      min_idle_age: 30s
      stability_window: 2s
    actions:
      - rename_by_date
      - compress
      - move
    rename_template: "%Y%m%d_%H%M%S"
    compression: brotli
    compression_level: 6
    compress_remove_source: true
    move_to: /var/log/nginx/archive
    move_date_subdir: "%Y/%m"
```

Notez que `exclude_globs: ["archive/**"]` sur la règle récursive `nginx-reopened-logs` satisfait `INV-CFG-09` afin que `/var/log/nginx/archive` ne soit jamais réexploré.

### Validation, planification et sortie attendue

```bash
filecleaner validate -c /etc/filecleaner/logs-rotation.yaml
filecleaner plan -c /etc/filecleaner/logs-rotation.yaml --json
```

Sortie attendue de `validate` et extrait `plan --json` :

```text
warning: rule 'app-logs-copy-truncate' enables CopyTruncate; bytes written between archive copy EOF and truncate may be lost for non-cooperating writers
configuration valid: 2 rule(s) loaded from /etc/filecleaner/logs-rotation.yaml
```

```json
{"ts":"2026-11-20T09:15:00.044Z","event":"file_decision","run_id":"plan-9012-1","rule":"nginx-reopened-logs","path":"/var/log/nginx/access.log.1","size_bytes":268435456,"resolved_ts":"2026-11-19T00:00:05Z","decision":"would_act","in_use_reasons":null,"actions":["RenameByDate","Compress(Brotli)","Move"],"dest_path":"/var/log/nginx/archive/2026/11/access.log_20261119_000005.1.br","sha256":null,"dry_run":true,"duration_ms":3,"error":null}
```

---

## 3. Recette 3 : File d'entrée ETL haut débit

### Objectif opérationnel

Purger les lots Parquet/CSV déjà ingérés dans `/var/spool/etl/incoming` **uniquement** lorsque producteurs et consommateurs ont terminé :

- Protéger les transferts en cours par marqueurs annexes (`.part`, `.lock`, `.uploading`, `.processing`), verrou Unix `flock`, scan Linux `/proc/*/fd`, `min_idle_age = "30s"`, et fenêtre de stabilité groupée de `3s` sur `(size, mtime)`.
- Attacher un **`pre_delete_hook`** qui contrôle l'existence d'un reçu `.sha256.verified` et la validité de l'empreinte SHA-256 avant toute compression/mise en corbeille.
- Si le reçu est absent (ingestion encore en cours), le hook renvoie **`42`** (veto bénin, code global conservé à `0`). En cas d'empreinte divergente, il renvoie **`1`** (`abort_file`) et remonte une erreur.

### Script hook (`/usr/local/bin/verify_etl_manifest.sh`)

```bash
#!/usr/bin/env bash
set -euo pipefail

# FileCleaner executes hook commands directly without a shell (argv + FILECLEANER_* env).
TARGET="${FILECLEANER_PATH:?missing FILECLEANER_PATH}"
RECEIPT="${TARGET}.sha256.verified"

# Return exit 42 (skip_exit_code) to veto eviction safely if ingestion receipt is absent
if [[ ! -f "${RECEIPT}" ]]; then
  echo "veto: ingestion receipt missing for ${FILECLEANER_NAME} (rule=${FILECLEANER_RULE})" >&2
  exit 42
fi

EXPECTED_HASH="$(awk '{print $1}' "${RECEIPT}")"
ACTUAL_HASH="$(sha256sum "${TARGET}" | awk '{print $1}')"

if [[ -z "${EXPECTED_HASH}" || "${EXPECTED_HASH}" != "${ACTUAL_HASH}" ]]; then
  echo "error: checksum mismatch on ${TARGET} (expected=${EXPECTED_HASH}, actual=${ACTUAL_HASH})" >&2
  exit 1
fi

exit 0
```

Rendez-le exécutable (`chmod 0755 /usr/local/bin/verify_etl_manifest.sh`).

### Configuration (`/etc/filecleaner/etl-spool.toml`)

```toml
dry_run = false
global_trash_dir = "/var/spool/etl/.quarantine"

[audit]
log_path = "/var/log/filecleaner/etl-spool-audit.jsonl"
jsonl = true

[[rules]]
name = "etl-incoming-parquet"
path = "/var/spool/etl/incoming"
recursive = true
max_depth = 3
follow_symlinks = false
include_globs = ["*.parquet", "*.csv"]
exclude_globs = ["*.sha256.verified", "quarantine/**"]
min_file_size = "1B"
time_basis = "modified"
max_age = "6h"
max_total_size = "200GiB"
combine_mode = "all"
eviction_order = "oldest_first"
actions = ["compress", "trash"]
compression = "lz4"
compress_remove_source = true

[rules.in_use]
enabled = true
check_sidecar_locks = true
sidecar_suffixes = [".lock", ".lck", ".part", ".tmp", ".processing", ".uploading"]
check_os_locks = true
check_proc_fd = true
min_idle_age = "30s"
stability_window = "3s"

[rules.pre_delete_hook]
command = "/usr/local/bin/verify_etl_manifest.sh"
args = ["--file", "{path}", "--size", "{size}", "--rule", "{rule}"]
timeout = "10s"
on_failure = "abort_file"
skip_exit_code = 42
run_in_dry_run = false
```

### Vérifications et journaux attendus

```bash
# Sonde unitaire avant un traitement manuel
filecleaner check-in-use /var/spool/etl/incoming/batch_20261120_0900.parquet --idle-age 30s --stability 3s --json
echo "exit=$?"   # 10 si occupé (ex. .part ou flock actif), 0 si stabilisé

# Exécution de la passe
filecleaner run -c /etc/filecleaner/etl-spool.toml
```

Sortie `check-in-use` sur un fichier en cours de transfert (code `10`) :

```json
{"all_safe":false,"busy_count":1,"reports":[{"path":"/var/spool/etl/incoming/batch_20261120_0900.parquet","is_busy":true,"reasons":[{"kind":"sidecar_lock","lock_path":"/var/spool/etl/incoming/batch_20261120_0900.parquet.part"},{"kind":"min_idle_age","age_ms":4120,"min_idle_ms":30000}]}]}
```

Événements JSONL illustrant le saut d'un fichier actif (`skip_in_use`), un veto de hook par code `42` (`skip_hook`) et un archivage vérifié (`acted`) :

```json
{"ts":"2026-11-20T09:20:03.012Z","event":"file_decision","run_id":"run-30112-1","rule":"etl-incoming-parquet","path":"/var/spool/etl/incoming/batch_20261120_0900.parquet","size_bytes":67108864,"resolved_ts":"2026-11-20T09:19:58Z","decision":"skip_in_use","in_use_reasons":["SidecarLock","MinIdleAge"],"actions":[],"dest_path":null,"sha256":null,"dry_run":false,"duration_ms":3002,"error":null}
{"ts":"2026-11-20T09:20:03.040Z","event":"file_decision","run_id":"run-30112-1","rule":"etl-incoming-parquet","path":"/var/spool/etl/incoming/batch_20261120_0100.parquet","size_bytes":134217728,"resolved_ts":"2026-11-20T01:00:00Z","decision":"skip_hook","in_use_reasons":null,"actions":[],"dest_path":null,"sha256":null,"dry_run":false,"duration_ms":14,"error":"hook returned skip_exit_code=42"}
{"ts":"2026-11-20T09:20:03.590Z","event":"file_decision","run_id":"run-30112-1","rule":"etl-incoming-parquet","path":"/var/spool/etl/incoming/batch_20261119_2200.parquet","size_bytes":134217728,"resolved_ts":"2026-11-19T22:00:00Z","decision":"acted","in_use_reasons":null,"actions":["Compress(Lz4)","Trash"],"dest_path":"/var/spool/etl/.quarantine/batch_20261119_2200.parquet.lz4","sha256":null,"dry_run":false,"duration_ms":548,"error":null}
```

---

## 4. Recette 4 : Déploiement démon multi-règles

### Objectif opérationnel

Déployer un démon continu multi-règles sur un serveur de production associant :

- **Planification `Hybrid`** (`mode = "hybrid"`, `interval = "5m"`, `watch = true`, `debounce = "2s"`) alliant réactivité événementielle et évaluation périodique garantie des seuils d'âge et de fraîcheur.
- Fichiers de contrôle atomiques **PID**, **état JSON** et **arrêt coopératif** sous `/run/filecleaner/`.
- Plusieurs règles indépendantes (`db-backups`, `nginx-logs`, `tmp-build-artifacts`) dotées de leurs propres réglages `BackupGuard` et `InUse`.
- Supervision stricte par `--fail-on-freeze` et `--strict-constraints` (code de sortie `3`) et consultation de l'état temps réel.

### Configuration (`/etc/filecleaner/production-daemon.toml`)

```toml
dry_run = false
global_trash_dir = "/var/tmp/filecleaner-quarantine"

[daemon]
mode = "hybrid"
interval = "5m"
watch = true
debounce = "2s"
sweep_on_start = true
pid_file = "/run/filecleaner/filecleaner.pid"
state_file = "/run/filecleaner/state.json"
stop_file = "/run/filecleaner/filecleaner.stop"

[audit]
log_path = "/var/log/filecleaner/daemon-audit.jsonl"
jsonl = true

[[rules]]
name = "db-backups"
path = "/var/backups/postgres"
include_globs = ["*.sql", "*.dump"]
time_basis = "filename_date"
filename_date_regex = 'pg_(\d{8}_\d{6})'
filename_date_format = "%Y%m%d_%H%M%S"
max_files = 14
max_total_size = "40GiB"
combine_mode = "any"
eviction_order = "oldest_first"
actions = ["compress", "move"]
compression = "gzip"
move_to = "/mnt/cold-nas/postgres"
move_date_subdir = "%Y/%m"

[rules.backup_guard]
enabled = true
min_keep_files = 5
min_valid_backup_bytes = "1MiB"
require_newer_within = "36h"
freeze_on_empty_valid = true

[[rules]]
name = "nginx-logs"
path = "/var/log/nginx"
include_globs = ["*.log.[0-9]*"]
max_age = "14d"
max_total_size = "10GiB"
actions = ["compress", "move"]
compression = "brotli"
move_to = "/var/archive/nginx"

[rules.in_use]
enabled = true
min_idle_age = "30s"
stability_window = "2s"

[[rules]]
name = "tmp-build-artifacts"
path = "/var/cache/ci-artifacts"
recursive = true
max_depth = 3
max_age = "48h"
max_total_size = "25GiB"
combine_mode = "any"
eviction_order = "largest_first"
actions = ["delete"]

[rules.in_use]
enabled = true
min_idle_age = "2m"
stability_window = "2s"
```

### Déploiement, contrôle de santé et alertes strictes

```bash
# 1. Valider la configuration et générer l'unité systemd
filecleaner validate -c /etc/filecleaner/production-daemon.toml
filecleaner daemon service-template --init-system systemd \
  --config /etc/filecleaner/production-daemon.toml \
  --output /etc/systemd/system/filecleaner.service

sudo systemctl daemon-reload
sudo systemctl enable --now filecleaner

# 2. Consulter l'état et les métriques temps réel du démon
filecleaner daemon status -c /etc/filecleaner/production-daemon.toml --json

# 3. Sonde périodique de supervision (cron / Nagios / timer systemd)
# Retourne immédiatement le code 3 si une règle est gelée ou contrainte !
filecleaner plan -c /etc/filecleaner/production-daemon.toml \
  --fail-on-freeze --strict-constraints --json

# 4. Test borné en CI (exécute exactement 1 passe puis s'arrête proprement)
filecleaner daemon run -c /etc/filecleaner/production-daemon.toml --max-sweeps 1 --dry-run

# 5. Arrêt gracieux multiplateforme (termine le fichier en cours, supprime PID et stop_file)
filecleaner daemon stop -c /etc/filecleaner/production-daemon.toml --timeout 30s
```

### Sortie attendue du fichier d'état `state.json`

```json
{
  "version": "1.0.0",
  "pid": 42180,
  "status": "idle",
  "watch_mode": "hybrid",
  "started_at": "2026-11-20T09:00:00.000Z",
  "last_sweep_at": "2026-11-20T09:05:00.214Z",
  "next_sweep_at": "2026-11-20T09:10:00.000Z",
  "sweeps_completed": 2,
  "total_files_evaluated": 312,
  "total_files_processed": 27,
  "total_bytes_reclaimed": 4831838208,
  "active_freezes": [],
  "constrained_rules": ["db-backups"],
  "last_error": null
}
```

Ici, `constrained_rules: ["db-backups"]` signale que le plancher minimal obligatoire `min_keep_files = 5` dépasse déjà le plafond `40GiB` : FileCleaner a préservé les `5` derniers dumps valides, évincé tout le surplus antérieur et consigné la contrainte pour que l'administrateur puisse agrandir le volume ou ajuster le plancher.

---

## 5. Recette 5 : Archivage S3 / MinIO des sauvegardes de base et WAL PostgreSQL

### Objectif opérationnel

Archiver directement les sauvegardes de base PostgreSQL et les segments WAL de 16 Mio vers AWS S3 ou une passerelle MinIO / Ceph RGW sans aucune dépendance C/C++ et avec intégrité stricte à échec sécurisé :

- **Génération immédiate** : Générez un profil prêt à l'emploi en simulation (`dry_run = true`) via `filecleaner init --profile dba-postgres --format toml` (exemples canoniques fournis dans `examples/dba-postgres-wal-s3.toml` et `examples/dba-postgres-wal-s3.yaml`).
- **Archivage S3 vérifié SHA-256 (`upload_s3`)** : Compresse les dumps (`gzip`) et les segments WAL de 16 Mio (`lz4`), les transfère en SigV4 pure-Rust (`ureq` v3 + `rustls-rustcrypto`), vérifie `x-amz-checksum-sha256` et `Content-Length` par `HeadObject`, puis n'exécute `delete` **qu'après** validation cryptographique complète.
- **Zéro secret en clair** : Les clés S3 proviennent exclusivement des variables d'environnement (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN` optionnel) et sont masquées en mémoire (`RedactedSecret`).
- **Sûreté PITR et timelines PostgreSQL** : Cible uniquement les segments WAL canoniques de 24 caractères hexadécimaux (`^[0-9A-F]{24}$`, `min_file_size = "16MiB"`), exclut inconditionnellement `*.history`, `*.partial`, `*.ready`, `*.done`, `*.tmp` et `archive_status/**`, et attache `examples/hooks/verify_pg_wal_safe.sh` (veto code `42` pour les segments requis par un backup de base ou un réplica en retard).

### Configuration (`/etc/filecleaner/dba-postgres-wal-s3.toml`)

```toml
dry_run = false

[audit]
log_path = "/var/log/filecleaner/pg-wal-s3-audit.jsonl"
jsonl = true

[s3]
bucket = "pg-pitr-archive"
region = "eu-west-3"
endpoint = "https://minio.internal:9000"
force_path_style = true
allow_insecure_http = false
overwrite = false
verify_mode = "checksum_sha256"
access_key_env = "AWS_ACCESS_KEY_ID"
secret_key_env = "AWS_SECRET_ACCESS_KEY"
session_token_env = "AWS_SESSION_TOKEN"
timeout = "60s"
max_retries = 3
multipart_part_size = "16MiB"

[[rules]]
name = "pg-base-backups"
path = "/var/backups/postgres/base"
recursive = true
max_depth = 2
follow_symlinks = false
include_globs = ["*.sql", "*.sql.gz", "*.dump", "*.tar"]
exclude_globs = ["pinned/**", "*.keep"]
min_file_size = "1KiB"
time_basis = "filename_date"
filename_date_regex = 'pg_(\d{8}_\d{6})'
filename_date_format = "%Y%m%d_%H%M%S"
max_files = 14
max_age = "30d"
max_total_size = "200GiB"
actions = ["compress", "upload_s3", "delete"]
compression = "gzip"

[rules.s3]
bucket = "pg-pitr-archive"
region = "eu-west-3"
endpoint = "https://minio.internal:9000"
prefix = "postgres/base-backups"
key_date_subdir = "%Y/%m"
force_path_style = true
overwrite = false
verify_mode = "checksum_sha256"

[rules.backup_guard]
enabled = true
min_keep_files = 7
min_valid_backup_bytes = "1MiB"
require_newer_within = "36h"
freeze_on_empty_valid = true

[rules.in_use]
enabled = true
min_idle_age = "2m"
stability_window = "5s"

[[rules]]
name = "pg-wal-archive"
path = "/var/lib/postgresql/wal_archive"
recursive = false
follow_symlinks = false
include_regex = '^[0-9A-F]{24}$'
exclude_globs = ["*.history", "*.partial", "*.ready", "*.done", "*.tmp", "archive_status/**"]
min_file_size = "16MiB"
time_basis = "modified"
max_files = 64
max_age = "7d"
max_total_size = "100GiB"
actions = ["compress", "upload_s3", "delete"]
compression = "lz4"

[rules.s3]
bucket = "pg-pitr-archive"
region = "eu-west-3"
endpoint = "https://minio.internal:9000"
prefix = "postgres/wal"
key_date_subdir = "%Y/%m/%d"
force_path_style = true
overwrite = false
verify_mode = "checksum_sha256"

[rules.backup_guard]
enabled = true
min_keep_files = 16
min_valid_backup_bytes = "16MiB"
freeze_on_empty_valid = true

[rules.in_use]
enabled = true
min_idle_age = "60s"
stability_window = "3s"

[rules.pre_delete_hook]
command = "/usr/local/bin/verify_pg_wal_safe.sh"
args = ["--file", "{path}", "--name", "{name}", "--rule", "{rule}"]
timeout = "15s"
on_failure = "abort_file"
skip_exit_code = 42
run_in_dry_run = false

[rules.pre_delete_hook.env]
FILECLEANER_PG_MIN_KEEP_WAL_FILE = "/var/lib/postgresql/wal_archive/.pitr_min_keep_wal"
```

### Validation, exécution réelle et laboratoire Docker E2E

```bash
# 1. Générer un profil DBA de démarrage (dry_run = true par défaut)
filecleaner init --profile dba-postgres --format toml --output /etc/filecleaner/dba-postgres-wal-s3.toml

# 2. Exporter les identifiants S3 par variables d'environnement (jamais en clair dans TOML/YAML)
export AWS_ACCESS_KEY_ID="minio-archive-writer"
export AWS_SECRET_ACCESS_KEY="<injecté-depuis-coffre>"

# 3. Valider les invariants (INV-CFG-01..14) et prévisualiser les URI S3 sans E/S réseau
filecleaner validate /etc/filecleaner/dba-postgres-wal-s3.toml
filecleaner plan -c /etc/filecleaner/dba-postgres-wal-s3.toml --json

# 4. Exécuter l'archivage réel à échec sécurisé (code 3 sur gel de fraîcheur ; code 1 sur erreur S3/SHA-256)
filecleaner run -c /etc/filecleaner/dba-postgres-wal-s3.toml --fail-on-freeze

# 5. Laboratoire Docker E2E reproductible (PostgreSQL 17 compatible multi-db-docker-env + MinIO épinglé)
bash scripts/e2e-dba-s3-lab.sh
# Ou via la suite Cargo E2E ignorée par défaut :
cargo test --test e2e_dba_s3_docker_test -- --ignored --nocapture
```

### Enregistrement d'audit S3 attendu (`audit.jsonl`)

```json
{"ts":"2026-11-22T03:04:01.412Z","event":"action_exec","run_id":"run-7681-1","rule":"pg-wal-archive","path":"/var/lib/postgresql/wal_archive/000000010000000000000001","size_bytes":16777216,"resolved_ts":"2026-11-21T02:00:00Z","decision":"acted","in_use_reasons":null,"actions":["Compress(Lz4)","UploadS3","Delete"],"dest_path":"s3://pg-pitr-archive/postgres/wal/2026/11/21/000000010000000000000001.lz4","sha256":"dac906d5f1872d44e1d0f0b47c8e9c9a21f42c44d8f28a391b7c98f3b21c5e12","details":{"etag":"\"597539973be5e225\"","parts_uploaded":1,"s3_uri":"s3://pg-pitr-archive/postgres/wal/2026/11/21/000000010000000000000001.lz4","verified_size":84210,"verify_mode":"checksum_sha256"},"dry_run":false,"duration_ms":64,"error":null}
```
