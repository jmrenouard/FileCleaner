# Schéma de configuration et invariants

FileCleaner accepte les formats `.toml`, `.yaml` et `.yml` via `Config::from_file` et impose la validation stricte `Config::validate` avant toute exécution de balayage.

## Paramètres globaux

| Clé | Type | Défaut | Description |
| :--- | :--- | :--- | :--- |
| `dry_run` | `bool` | `false` (`true` via `init`) | Désactive toute mutation disque sur les répertoires cibles |
| `global_trash_dir` | `PathBuf \| null` | `null` | Répertoire de quarantaine par défaut pour l'action `trash` |
| `audit_log` | `PathBuf \| null` | `null` | Chemin raccourci optionnel vers le fichier d'audit JSONL |

## Bloc `[daemon]`

| Clé | Valeurs / Type | Défaut `init` | Description |
| :--- | :--- | :--- | :--- |
| `mode` | `interval` \| `notify` \| `hybrid` | `hybrid` | Stratégie de planification des balayages en démon |
| `interval` | Durée (`> 0`) | `5m` | Période d'exécution (ex. `30s`, `5m`, `1h`) |
| `watch` | `bool` | `true` | Active la surveillance système `notify` |
| `debounce` | Durée | `2s` | Fenêtre de regroupement des événements fichiers |
| `sweep_on_start` | `bool` | `true` | Exécute un premier balayage dès le démarrage du démon |
| `pid_file` | `PathBuf \| null` | — | Emplacement du fichier PID |
| `state_file` | `PathBuf \| null` | — | État JSON temps réel du démon |
| `stop_file` | `PathBuf \| null` | — | Fichier témoin pour arrêt coopératif (`daemon stop`) |

## Bloc `[s3]` et `[rules.s3]`

| Clé | Type | Défaut | Contrainte / Description |
| :--- | :--- | :--- | :--- |
| `bucket` | `string` | `""` | Nom du compartiment S3 cible |
| `region` | `string` | `""` | Région AWS ou S3-compatible (ex. `eu-west-3`) |
| `endpoint` | `string \| null` | `null` | Endpoint personnalisé HTTPS (HTTP autorisé seulement si `allow_insecure_http = true`) |
| `prefix` | `string \| null` | `null` | Préfixe de clé objet (ex. `postgres/wal`) |
| `key_date_subdir` | `string \| null` | `null` | Sous-répertoire `strftime` inséré dans la clé (ex. `%Y/%m`) |
| `force_path_style` | `bool` | `false` | Force l'adressage path-style (utile pour MinIO/RustFS) |
| `overwrite` | `bool` | `false` | Autorise l'écrasement d'une clé distante existante |
| `verify_mode` | `checksum_sha256` \| `readback_sha256` | `checksum_sha256` | Méthode de vérification SHA-256 post-upload |
| `access_key_env` | `string` | `AWS_ACCESS_KEY_ID` | Variable d'environnement pour la clé d'accès |
| `secret_key_env` | `string` | `AWS_SECRET_ACCESS_KEY` | Variable d'environnement pour la clé secrète |
| `session_token_env` | `string \| null` | `AWS_SESSION_TOKEN` | Variable d'environnement optionnelle de jeton STS |
| `timeout` | Durée | `60s` | Délai maximal par requête HTTP |
| `max_retries` | `u32` | `3` | Nombre de réessais sur erreur transitoire 5xx/réseau |
| `multipart_part_size` | Taille (`>= 5MiB`) | `16MiB` | Seuil et taille de fragment multipart S3 |

## Bloc `[[rules]]`, `[rules.backup_guard]`, `[rules.in_use]` et `[rules.pre_delete_hook]`

