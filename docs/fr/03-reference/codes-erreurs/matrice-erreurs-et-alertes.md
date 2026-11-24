# Matrice des erreurs et alertes

Cette matrice répertorie les variantes d'erreur `FileCleanerError`, les codes de sortie correspondants et les procédures de remédiation d'astreinte.

## Matrice des erreurs `FileCleanerError`

| Variante / Alerte | Code sortie | Cause racine | Guide de résolution |
| :--- | :---: | :--- | :--- |
| `ConfigParse` / `ConfigFormat` / `ConfigValidation` | `2` | Syntaxe TOML/YAML invalide, extension inconnue ou invariant violé | [Schéma de configuration et invariants](../configuration/schema-configuration-et-invariants.md) |
| `InvalidGlob` / `InvalidRegex` / `InvalidDuration` / `InvalidByteSize` | `2` | Motif glob/regex, durée (`30d`) ou taille (`16MiB`) mal formée | [Schéma de configuration et invariants](../configuration/schema-configuration-et-invariants.md) |
| `BackupGuardViolation` / Gel `--fail-on-freeze` | `3` | Nombre de sauvegardes valides inférieur à `min_keep_files` ou absence de fichier sous `require_newer_within` | [Résoudre un gel de sauvegarde ou un échec S3](../../02-guides-pratiques/gestion-incidents/resoudre-gel-sauvegarde-ou-echec-s3.md) |
| `S3Upload` | `1` | Échec HTTP/TLS, identifiants `AWS_*` absents/invalides ou vérification `checksum_sha256` rejetée | [Résoudre un gel de sauvegarde ou un échec S3](../../02-guides-pratiques/gestion-incidents/resoudre-gel-sauvegarde-ou-echec-s3.md) |
| `IntegrityMismatch` | `1` | Empreinte SHA-256 divergente entre source et destination après copie cross-filesystem | [Pipeline fail-closed en 6 phases](../../04-explications/vue-architecture/pipeline-fail-closed-six-phases.md) |
| `HookFailed` / `HookTimedOut` | `1` | Hook pré-suppression terminé avec un code non nul différent de `skip_exit_code` (`42`) ou timeout atteint | [Appliquer les hooks pré-suppression et protections TOCTOU](../../02-guides-pratiques/securite/appliquer-hooks-pre-suppression-et-toctou.md) |
| `Compression` / `Io` | `1` | Droits insuffisants, disque plein ou flux corrompu lors de l'action `compress`/`move` | [Exécuter le démon réactif et la rotation des journaux](../../02-guides-pratiques/exploitation/executer-demon-reactif-et-rotation-journaux.md) |
| `InUseProbe` | `1` | Impossibilité système de lire `/proc/*/fd` ou métadonnées verrou lors d'une sonde | [Commandes CLI et codes de sortie](../cli-et-scripts/commandes-cli-et-codes-sortie.md) |
| `Daemon` | `4` | PID déjà verrouillé, fichier d'état manquant ou délai d'arrêt coopératif dépassé | [Déployer avec Docker et systemd](../../02-guides-pratiques/deploiement/deployer-filecleaner-docker-et-systemd.md) |
| `EXIT_IN_USE_BUSY` (`check-in-use`) | `10` | Au moins un fichier sondé possède un verrou OS, sidecar ou un âge d'inactivité insuffisant | [Appliquer les hooks pré-suppression et protections TOCTOU](../../02-guides-pratiques/securite/appliquer-hooks-pre-suppression-et-toctou.md) |
