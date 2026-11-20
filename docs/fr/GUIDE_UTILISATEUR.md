<div align="center">
  <img src="../assets/header-banner.svg" alt="FileCleaner — Guide Utilisateur" width="100%" />
</div>

# FileCleaner v1.0.1 — Guide Utilisateur & Manuel d'Exploitation Complet

> **Éditions linguistiques** : [English (`docs/en/USER_GUIDE.md`)](../en/USER_GUIDE.md) · **Français**  
> **Documents associés** : [README](../../README.fr.md) · [HOWTO 5 minutes](HOWTO.md) · [Exemples & Recettes](EXEMPLES_RECETTES.md) · [Spécifications formelles](SPECIFICATIONS.md)

---

## Table des matières

1. [Installation (binaires précompilés & compilation source)](#1-installation-binaires-précompilés--compilation-source)
2. [Référence CLI et codes de sortie](#2-référence-cli-et-codes-de-sortie)
3. [Référence complète de configuration TOML & YAML](#3-référence-complète-de-configuration-toml--yaml)
4. [Garde-fous de sauvegarde et statut `Constrained`](#4-garde-fous-de-sauvegarde-et-statut-constrained)
5. [Matrice de détection des fichiers actifs en 6 couches](#5-matrice-de-détection-des-fichiers-actifs-en-6-couches)
6. [Hooks pré-suppression (`FILECLEANER_*` & code `42`)](#6-hooks-pré-suppression)
7. [Pipeline d'actions de cycle de vie et staging atomique](#7-pipeline-dactions-de-cycle-de-vie-et-staging-atomique)
8. [Journal d'audit JSONL et état JSON temps réel du démon](#8-journal-daudit-jsonl-et-état-json-temps-réel-du-démon)
9. [Mode démon et intégration des services OS (`systemd`, `launchd`, Windows Task Scheduler)](#9-mode-démon-et-intégration-des-services-os)

---

## 1. Installation (binaires précompilés & compilation source)

FileCleaner est distribué sous forme d'exécutable natif autonome avec **zéro dépendance d'exécution** et **zéro liaison C/C++ `-sys`**.

### 1.1 Binaires précompilés (9 cibles officielles)

Téléchargez l'archive adaptée à votre système depuis [GitHub Releases](https://github.com/jmrenouard/FileCleaner/releases), contrôlez son empreinte SHA-256 via `SHA256SUMS.txt`, puis installez `filecleaner` (ou `filecleaner.exe`) dans votre `PATH` :

| OS | Architecture | Cible Rust (Triple) | Archive |
| :--- | :--- | :--- | :--- |
| Linux (glibc) | `x86_64` | `x86_64-unknown-linux-gnu` | `filecleaner-v1.0.1-x86_64-unknown-linux-gnu.tar.gz` |
| Linux (statique `musl`) | `x86_64` | `x86_64-unknown-linux-musl` | `filecleaner-v1.0.1-x86_64-unknown-linux-musl.tar.gz` |
| Linux (glibc) | `ARM64` | `aarch64-unknown-linux-gnu` | `filecleaner-v1.0.1-aarch64-unknown-linux-gnu.tar.gz` |
| Linux (statique `musl`) | `ARM64` | `aarch64-unknown-linux-musl` | `filecleaner-v1.0.1-aarch64-unknown-linux-musl.tar.gz` |
| Linux (hard-float) | `ARMv7` | `armv7-unknown-linux-gnueabihf` | `filecleaner-v1.0.1-armv7-unknown-linux-gnueabihf.tar.gz` |
| Windows (MSVC) | `x86_64` | `x86_64-pc-windows-msvc` | `filecleaner-v1.0.1-x86_64-pc-windows-msvc.zip` |
| Windows (ARM64 MSVC) | `ARM64` | `aarch64-pc-windows-msvc` | `filecleaner-v1.0.1-aarch64-pc-windows-msvc.zip` |
| macOS (Apple Silicon) | `ARM64` | `aarch64-apple-darwin` | `filecleaner-v1.0.1-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `x86_64` | `x86_64-apple-darwin` | `filecleaner-v1.0.1-x86_64-apple-darwin.tar.gz` |

#### Installation & vérification SHA-256 sous Linux / macOS

```bash
VERSION="v1.0.1"
TARGET="x86_64-unknown-linux-musl"   # ou aarch64-apple-darwin, etc.

curl -fLO "https://github.com/jmrenouard/FileCleaner/releases/download/${VERSION}/filecleaner-${VERSION}-${TARGET}.tar.gz"
curl -fLO "https://github.com/jmrenouard/FileCleaner/releases/download/${VERSION}/SHA256SUMS.txt"

sha256sum --ignore-missing -c SHA256SUMS.txt   # macOS : shasum -a 256 --ignore-missing -c SHA256SUMS.txt
tar -xzf "filecleaner-${VERSION}-${TARGET}.tar.gz"
sudo install -m 0755 filecleaner /usr/local/bin/filecleaner
filecleaner --version
```

#### Installation & vérification SHA-256 sous Windows PowerShell

```powershell
$Version = "v1.0.1"
$Target  = "x86_64-pc-windows-msvc"
$Archive = "filecleaner-$Version-$Target.zip"

Invoke-WebRequest -Uri "https://github.com/jmrenouard/FileCleaner/releases/download/$Version/$Archive" -OutFile $Archive
Invoke-WebRequest -Uri "https://github.com/jmrenouard/FileCleaner/releases/download/$Version/SHA256SUMS.txt" -OutFile "SHA256SUMS.txt"

Get-FileHash $Archive -Algorithm SHA256
Expand-Archive -Path $Archive -DestinationPath "$Env:ProgramFiles\FileCleaner" -Force
# Ajoutez "$Env:ProgramFiles\FileCleaner" au PATH système
& "$Env:ProgramFiles\FileCleaner\filecleaner.exe" --version
```

### 1.2 Installation via Cargo (`cargo install` ou sources)

Requiert Rust Édition 2021 (`rustc >= 1.75`). Grâce à l'absence totale de crates C `-sys`, aucun compilateur `gcc`, `cmake` ni en-tête OpenSSL n'est nécessaire :

```bash
# Installation directe depuis GitHub
cargo install --git https://github.com/jmrenouard/FileCleaner --tag v1.0.1 --locked

# Ou compilation locale depuis le dépôt
git clone https://github.com/jmrenouard/FileCleaner.git
cd FileCleaner
cargo build --release --locked
./target/release/filecleaner --help
```

### 1.3 Exécution via Docker Hub (`jmrenouard/filecleaner`)

Les images multiplateformes (`linux/amd64` et `linux/arm64`) s'exécutent sous l'utilisateur non privilégié UID/GID `10001` (`fcuser:fcgroup`) avec les exemples fournis sous `/etc/filecleaner/examples/` :

```bash
# Vérifier la version de l'image
docker run --rm jmrenouard/filecleaner:1.0.1 --version

# Simuler un nettoyage sur un dossier hôte monté dans /data
docker run --rm -v "$(pwd)/data:/data" jmrenouard/filecleaner:1.0.1 plan --dir /data --max-age 14d
```

---

## 2. Référence CLI et codes de sortie

### 2.1 Options globales

Les options globales peuvent être placées avant ou avec les sous-commandes :

| Option | Raccourci | Paramètre | Rôle |
| :--- | :--- | :--- | :--- |
| `--config` | `-c` | `<PATH>` | Chemin du fichier de configuration `.toml`, `.yaml` ou `.yml`. |
| `--dry-run` | — | *(interrupteur)* | Force la simulation globale : interdit toute écriture, renommage, compression, déplacement, troncature ou suppression. |
| `--json` | — | *(interrupteur)* | Produit une sortie JSON structurée sur la sortie standard. |
| `--verbose` | `-v` | *(interrupteur)* | Augmente le détail diagnostique (motifs d'exclusion, détails de sondes). |
| `--quiet` | `-q` | *(interrupteur)* | Supprime les résumés non essentiels sur `stdout` (les erreurs restent émises sur `stderr`). |
| `--audit-log` | — | `<PATH>` | Surcharge `[audit].log_path` / `audit_log` depuis la ligne de commande. |

### 2.2 Sous-commandes CLI

#### `filecleaner init`

Génère un modèle de configuration commenté avec des réglages sûrs (`dry_run = true`, `backup_guard.min_keep_files = 3`).

| Option | Défaut | Description |
| :--- | :--- | :--- |
| `-o, --output <PATH>` | `./filecleaner.toml` (ou `.yaml`) | Chemin cible écrit atomiquement via `.fc-tmp`. |
| `--profile <PROFILE>` | `backups` | Profil de modèle de départ : `backups`, `logs` ou `spool`. |
| `--format <FMT>` | Auto / `toml` | Format souhaité : `toml` ou `yaml` (déduit de `--output` si omis). |
| `--force` | `false` | Écrase un fichier existant à l'emplacement `--output`. |

```bash
filecleaner init --output /etc/filecleaner/filecleaner.toml --profile backups --format toml
filecleaner init --output /etc/filecleaner/filecleaner.yaml --profile logs --format yaml --force
```

#### `filecleaner validate [PATH]`

Analyse `.toml`/`.yaml`/`.yml`, contrôle les unités d'octets et de durées, compile les globs, regex et formats `strftime`, puis vérifie les 12 invariants `INV-CFG-*`. N'accède jamais au contenu des répertoires surveillés.

| Option | Description |
| :--- | :--- |
| `<PATH>` / `-c, --config <PATH>` | Fichier de configuration à valider (**obligatoire** en argument positionnel `<PATH>` ou via `--config`). |
| `--json` | Retourne le verdict structuré en JSON (nombre de règles valides, avertissements comme `CopyTruncate`, ou détail d'erreur). |

```bash
filecleaner validate /etc/filecleaner/filecleaner.toml
filecleaner validate -c /etc/filecleaner/filecleaner.yaml --json
```

#### `filecleaner plan`

Exécute les phases 1 à 4 (`Scan -> InUseProbe -> BackupGuard -> RetentionEval`) en lecture seule stricte (`dry_run` forcé à `true`). Accepte soit `--config <PATH>`, soit le mode ad-hoc `--dir <DIR>`. Produit le `SweepPlan` exactement identique à celui qu'exécuterait `run` sur le même instantané disque. Les hooks pré-suppression sont simulés sauf si `hook.run_in_dry_run = true`.

| Option | Description |
| :--- | :--- |
| `-c, --config <PATH>` | Fichier de configuration (mutuellement exclusif avec `--dir`). |
| `--rule <NAME>` | Restreint le plan à une ou plusieurs règles nommées (répétable). |
| `--fail-on-freeze` | Retourne le code `3` si au moins une règle passe en gel `BackupGuard`. |
| `--strict-constraints` | Retourne le code `3` si au moins une règle termine au statut `Constrained`. |
| `--json` | Exporte le rapport complet et les décisions `SweepPlan` en JSON. |

```bash
filecleaner plan -c /etc/filecleaner/filecleaner.toml --rule pg-backups
filecleaner plan -c /etc/filecleaner/filecleaner.toml --json --fail-on-freeze --strict-constraints
filecleaner plan --dir /var/backups/postgres --include "*.sql" --max-age 14d --min-keep 3 --compress gzip
```

#### `filecleaner run` (Alias : `clean`)

Exécute une passe synchrone complète sur les règles actives (6 phases : `Scan -> InUseProbe -> BackupGuard -> RetentionEval -> PreHook -> Execute`) via `--config <PATH>` ou en mode CLI ad-hoc (`--dir`).

| Option | Description |
| :--- | :--- |
| `-c, --config <PATH>` | Fichier de configuration (mutuellement exclusif avec `--dir`). |
| `--rule <NAME>` | Ne traite que la ou les règles spécifiées (répétable). |
| `--dry-run` | Simule à l'identique de `plan` sans toucher au disque. |
| `--fail-on-freeze` | Convertit le code de sortie `0` en `3` lorsqu'une règle gèle par défaut de sauvegarde fraîche. |
| `--strict-constraints` | Convertit le code de sortie `0` en `3` lorsqu'une règle achève la passe en état `Constrained`. |
| `--json` | Émet le rapport récapitulatif de la passe en JSON. |

**Options du mode Ad-Hoc (pour `run` / `clean` / `plan` sans `--config`)** :

| Option | Description |
| :--- | :--- |
| `--dir <DIR>` | Répertoire cible à analyser sans fichier de configuration (mutuellement exclusif avec `--config`). |
| `--include <GLOB>` / `--exclude <GLOB>` | Filtres glob d'inclusion/exclusion (répétables). |
| `--recursive` | Parcourt les sous-répertoires récursivement. |
| `--max-files <N>` / `--max-age <DUR>` / `--max-size <BYTES>` | Plafonds de rétention en mode ad-hoc. |
| `--min-keep <N>` / `--min-valid-backup-bytes <BYTES>` | Protège les `N` sauvegardes les plus récentes dépassant la taille minimale (`backup_guard`). |
| `--require-newer-within <DUR>` / `--freeze-before-date <TS>` | Fenêtre homme-mort de fraîcheur ou borne UTC (`RFC 3339` ou `YYYY-MM-DD`). |
| `--min-idle-age <DUR>` | Délai d'inactivité minimal depuis `mtime` avant toute action. |
| `--rename-by-date` / `--rename-template <STRFTIME>` | Ajoute l'étape `RenameByDate` en tête de pipeline avec modèle `strftime` optionnel. |
| `--compress <gzip\|lz4\|brotli\|zip>` | Compresse les candidats évincés avec un codec pure-Rust. |
| `--move-to <DIR>` / `--trash-dir <DIR>` | Répertoire d'archivage ou de quarantaine (mutuellement exclusifs en mode ad-hoc). |
| `--pre-hook <COMMAND>` | Commande synchrone pré-suppression (`timeout = 30s`, `skip_exit_code = 42`). |

```bash
filecleaner run -c /etc/filecleaner/filecleaner.toml
filecleaner clean -c /etc/filecleaner/filecleaner.toml --dry-run --rule app-logs
filecleaner run --dir /var/tmp/builds --recursive --max-age 48h --trash-dir /var/tmp/quarantine
```

#### `filecleaner check-in-use <PATH...>`

Soumet un ou plusieurs fichiers à la matrice de détection active en 6 couches sans les modifier. Retourne `0` uniquement si **tous** les fichiers sont sûrs ; retourne `10` dès qu'un fichier est occupé.

| Option | Défaut | Description |
| :--- | :--- | :--- |
| `<PATH...>` | **Obligatoire** | Un ou plusieurs chemins de fichiers cibles à sonder. |
| `--idle-age <DUR>` | Aucun | Exige `now - mtime >= <DUR>` (ex. `15s`, `2m`). |
| `--stability <DUR>` | Aucun | Fenêtre de stabilité en deux points `(size, mtime)` (ex. `2s`, `500ms`). |
| `--sidecar-ext <EXT>` | `.lock,.lck,.part,.tmp,.processing` | Surcharge la liste des suffixes annexes (répétable). |
| `--no-flock` | `false` | Désactive la sonde Unix `flock(LOCK_EX \| LOCK_NB)`. |
| `--no-proc-fd` | `false` | Désactive l'analyse Linux `/proc/*/fd`. |
| `--no-windows-share` | `false` | Désactive la sonde Windows `share_mode(0)`. |
| `--json` | `false` | Émet `{ all_safe, busy_count, reports }` en JSON. |

```bash
# Code 0 si sûr, code 10 si occupé
if filecleaner check-in-use /var/spool/etl/batch_094.parquet --idle-age 30s --stability 3s; then
  echo "Fichier stabilisé, prêt pour l'ingestion"
else
  rc=$?
  echo "Fichier occupé ou erreur de sonde (code=$rc)"
fi
```

#### `filecleaner daemon` (Sous-commandes)

| Commande | Options | Comportement |
| :--- | :--- | :--- |
| `daemon run` | `-c <PATH>`, `--mode interval\|notify\|hybrid`, `--max-sweeps <N>`, `--fail-on-freeze`, `--strict-constraints`, `--dry-run` | Lance la boucle du démon au premier plan ; gère `pid_file`, `state_file`, `stop_file`, `SIGINT`/`SIGTERM` et `--max-sweeps`. |
| `daemon status` | `-c <PATH>`, `--pid-file <PATH>`, `--state-file <PATH>`, `--json` | Vérifie la vivacité du PID dans `pid_file` et lit les métriques de `state_file`. Retourne `0` si actif, `4` si arrêté ou obsolète. |
| `daemon stop` | `-c <PATH>`, `--pid-file <PATH>`, `--stop-file <PATH>`, `--timeout <DUR>` (`10s` par défaut) | Écrit `stop_file` (et envoie `SIGTERM` sous Unix), puis attend jusqu'à `--timeout` l'arrêt propre (`0` si réussi, `4` si expiré). |
| `daemon service-template` | `--init-system systemd\|launchd\|windows-task` (alias `--target`), `-c <PATH>`, `--user <USER>`, `-o/--output <PATH>` | Génère un modèle de service OS prêt à déployer sur `stdout` ou vers `--output`. |

### 2.3 Codes de sortie déterministes

| Code de sortie | Signification et contrat d'automatisation |
| :---: | :--- |
| **`0`** | Succès (`run`, `plan`, `validate`, `init`, arrêt propre `daemon run`, `daemon status` actif, `daemon stop` achevé), **ou** `check-in-use` confirme que le fichier est **SÛR (inactif)**. Les sauts volontaires de hook (`skip_exit_code = 42`) et les règles `Frozen`/`Constrained` non strictes retournent également `0`. |
| **`1`** | Erreur d'I/O opérationnelle, divergence `IntegrityMismatch` lors d'un `Move` inter-FS, échec de hook `AbortFile`/`AbortRule`, erreur de compression ou `TimestampUnsupported` en passe réelle. |
| **`2`** | Erreur de syntaxe CLI, extension de configuration non supportée (`.toml`/`.yaml`/`.yml` requis) ou violation d'invariant `INV-CFG-*`. |
| **`3`** | Garde de sûreté déclenchée en mode strict : gel `BackupGuard` sous `--fail-on-freeze`, **ou** règle `Constrained` sous `--strict-constraints`. Idéal pour déclencher une alerte de supervision. |
| **`4`** | Erreur de cycle de vie du démon (`daemon run` bloqué par un `pid_file` actif, `daemon status` arrêté/obsolète, délai dépassé sur `daemon stop`). |
| **`10`** | `check-in-use <PATH>` a détecté que le fichier est **EN COURS D'UTILISATION / OCCUPÉ** (ou erreur de sonde *fail-closed*). Distinct de `1`/`2` pour éviter toute confusion avec une erreur de syntaxe CLI. |

---

## 3. Référence complète de configuration TOML & YAML

`Config::from_file` détermine le format exclusivement à partir de l'extension en minuscules : `.toml` utilise le parseur `toml`, `.yaml`/`.yml` utilise `serde_yaml_ng`. Toute autre extension échoue immédiatement avec `ConfigFormat`.

Toutes les énumérations Rust se sérialisent en `snake_case` par défaut tout en acceptant des alias `serde` en `PascalCase` (par exemple `"oldest_first"` ou `"OldestFirst"`).

### 3.1 Clés racines de `Config`

| Clé | Type | Défaut | Description |
| :--- | :--- | :--- | :--- |
| `dry_run` | `bool` | `false` | Interrupteur global de simulation empêchant toute mutation disque sur toutes les règles. |
| `audit_log` | `Option<PathBuf>` | `None` | Raccourci racine pour le chemin du journal JSONL (`[audit].log_path` est prioritaire si les deux sont définis). |
| `global_trash_dir` | `Option<PathBuf>` | `None` | Répertoire de quarantaine par défaut utilisé par `actions = [..., "trash"]` sans `trash_dir` local. |
| `daemon` | `DaemonConfig` | Défaut `DaemonConfig` | Mode de planification, période, anti-rebond *watcher* et chemins PID/état/arrêt. |
| `audit` | `AuditConfig` | Défaut `AuditConfig` | Journalisation d'audit structurée (`log_path`, `jsonl` à `true` par défaut). |
| `rules` | `Vec<RuleConfig>` | **Obligatoire (non vide)** | Liste ordonnée des règles indépendantes de rétention. |

### 3.2 `[daemon]` (`DaemonConfig`)

| Clé | Type | Défaut | Alias | Description |
| :--- | :--- | :--- | :--- | :--- |
| `mode` | `DaemonMode` | `"hybrid"` | `Interval`, `Notify`, `Hybrid` | `"interval"` (période fixe), `"notify"` (observateur FS OS) ou `"hybrid"` (observateur + repli périodique). |
| `interval` | `Duration` | `"60s"` | `poll_interval` | Période des passes (`> 0`). En mode `hybrid`, garantit l'évaluation de `max_age` et de fraîcheur même sans événement FS. |
| `watch` | `bool` | `false` | — | Active explicitement `notify` ; `mode = "notify"` ou `"hybrid"` active automatiquement la surveillance. |
| `debounce` | `Duration` | `"2s"` | — | Délai de regroupement des salves d'événements FS avant de lancer une passe. |
| `sweep_on_start` | `bool` | `true` | — | Lance une passe immédiate au démarrage du démon avant d'entrer en attente. |
| `pid_file` | `Option<PathBuf>` | `None` | — | Chemin atomique du fichier verrou PID contrôlé au lancement et supprimé à l'arrêt propre. |
| `state_file` | `Option<PathBuf>` | `None` | — | Chemin atomique du fichier JSON d'état temps réel du démon. |
| `stop_file` | `Option<PathBuf>` | `None` | — | Fichier déclencheur d'arrêt coopératif scruté toutes les $\le 250\text{ ms}$. |

### 3.3 `[audit]` (`AuditConfig`)

| Clé | Type | Défaut | Description |
| :--- | :--- | :--- | :--- |
| `log_path` | `Option<PathBuf>` | `None` | Fichier dans lequel les événements JSONL sont ajoutés et vidés sur disque de façon synchrone. |
| `jsonl` | `bool` | `true` | Active la sérialisation JSON Lines lorsque `log_path` / `audit_log` (ou `--audit-log`) est défini. |

### 3.4 `[[rules]]` (`RuleConfig`)

| Clé | Type | Défaut | Description |
| :--- | :--- | :--- | :--- |
| `name` | `String` | **Obligatoire** | Identifiant unique non vide dans `rules`. |
| `path` | `PathBuf` | **Obligatoire** | Répertoire racine surveillé. |
| `enabled` | `bool` | `true` | Passer à `false` pour désactiver temporairement la règle. |
| `recursive` | `bool` | `false` | Parcourir les sous-répertoires si `true`. |
| `max_depth` | `Option<usize>` | `None` | Profondeur maximale de récursion (ignoré sauf si `recursive = true`). |
| `follow_symlinks` | `bool` | `false` | Suivre les liens symboliques. **Conserver `false`** sauf besoin explicite. |
| `include_globs` | `Vec<String>` | `[]` | Motifs `globset` appliqués aux chemins relatifs normalisés avec `/` (vide = tout inclure). |
| `exclude_globs` | `Vec<String>` | `[]` | Motifs glob d'exclusion ; prioritaires sur `include_globs` et `include_regex`. |
| `include_regex` | `Option<String>` | `None` | Expression régulière Rust d'inclusion sur chemins relatifs normalisés. |
| `exclude_regex` | `Option<String>` | `None` | Expression régulière Rust d'exclusion ; toujours prioritaire sur les inclusions. |
| `min_file_size` | `Option<Bytes>` | `None` | Borne inférieure de taille (`size >= min_file_size`) écartant les petits fichiers hors périmètre. |
| `max_file_size` | `Option<Bytes>` | `None` | Borne supérieure de taille (`size <= max_file_size`) ; impose `min_file_size <= max_file_size`. |
| `time_basis` | `TimeBasis` | `"modified"` | Source d'horodatage : `"modified"`, `"created"` ou `"filename_date"`. |
| `filename_date_regex` | `Option<String>` | `None` | Regex d'extraction pour `time_basis = "filename_date"` (préfère le groupe nommé `(?<date>...)`, puis groupe `1`, puis `0`). |
| `filename_date_format` | `Option<String>` | `None` | Format `chrono` `strftime` (ex. `"%Y%m%d_%H%M%S"`, `"%Y-%m-%d"`). |
| `max_files` | `Option<usize>` | `None` | Nombre maximal de fichiers candidats conservés (`>= backup_guard.min_keep_files`). |
| `max_age` | `Option<Duration>` | `None` | Âge maximal (`now - t(f) > max_age` rend le fichier éligible à l'éviction). |
| `max_total_size` | `Option<Bytes>` | `None` | Plafond cumulé en octets sur l'ensemble des candidats de la règle. |
| `combine_mode` | `CombineMode` | `"any"` | `"any"` (OU : évince dès qu'un seuil actif est dépassé) ou `"all"` (ET : évince uniquement si tous les seuils actifs sont dépassés simultanément). |
| `eviction_order` | `EvictionOrder` | `"oldest_first"` | `"oldest_first"` ($t(f)$ croissant) ou `"largest_first"` (`size(f)` décroissant, départage au plus ancien). |
| `dry_run` | `bool` | `false` | Force le mode simulation uniquement sur cette règle. |
| `actions` | `Vec<ActionStep>` | `["delete"]` | Étapes ordonnées : `"rename_by_date"`, `"compress"`, `"move"`, `"copy_truncate"`, `"trash"`, `"delete"`. Les actions terminales (`trash`, `delete`) doivent être en fin de liste. |
| `move_to` | `Option<PathBuf>` | `None` | Répertoire cible exigé lorsque `actions` contient `"move"`. |
| `move_date_subdir` | `Option<String>` | `None` | Sous-répertoire `strftime` optionnel sous `move_to` (ex. `"%Y/%m"`). |
| `move_overwrite` | `bool` | `false` | Autorise l'écrasement d'une cible existante lors de `move`. |
| `trash_dir` | `Option<PathBuf>` | `None` | Quarantaine propre à la règle (prioritaire sur `global_trash_dir`). |
| `compression` | `CompressionFormat` | `"gzip"` | Codec pure-Rust pour `"compress"` : `"gzip"`, `"lz4"`, `"brotli"`, `"zip"`. |
| `compression_level` | `Option<u32>` | `None` | Niveau de compression optionnel spécifique au codec. |
| `compress_remove_source` | `bool` | `true` | Supprime la source après réussite atomique de la compression. |
| `compress_overwrite` | `bool` | `false` | Autorise l'écrasement d'une archive compressée déjà présente. |
| `copy_truncate_to` | `Option<PathBuf>` | `None` | Répertoire d'archive pour `"copy_truncate"` (repli sur `move_to`). |
| `rename_template` | `Option<String>` | `None` | Modèle `strftime`/nom pour `"rename_by_date"` (`%Y%m%d_%H%M%S` par défaut). |
| `backup_guard` | `BackupGuardConfig` | Défaut | Plancher de sauvegarde et gel sur vétusté (§4). |
| `in_use` | `InUseConfig` | Défaut | Contrôles de détection active en 6 couches (§5). |
| `pre_delete_hook` | `Option<HookConfig>` | `None` | Alias : `pre_hook`. Commande externe synchrone de veto (§6). |

### 3.5 Unités d'octets et de durées lisibles

**Tailles en octets (`parse_bytes`)** : Accepte des entiers bruts (`1048576`) ou `<nombre>\s*<unité>` (insensible à la casse, décimales acceptées comme `"1.5 GiB"`) :

- **Décimal SI (puissances de $10^3$)** : `KB` (`1 000`), `MB` (`1 000 000`), `GB` (`1 000 000 000`), `TB` ($10^{12}$), `PB` ($10^{15}$).
- **Binaire IEC (puissances de $2^{10}$)** : `K`/`KiB` (`1 024`), `M`/`MiB` (`1 048 576`), `G`/`GiB` (`1 073 741 824`), `T`/`TiB` (`1 099 511 627 776`), `P`/`PiB` ($2^{50}$).

**Durées (`parse_duration_str`)** : Accepte des secondes brutes (`60`) ou des segments composés `<uint><unité>` (`"500ms"`, `"30s"`, `"1d 12h"`, `"2m30s"`, `"2w"`) :

- `ms` ($1\text{ ms}$), `s` ($1{,}000\text{ ms}$), `m` ($60{,}000\text{ ms}$), `h` ($3{,}600{,}000\text{ ms}$), `d` ($24\text{ h}$), `w` ($7\text{ d}$).

### 3.6 Garde-fous de validation statique (`INV-CFG-01` .. `INV-CFG-12`)

`filecleaner validate` impose douze contrôles structurels avant toute exploration disque :

- **Non-vacuité & Unicité** : `rules` non vide, `rule.name` unique et non vide, `rule.path` non vide, `rule.actions` non vide, `daemon.interval > 0`.
- **Cohérence des bornes** : `min_file_size <= max_file_size` ; `max_files >= backup_guard.min_keep_files`.
- **Motifs et formats valides** : Tous les globs (`include_globs`, `exclude_globs`), regex (`include_regex`, `exclude_regex`, `filename_date_regex`) et formats `strftime` (`filename_date_format`, `move_date_subdir`) doivent compiler proprement ; `time_basis = "filename_date"` exige à la fois regex et format.
- **Destinations requises & Terminaison** : `move` exige `move_to` ; `copy_truncate` exige `copy_truncate_to` ou `move_to` ; `trash` exige `trash_dir` ou `global_trash_dir`. Aucune étape ne peut suivre `delete` ou `trash`.
- **Garde d'auto-inclusion récursive (`INV-CFG-09`)** : Si `recursive = true`, `move_to`, `copy_truncate_to` et `trash_dir` **ne peuvent pas** pointer sur ou dans `rule.path` sauf exclusion explicite dans `exclude_globs` ou `exclude_regex`, évitant toute boucle infinie de déplacement-réindexation.

---

## 4. Garde-fous de sauvegarde et statut `Constrained`

`[rules.backup_guard]` empêche un nettoyage automatisé de détruire vos dernières sauvegardes saines lorsqu'un dump amont ou une réplication tombe en panne silencieusement.

### 4.1 Paramètres de `[rules.backup_guard]`

| Champ | Type | Défaut | Alias | Comportement |
| :--- | :--- | :--- | :--- | :--- |
| `enabled` | `bool` | `false` | — | Active les garde-fous de sauvegarde sur la règle. |
| `min_keep_files` | `usize` | `0` | — | Protège inconditionnellement les $N_{\min}$ sauvegardes valides les plus récentes ($P_{\text{floor}}$). |
| `min_valid_backup_bytes` | `Bytes` | `0` | `min_valid_size` | Taille minimale $B_{\min}$ pour appartenir à $\mathcal{F}_{\text{valid}}$. Les fichiers plus petits (ex. dumps ratés de `0` octet) ne comptent jamais dans `min_keep_files` ni dans la fraîcheur. |
| `require_newer_within` | `Option<Duration>` | `None` | `max_newest_age` | **Interrupteur homme-mort** : gèle toute la règle ($E = \emptyset$) si `now - newest_valid_ts > require_newer_within`. |
| `freeze_if_newest_older_than` | `Option<DateTime<Utc>>` | `None` | `min_newest_timestamp` | Seuil absolu RFC 3339 UTC : gèle la règle si la sauvegarde valide la plus récente est antérieure à cette date. |
| `freeze_on_empty_valid` | `bool` | `true` | `freeze_on_empty` | Gèle la règle si aucun fichier ne satisfait `size >= min_valid_backup_bytes`. |

### 4.2 Comptabilisation des fichiers transformés sur place

- **Transformations sur place (`rename_by_date`, `compress`)** : Si le fichier compressé ou renommé reste dans `rule.path`, continue de correspondre à `include_globs`/`exclude_globs` et vérifie `size >= min_valid_backup_bytes`, il **continue de compter** dans `min_keep_files` et dans le calcul de fraîcheur lors des passes suivantes.
- **Transformations avec sortie (`move`, `trash`)** : Dès qu'un artefact est déplacé hors de `rule.path` vers un NAS froid ou la corbeille, il sort de l'ensemble $\mathcal{F}$ de cette règle.

### 4.3 Résolution de conflits de contraintes (`Constrained`)

Si `min_keep_files` ($P_{\text{floor}}$) et/ou des fichiers occupés ($S_{\text{busy}}$) empêchent de redescendre sous `max_total_size` ou `max_files` :

1. **La sûreté l'emporte toujours** : FileCleaner ne viole jamais `min_keep_files` et ne touche jamais à un fichier en cours d'utilisation.
2. **Statut = `Constrained`** : FileCleaner journalise le dépassement (`remaining_files`, `target_max_files`, `remaining_bytes`, `target_max_total_size`, `protected_floor_count`, `busy_skipped_count`) dans la sortie CLI, le journal JSONL et `state_file` (`constrained_rules`).
3. **Code de sortie** : Reste à `0` par défaut (évitant les faux positifs cron sur verrous transitoires) ou passe à `3` avec l'option `--strict-constraints`.

---

## 5. Matrice de détection des fichiers actifs en 6 couches

Lorsque `in_use.enabled = true` (défaut), chaque candidat traverse jusqu'à six sondes à échec sécurisé avant planification d'éviction. Un seul déclenchement ou une erreur de sonde directe place le fichier dans $S_{\text{busy}}$.

### 5.1 Paramètres de `[rules.in_use]`

| Champ | Type | Défaut | Description |
| :--- | :--- | :--- | :--- |
| `enabled` | `bool` | `true` | Interrupteur général des sondes de détection active. |
| `check_sidecar_locks` | `bool` | `true` | Détecte `.lock`, `.lck`, `.part`, `.tmp`, `.processing` en suffixe ou en fichier annexe `<fichier><suffixe>`. |
| `sidecar_suffixes` | `Vec<String>` | `[".lock", ".lck", ".part", ".tmp", ".processing"]` | Liste personnalisable des extensions de verrous et fichiers temporaires. |
| `check_os_locks` | `bool` | `true` | `flock(LOCK_EX \| LOCK_NB)` via `O_RDONLY` sous Unix / `OpenOptionsExt::share_mode(0)` sous Windows. |
| `check_proc_fd` | `bool` | `true` | Parcours Linux `/proc/<pid>/fd` par paire `(st_dev, st_ino)`. |
| `min_idle_age` | `Option<Duration>` | `None` | Délai de refroidissement exigeant `now - mtime >= min_idle_age` (protège aussi tout `mtime > now`). |
| `stability_window` | `Option<Duration>` | `None` | Fenêtre de vérification en deux points sur `(size, mtime)`. |

### 5.2 Spécificités par système et optimisation

- **Linux `/proc/*/fd` et droits d'accès** : Un `EACCES`/`EPERM` non privilégié sur le `/proc/<pid>/fd` d'un autre utilisateur est consigné en note diagnostique (sans bloquer à tort tous les fichiers de l'utilisateur), tandis qu'un descripteur correspondant déclenche `ProcFdOpen { pid }` et que toute erreur d'ouverture/stat directe sur le candidat échoue en sûreté (`ProbeError`).
- **macOS / BSD** : S'appuie sur les fichiers annexes, `flock`, `min_idle_age` et `stability_window` (fortement recommandé sur macOS pour les transferts non verrouillés par `flock`).
- **Windows** : `share_mode(0)` applique un contrôle de partage exclusif au niveau du noyau, détectant même les processus ne posant pas de verrou explicite.
- **Attente groupée (`O(stability_window)`)** : `stability_window` capture l'instantané `p0 = (size, mtime)` sur l'ensemble des survivants des sondes 1 à 5, attend **une seule fois** la durée `stability_window`, puis compare `p1` pour tous les candidats.

---

## 6. Hooks pré-suppression

`[rules.pre_delete_hook]` (alias `pre_hook`) exécute un script ou vérificateur externe avant toute mutation sur un candidat sélectionné.

### 6.1 Garantie de sécurité : zéro injection shell

FileCleaner lance toujours `hook.command` directement via `std::process::Command::new(&hook.command).args(...)` — **jamais** via `sh -c` ni `cmd /C`. Les noms de fichiers contenant espaces, guillemets, `;`, `|` ou `$()` restent des octets littéraux dans `argv`.

### 6.2 Champs de `[rules.pre_delete_hook]` et variables d'environnement

| Champ | Type | Défaut | Description |
| :--- | :--- | :--- | :--- |
| `command` | `String` | **Obligatoire** | Chemin de l'exécutable ou nom du binaire. |
| `args` | `Vec<String>` | `[]` | Arguments supportant `{path}`, `{name}`, `{size}`, `{mtime_iso}`, `{rule}`. |
| `timeout` | `Duration` | `"30s"` | Délai maximal ; tout hook expiré est tué (`kill()` + `wait()`). |
| `on_failure` | `HookFailurePolicy` | `"abort_file"` | `"abort_file"` (saute le fichier, erreur), `"abort_rule"` (stoppe la règle, code `1`), `"warn_and_continue"` (alerte puis continue). |
| `skip_exit_code` | `Option<i32>` | `42` | Code de sortie interprété comme un veto bénin par fichier (code global conservé à `0`). |
| `run_in_dry_run` | `bool` | `false` | Autorise l'exécution du hook même pendant `plan` ou `--dry-run`. |
| `working_dir` | `Option<PathBuf>` | `None` | Répertoire de travail du processus enfant. |
| `env` | `Map<String, String>` | `{}` | Variables statiques additionnelles fusionnées avec `FILECLEANER_*`. |

**Variables d'environnement `FILECLEANER_*` injectées** :

- `FILECLEANER_PATH` : Chemin complet du fichier candidat.
- `FILECLEANER_NAME` : Nom de base du fichier.
- `FILECLEANER_DIR` : Répertoire parent.
- `FILECLEANER_SIZE` : Taille en octets.
- `FILECLEANER_MTIME_ISO` : Horodatage `mtime` au format RFC 3339 UTC.
- `FILECLEANER_RULE` : Nom de la règle (`rule.name`).
- `FILECLEANER_ACTION` : Résumé du pipeline d'actions (ex. `Compress(Gzip) -> Move`).
- `FILECLEANER_DRY_RUN` : `"true"` ou `"false"`.

---

## 7. Pipeline d'actions de cycle de vie et staging atomique

Les étapes de `rule.actions` s'exécutent strictement dans l'ordre en propageant `WorkingTarget` (chemin actif, taille, horodatages d'origine) d'une étape à l'autre (ex. `["rename_by_date", "compress", "move"]`).

| Étape | Comportement et paramètres associés | Contrat d'atomicité et de métadonnées |
| :--- | :--- | :--- |
| `rename_by_date` | Formate l'horodatage avec `rename_template` (`%Y%m%d_%H%M%S` par défaut). | N'écrase jamais une cible existante silencieusement ; préserve `atime`/`mtime` via `std::fs::FileTimes` (`File::set_times`) ; met à jour `WorkingTarget`. |
| `compress` | Codecs pure-Rust : `gzip` (`.gz`), `lz4` (`.lz4`), `brotli` (`.br`), `zip` (`.zip`). Paramètres : `compression`, `compression_level`, `compress_remove_source`, `compress_overwrite`. | Écrit dans `<dest>.fc-tmp...`, vide le flux + `sync_all()`, restaure `atime`/`mtime` via `std::fs::FileTimes`, renomme atomiquement vers `<dest>`, puis supprime la source si `compress_remove_source = true`. |
| `move` | Déplace `WorkingTarget` vers `move_to` (plus sous-dossier `strftime` `move_date_subdir`). Paramètre : `move_overwrite`. | Voie rapide `rename` sur même FS ; en inter-FS (`EXDEV`), copie en flux dans `<dest>.fc-tmp...`, `sync_all()`, **relit `<dest>.fc-tmp...` sur disque pour vérifier le SHA-256**, restaure dates (`std::fs::FileTimes`) et permissions, renomme atomiquement, puis supprime la source. |
| `copy_truncate` | **Activation explicite** (*opt-in*) pour journaux ouverts en ajout continu (`copy_truncate_to` ou `move_to`, `compression` optionnelle). | Archive et vérifie sur `.fc-tmp` **avant** `file.set_len(0)` + `sync_all()` (conserve chemin et inode). Émet un avertissement `validate` concernant la fenêtre entre fin de copie et troncature. |
| `trash` | Place le fichier en quarantaine dans `trash_dir` ou `global_trash_dir`. | Désambiguïse les collisions (`_1`, `_2`, ...) ; vérification SHA-256 appliquée en traversée inter-FS. |
| `delete` | Supprime définitivement `WorkingTarget` (`remove_file`). | Étape terminale (`trash` et `delete` doivent toujours figurer en dernier). |

---

## 8. Journal d'audit JSONL et état JSON temps réel du démon

### 8.1 Journal d'audit JSONL (`[audit].log_path` / `--audit-log`)

Chaque décision, gel, contrainte, exécution de hook, action ou erreur écrit une ligne JSON UTF-8 vidée de façon synchrone sur disque :

```json
{"ts":"2026-11-20T09:05:00.010Z","event":"file_decision","run_id":"run-42180-1","rule":"pg-backups","path":"/var/backups/postgres/pg_20261101_020000.sql","size_bytes":10485760,"resolved_ts":"2026-11-01T02:00:00Z","decision":"acted","in_use_reasons":null,"actions":["Compress(Gzip)","Move"],"dest_path":"/mnt/cold-archive/postgres/2026/11/pg_20261101_020000.sql.gz","sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","dry_run":false,"duration_ms":42,"error":null}
```

Champs clés :

- `event` : `sweep_start` | `sweep_end` | `file_decision` | `backup_freeze` | `rule_constrained` | `hook_exec` | `action_exec` | `error`
- `decision` : `keep_floor` | `keep_retention` | `skip_in_use` | `skip_hook` | `skip_ts_parse` | `freeze` | `would_act` | `acted` | `aborted` | `error`
- `sha256` : Renseigné dès qu'une étape inter-FS `move` ou `copy_truncate` valide l'intégrité binaire.

### 8.2 Fichier d'état JSON temps réel du démon (`[daemon].state_file`)

Écrit atomiquement (`.fc-tmp` + `rename`) lors des transitions `"starting"`, `"idle"`, `"sweeping"`, `"stopping"` et `"stopped"` :

```json
{
  "version": "1.0.0",
  "pid": 42180,
  "status": "idle",
  "watch_mode": "hybrid",
  "started_at": "2026-11-20T09:00:00.000Z",
  "last_sweep_at": "2026-11-20T09:05:00.142Z",
  "next_sweep_at": "2026-11-20T09:10:00.000Z",
  "sweeps_completed": 2,
  "total_files_evaluated": 128,
  "total_files_processed": 14,
  "total_bytes_reclaimed": 536870912,
  "active_freezes": ["pg-backups"],
  "constrained_rules": [],
  "last_error": null
}
```

---

## 9. Mode démon et intégration des services OS

Le démon intégré élimine l'imprécision des tâches cron et associe réactivité instantanée (`notify`) et garanties temporelles (`mode = "hybrid"`).

### 9.1 Service `systemd` (Linux)

Générez le fichier directement via `filecleaner daemon service-template --init-system systemd --config /etc/filecleaner/filecleaner.toml` :

```ini
[Unit]
Description=FileCleaner Deterministic Directory Retention Daemon
Documentation=https://github.com/jmrenouard/FileCleaner
After=local-fs.target network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/filecleaner daemon run --config /etc/filecleaner/filecleaner.toml
ExecStop=/usr/local/bin/filecleaner daemon stop --config /etc/filecleaner/filecleaner.toml --timeout 15s
Restart=on-failure
RestartSec=5s
KillSignal=SIGTERM
TimeoutStopSec=20s
NoNewPrivileges=true
PrivateTmp=false

[Install]
WantedBy=multi-user.target
```

Activation et contrôle :

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now filecleaner
filecleaner daemon status -c /etc/filecleaner/filecleaner.toml --json
```

### 9.2 Agent / Démon `launchd` (macOS)

Générez via `filecleaner daemon service-template --init-system launchd --config /usr/local/etc/filecleaner.toml` :

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.filecleaner.daemon</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/filecleaner</string>
        <string>daemon</string>
        <string>run</string>
        <string>--config</string>
        <string>/usr/local/etc/filecleaner.toml</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/var/log/filecleaner/daemon.stdout.log</string>
    <key>StandardErrorPath</key>
    <string>/var/log/filecleaner/daemon.stderr.log</string>
</dict>
</plist>
```

### 9.3 Planificateur de tâches Windows (Task Scheduler)

Générez un modèle XML via `filecleaner daemon service-template --init-system windows-task --config "C:\ProgramData\FileCleaner\filecleaner.toml" --output filecleaner-task.xml` et enregistrez-le sous PowerShell :

```powershell
Register-ScheduledTask -TaskName "FileCleanerDaemon" `
  -Xml (Get-Content "C:\ProgramData\FileCleaner\filecleaner-task.xml" -Raw)
Start-ScheduledTask -TaskName "FileCleanerDaemon"

# Arrêt coopératif multiplateforme (crée daemon.stop_file et attend la fin propre du fichier en cours)
filecleaner.exe daemon stop --config C:\ProgramData\FileCleaner\filecleaner.toml --timeout 15s
```

### 9.4 Exécution bornée en CI / cron (`--max-sweeps`)

Pour exécuter exactement `N` passes déterministes du démon (validation de l'observateur, du fichier d'état JSON et du nettoyage à l'arrêt) sans boucle infinie :

```bash
filecleaner daemon run -c filecleaner.toml --mode interval --max-sweeps 1
```
