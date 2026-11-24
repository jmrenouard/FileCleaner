# Tutoriel 2 : Laboratoire BackupGuard et détection active

Ce deuxième laboratoire montre comment FileCleaner protège vos sauvegardes lorsque des archives sont trop petites (sauvegardes tronquées), trop anciennes (panne silencieuse du planificateur) ou activement ouvertes par un autre processus.

## Ce que vous allez apprendre

1. Bloquer automatiquement toute suppression lorsque `BackupGuard` détecte une sauvegarde absente, tronquée ou périmée, et récupérer le code de sortie `3` via `--fail-on-freeze`.
2. Vérifier les 6 sondes de détection `check-in-use` sur des fichiers verrouillés par fichier compagnon `.part` ou descripteur ouvert.

## Étape 1 : Préparer un laboratoire avec sauvegardes valides et corrompues

Créez un espace temporaire contenant une sauvegarde valide de `2 MiB` ainsi qu'une sauvegarde tronquée de `512 B` :

```bash
LAB=$(mktemp -d)
mkdir -p "$LAB/pg-backups" "$LAB/trash"

truncate -s 2M "$LAB/pg-backups/pg_valid_01.dump"
truncate -s 512 "$LAB/pg-backups/pg_corrupt_02.dump"
touch -d "10 days ago" "$LAB/pg-backups/pg_valid_01.dump"
touch -d "20 days ago" "$LAB/pg-backups/pg_corrupt_02.dump"
```

## Étape 2 : Déclencher le gel BackupGuard et le code de sortie 3

Créez `$LAB/guard.toml` exigeant au moins `2` sauvegardes valides d'au moins `1 MiB`, dont une plus récente que `24h` :

```toml
dry_run = false
global_trash_dir = "/tmp/LAB_PATH/trash"

[[rules]]
name = "pg-base"
path = "/tmp/LAB_PATH/pg-backups"
enabled = true
include_globs = ["*.dump"]
time_basis = "modified"
max_files = 3
max_age = "5d"
combine_mode = "any"
actions = ["delete"]

[rules.backup_guard]
enabled = true
min_keep_files = 2
min_valid_backup_bytes = "1MiB"
require_newer_within = "24h"
freeze_on_empty_valid = true
```

Résolvez le chemin et lancez `filecleaner plan --fail-on-freeze` :

```bash
sed "s|/tmp/LAB_PATH|$LAB|g" "$LAB/guard.toml" > "$LAB/guard.resolved.toml"
filecleaner plan -c "$LAB/guard.resolved.toml" --fail-on-freeze
echo "Code de sortie : $?"
```

Observez le résultat : même si `max_age = "5d"` aurait dû supprimer `pg_valid_01.dump` âgé de 10 jours, `BackupGuard` passe en mode `freeze` parce qu'aucune archive valide n'a moins de `24h` et que `min_keep_files = 2` n'est pas atteint. L'option `--fail-on-freeze` renvoie exactement le code `3`, ce qui permet à votre ordonnanceur CI ou systemd d'alerter immédiatement l'astreinte.

## Étape 3 : Tester la détection active en 6 couches (`check-in-use`)

Créez un fichier actif protégé par un fichier compagnon `.part` et sonde-le avec `check-in-use` :

```bash
truncate -s 4K "$LAB/pg-backups/pg_live.dump"
touch "$LAB/pg-backups/pg_live.dump.part"

filecleaner check-in-use \
  --idle-age 10s \
  --stability 500ms \
  "$LAB/pg-backups/pg_live.dump"
echo "Code de sortie (avec .part) : $?"
```

`check-in-use` renvoie le code `10` (`EXIT_IN_USE_BUSY`) parce que la sonde détecte le sidecar `.part` ainsi que l'âge d'inactivité inférieur à `10s`.

Supprimez le fichier `.part`, attendez le dépassement de l'âge d'inactivité, puis testez un verrou POSIX ouvert via un processus Python en arrière-plan :

```bash
rm -f "$LAB/pg-backups/pg_live.dump.part"
touch -d "1 minute ago" "$LAB/pg-backups/pg_live.dump"

python3 -c '
import fcntl, time, sys
with open(sys.argv[1], "r+") as f:
    fcntl.flock(f.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    time.sleep(5)
' "$LAB/pg-backups/pg_live.dump" &
LOCK_PID=$!
sleep 0.5

filecleaner check-in-use --json "$LAB/pg-backups/pg_live.dump"
echo "Code de sortie (avec verrou fcntl + proc fd) : $?"
wait "$LOCK_PID"
```

Sous Linux, `check-in-use` détecte simultanément le descripteur ouvert dans `/proc/<pid>/fd` et le verrou consultatif `flock`, retournant le code `10`.

## Étape 4 : Nettoyer le laboratoire

```bash
rm -rf "$LAB"
```

## Prochaine étape

Passez au [Tutoriel 3 : Laboratoire Docker PostgreSQL WAL et S3](03-laboratoire-docker-postgres-wal-s3.md) pour combiner rétention WAL locale et archivage objet vérifié par SHA-256.
