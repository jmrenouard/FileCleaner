# Commandes CLI et codes de sortie

Le binaire `filecleaner` expose six sous-commandes principales pour la validation, la planification à blanc, l'exécution synchrone, la détection active et le contrôle du démon.

## Options globales

| Option | Rôle |
| :--- | :--- |
| `-c, --config <PATH>` | Chemin vers le fichier `.toml`, `.yaml` ou `.yml` |
| `--dry-run` | Simule le balayage sans modifier les répertoires surveillés |
| `--json` | Émet un rapport structuré JSON sur la sortie standard |
| `-v, --verbose` | Affiche le diagnostic détaillé fichier par fichier |
| `-q, --quiet` | Supprime les messages d'information (ignoré si `--json` est actif) |
| `--audit-log <PATH>` | Remplace le chemin de destination du journal JSONL |
| `-h, --help` / `-V, --version` | Affiche l'aide ou la version du binaire |

## Sous-commandes CLI

| Sous-commande | Arguments / Options clés | Usage exact |
| :--- | :--- | :--- |
| `run` (`clean`) | `-c`, `--rule`, `--dry-run`, `--fail-on-freeze`, `--strict-constraints`, `--dir`, `--include`, `--exclude`, `--recursive`, `--max-files`, `--max-age`, `--max-size`, `--min-keep`, `--min-valid-backup-bytes`, `--require-newer-within`, `--freeze-before-date`, `--min-idle-age`, `--rename-by-date`, `--rename-template`, `--compress`, `--move-to`, `--trash-dir`, `--pre-hook` | Exécute un balayage synchrone complet en 6 phases |
| `plan` | Mêmes options de règle et mode ad-hoc que `run` | Force `dry_run = true` et affiche les décisions prévues sans modifier le disque |
| `check-in-use <PATH>...` | `--idle-age`, `--stability`, `--sidecar-ext`, `--no-flock`, `--no-proc-fd`, `--no-windows-share` | Exécute les 6 sondes de détection active sur un ou plusieurs fichiers |
| `validate [PATH]` | `[PATH]` ou `-c <PATH>` | Parse et contrôle la configuration contre tous les invariants |
| `init` | `-o <PATH>`, `--profile <backups\|logs\|spool\|dba-postgres>`, `--format <toml\|yaml>`, `--force` | Génère une configuration initiale sécurisée (`dry_run = true`) |
| `daemon run` | `-c <PATH>`, `--dry-run`, `--json` | Démarre le démon au premier plan (`interval`, `notify` ou `hybrid`) |
| `daemon status` | `-c <PATH>`, `--json` | Contrôle la vivacité du PID et l'état JSON courant du démon |
| `daemon stop` | `-c <PATH>`, `--json` | Déclenche l'arrêt coopératif via `stop_file` et `SIGTERM` (Unix) |
| `daemon service-template` | `--init-system <systemd\|launchd\|windows-task>`, `-c`, `--user`, `-o` | Génère le modèle de service système prêt à déployer |

## Codes de sortie

| Code | Constante source | Signification |
| :---: | :--- | :--- |
| `0` | `EXIT_OK` | Succès complet (`run`, `plan`, `validate`, `init`, `daemon`, ou `check-in-use` libre) |
| `1` | `EXIT_OPERATIONAL_ERROR` | Erreur opérationnelle, I/O, compression, échec hook non ignoré, intégrité SHA-256 ou upload S3 |
| `2` | `EXIT_CONFIG_OR_USAGE_ERROR` | Syntaxe CLI invalide ou violation de schéma / invariant de configuration |
| `3` | `EXIT_STRICT_SAFETY_GATE` | Gel `BackupGuard` déclenché avec `--fail-on-freeze` ou contrainte non satisfaite avec `--strict-constraints` |
| `4` | `EXIT_DAEMON_ERROR` | Erreur de cycle de vie du démon (PID déjà actif, statut absent/périmé, délai d'arrêt dépassé) |
| `10` | `EXIT_IN_USE_BUSY` | Émis par `check-in-use` lorsqu'au moins un fichier inspecté est activement verrouillé ou utilisé |

Voir aussi : [Déployer avec Docker et systemd](../../02-guides-pratiques/deploiement/deployer-filecleaner-docker-et-systemd.md) et [Matrice des erreurs et alertes](../codes-erreurs/matrice-erreurs-et-alertes.md).