| Section / Clé | Valeurs / Type | Description |
| :--- | :--- | :--- |
| `name` | `string` (unique, non vide) | Identifiant unique de la règle |
| `path` | `PathBuf` (non vide) | Répertoire racine balayé |
| `enabled` | `bool` | Active ou ignore la règle |
| `recursive` / `max_depth` | `bool` / `usize \| null` | Descente récursive et profondeur maximale |
| `follow_symlinks` | `bool` (`false` par défaut) | Suit ou ignore les liens symboliques |
| `include_globs` / `exclude_globs` | `string[]` | Motifs glob d'inclusion et d'exclusion |
| `include_regex` / `exclude_regex` | `string` | Expressions régulières Rust compilées au chargement |
| `time_basis` | `modified` \| `created` \| `filename_date` | Horodatage de référence pour l'âge et le tri |
| `filename_date_regex` / `filename_date_format` | `string` | Extraction de date dans le nom de fichier |
| `max_files` / `max_age` / `max_total_size` | `usize` / Durée / Taille | Plafonds de rétention |
| `combine_mode` | `any` \| `all` | Union (`any`) ou intersection (`all`) des seuils |
| `eviction_order` | `oldest_first` \| `largest_first` | Ordre d'éviction déterministe |
| `actions` | `rename_by_date`, `compress`, `move`, `copy_truncate`, `upload_s3`, `trash`, `delete` | Pipeline ordonné d'actions |
| `compression` | `gzip` \| `lz4` \| `brotli` \| `zip` | Codec pur-Rust utilisé par `compress` |
| `backup_guard.min_keep_files` | `usize` | Plancher inconditionnel des plus récentes sauvegardes valides |
| `backup_guard.min_valid_backup_bytes` | Taille | Taille minimale pour qualifier une sauvegarde valide |
| `backup_guard.require_newer_within` | Durée \| `null` | Fenêtre de fraîcheur (dead-man's-switch) |
| `backup_guard.freeze_on_empty_valid` | `bool` | Gèle toute suppression si zéro sauvegarde valide existe |
| `in_use.check_sidecar_locks` / `sidecar_suffixes` | `bool` / `string[]` | Détection des verrous compagnons (`.lock`, `.part`, `.tmp`, `.writing`) |
| `in_use.check_os_locks` / `check_proc_fd` | `bool` | Verrous OS (`flock` / `share_mode(0)`) et `/proc/*/fd` |
| `in_use.min_idle_age` / `stability_window` | Durée | Âge d'inactivité et fenêtre de stabilité `(size, mtime)` |
| `pre_delete_hook.command` / `args` / `timeout` | `string` / `string[]` / Durée | Exécutable synchrone sans shell ; placeholders `{path}`, `{name}`, `{size}`, `{mtime_iso}`, `{rule}` |
| `pre_delete_hook.on_failure` / `skip_exit_code` / `run_in_dry_run` | `abort_file` \| `abort_rule` \| `warn_and_continue` / `i32` (`42`) / `bool` | Politique de défaillance et code d'exclusion bénin |

## Invariants de validation (`Config::validate`)

| Code d'invariant | Condition exigée | Erreur déclenchée (Code `2`) |
| :--- | :--- | :--- |
| **Règles non vides** | Au moins une entrée `[[rules]]` définie | `configuration must define at least one rule` |
| **Noms uniques** | `rule.name.trim()` non vide et unique | `duplicate rule name` ou `rule name must not be empty` |
| **Actions présentes** | `rule.actions` non vide et `rule.path` non vide | `must specify at least one action step` |
| **Démon positif** | `daemon.interval > 0` | `daemon.interval must be greater than zero` |
| **Multipart S3** | `s3.multipart_part_size >= 5MiB` | Erreur de validation S3 |
| **Absence de secrets** | Clés S3 uniquement via `access_key_env` / `secret_key_env` | Rejet de toute configuration non conforme au schéma |
| **Destinataires valides** | `move` exige `move_to`, `trash` exige `trash_dir` ou `global_trash_dir` | Erreur de validation d'action |

Voir aussi : [Guide utilisateur](../../GUIDE_UTILISATEUR.md) et [Configurer la rétention et l'archivage S3](../../02-guides-pratiques/exploitation/configurer-retention-et-archivage-s3.md).
