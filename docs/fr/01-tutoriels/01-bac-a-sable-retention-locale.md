# Tutoriel 1 : Bac à sable de rétention locale

Ce premier laboratoire vous apprend à créer un répertoire temporaire isolé, valider une politique TOML, simuler un plan d'éviction en lecture seule puis exécuter votre premier balayage réel avec journalisation d'audit JSONL.

## Ce que vous allez construire

Un bac à sable reproductible sous `/tmp` contenant cinq fichiers factices d'âges distincts, gérés par une règle conservant uniquement les 3 fichiers les plus récents tout en archivant les plus anciens vers un sous-répertoire local.

## Prérequis et installation

Installez la dernière version publiée (`v1.0.1`) ou compilez la branche `main` (`1.1.0`) avec Cargo :

```bash
# Installer depuis le tag de release v1.0.1
cargo install --git https://github.com/jmrenouard/FileCleaner --tag v1.0.1

# Ou compiler la branche de développement main (1.1.0)
cargo install --git https://github.com/jmrenouard/FileCleaner --branch main
```

Vérifiez l'installation :

```bash
filecleaner --version
```

Sortie attendue (avec la branche `main`) :

```text
filecleaner 1.1.0
```

## Étape 1 : Créer le bac à sable et les fichiers horodatés

Créez un répertoire temporaire isolé ainsi que cinq fichiers factices datés sur plusieurs jours :

```bash
SANDBOX=$(mktemp -d)
mkdir -p "$SANDBOX/backups" "$SANDBOX/archive" "$SANDBOX/trash"

for day in 1 2 3 4 5; do
  truncate -s 2K "$SANDBOX/backups/app_day${day}.dump"
done
```

Appliquez des horodatages progressifs (les commandes ci-dessous fonctionnent sous Linux GNU ; sur macOS/BSD, remplacez `date -d` par `date -v-${day}d +"%Y%m%d%H%M"`) :

```bash
# Linux GNU
for day in 1 2 3 4 5; do
  touch -d "$day days ago" "$SANDBOX/backups/app_day${day}.dump"
done
```

> [!NOTE]
> Sur macOS ou FreeBSD, utilisez `touch -t "$(date -v-${day}d +"%Y%m%d%H%M")" "$SANDBOX/backups/app_day${day}.dump"`.

## Étape 2 : Rédiger et valider la configuration TOML

Créez `$SANDBOX/policy.toml` avec `dry_run = true` par défaut :

```toml
dry_run = true
global_trash_dir = "/tmp/filecleaner-sandbox-trash"

[audit]
log_path = "/tmp/filecleaner-sandbox-audit.jsonl"
jsonl = true

[[rules]]
name = "sandbox-retention"
path = "/tmp/SANDBOX_PATH/backups"
enabled = true
include_globs = ["*.dump"]
min_file_size = "1KiB"
time_basis = "modified"
max_files = 3
combine_mode = "any"
eviction_order = "oldest_first"
actions = ["compress", "move"]
compression = "gzip"
compress_remove_source = true
move_to = "/tmp/SANDBOX_PATH/archive"
```

Remplacez `/tmp/SANDBOX_PATH` par le chemin réel de `$SANDBOX`, puis exécutez la validation stricte :

```bash
sed "s|/tmp/SANDBOX_PATH|$SANDBOX|g" "$SANDBOX/policy.toml" > "$SANDBOX/policy.resolved.toml"
sed -i "s|/tmp/filecleaner-sandbox-trash|$SANDBOX/trash|g" "$SANDBOX/policy.resolved.toml"
sed -i "s|/tmp/filecleaner-sandbox-audit.jsonl|$SANDBOX/audit.jsonl|g" "$SANDBOX/policy.resolved.toml"

filecleaner validate -c "$SANDBOX/policy.resolved.toml"
```

Sortie attendue (code de retour `0`) :

```text
Configuration is valid: 1 rule(s), dry_run=true
```

## Étape 3 : Simuler le plan de balayage sans mutation

Exécutez `filecleaner plan` pour inspecter exactement quels fichiers seront évincés (`app_day4.dump` et `app_day5.dump`) et lesquels restent protégés :

```bash
filecleaner plan -c "$SANDBOX/policy.resolved.toml" --verbose
```

## Étape 4 : Activer l'exécution réelle et vérifier l'audit

Passez `dry_run` à `false` dans le fichier résolu et exécutez un balayage synchrone :

```bash
sed -i 's/^dry_run = true$/dry_run = false/' "$SANDBOX/policy.resolved.toml"
filecleaner run -c "$SANDBOX/policy.resolved.toml"
```

Vérifiez le contenu de `$SANDBOX/backups`, `$SANDBOX/archive` et la trace JSONL dans `$SANDBOX/audit.jsonl` :

```bash
ls -1 "$SANDBOX/backups"
ls -1 "$SANDBOX/archive"
cat "$SANDBOX/audit.jsonl"
```

Vous constaterez que seuls les 3 dumps les plus récents (`app_day1.dump` à `app_day3.dump`) demeurent dans `backups/`, que `app_day4.dump.gz` et `app_day5.dump.gz` ont été archivés avec vérification d'intégrité dans `archive/`, et que chaque décision possède son événement horodaté dans `audit.jsonl`.

## Étape 5 : Nettoyer le bac à sable

```bash
rm -rf "$SANDBOX"
```

## Pour aller plus loin

Enchaînez avec le [Tutoriel 2 : Laboratoire BackupGuard et détection active](02-laboratoire-backup-guard-et-in-use.md) pour tester la protection contre les sauvegardes corrompues ou en cours d'écriture.
