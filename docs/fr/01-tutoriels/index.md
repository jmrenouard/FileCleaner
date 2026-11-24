# Tutoriels FileCleaner

Suivez ces trois laboratoires progressifs dans l'ordre pour maîtriser la rétention locale, les garde-fous de sauvegarde et l'archivage S3 vérifié par empreinte cryptographique sans risque pour vos données de production.

## Parcours d'apprentissage

| Étape | Tutoriel | Compétences acquises | Prérequis |
| :--- | :--- | :--- | :--- |
| **1** | [Bac à sable de rétention locale](01-bac-a-sable-retention-locale.md) | Valider une configuration TOML, inspecter un plan à blanc, exécuter un balayage sécurisé et lire l'audit JSONL | Binaire `filecleaner` et répertoire temporaire |
| **2** | [Laboratoire BackupGuard et détection active](02-laboratoire-backup-guard-et-in-use.md) | Simuler des archives corrompues ou périmées, déclencher le code de sortie `3` et tester `check-in-use` sur des fichiers verrouillés | Tutoriel 1 terminé et Python 3 ou `flock` |
| **3** | [Laboratoire Docker PostgreSQL WAL et S3](03-laboratoire-docker-postgres-wal-s3.md) | Exécuter le laboratoire de bout en bout PostgreSQL + RustFS S3, vérifier les objets distants et nettoyer la pile Docker | Docker Compose et dépôt cloné |

Pour une prise en main rapide sur une seule page, consultez également le [HOWTO 5 minutes](../HOWTO.md). Lorsque vous passez au déploiement en production, basculez vers les [Guides pratiques](../02-guides-pratiques/index.md).
