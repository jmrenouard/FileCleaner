# Résoudre un gel de sauvegarde ou un échec S3

Appliquez ce runbook d'astreinte lorsque FileCleaner retourne le code de sortie `3` (gel `BackupGuard`) ou `1` (échec d'archivage S3).

## Prérequis

- Accès en lecture au journal JSONL défini dans `[audit].log_path`.
- Accès au stockage de sauvegarde local et aux variables d'environnement S3.

## Diagnostiquer un gel BackupGuard (code 3)

1. Lisez les derniers événements d'audit pour identifier la règle bloquée :

```bash
grep -E 'backup_guard|freeze' /var/log/filecleaner/audit.jsonl | tail -n 10
```

2. Exécutez de nouveau le plan en mode verbeux pour voir la raison exacte du gel :

```bash
filecleaner plan -c /etc/filecleaner/policy.toml --verbose --fail-on-freeze
```

> [!WARNING]
> Ne désactivez pas `[rules.backup_guard]` pour contourner un gel. Un gel indique une panne de sauvegarde récente ou une archive trop petite.

3. Vérifiez le travail de sauvegarde amont (`pg_basebackup`, `mysqldump` ou script applicatif) et corrigez la taille ou la fraîcheur des archives.
4. Supprimez les archives partielles ou vides inférieures à `min_valid_backup_bytes` après vérification manuelle.
5. Démarrez une sauvegarde complète pour produire une archive saine dans le répertoire cible.

## Diagnostiquer un échec S3 ou d'intégrité (code 1)

1. Vérifiez les erreurs de transfert S3 dans le journal JSONL :

```bash
grep -E 's3_upload|IntegrityMismatch|S3Upload' /var/log/filecleaner/audit.jsonl | tail -n 10
```

2. Vérifiez que les variables `AWS_ACCESS_KEY_ID` et `AWS_SECRET_ACCESS_KEY` existent et restent valides dans l'environnement du service.
3. Vérifiez la joignabilité HTTPS de l'endpoint S3 configuré et la validité du certificat TLS.
4. Vérifiez que les fichiers locaux restent intacts. En fail-closed, `compress` et `delete` arrêtent l'opération avant suppression.

## Validation

Exécutez d'abord un plan strict, puis lancez le balayage complet :

```bash
filecleaner plan -c /etc/filecleaner/policy.toml --fail-on-freeze --strict-constraints
filecleaner run -c /etc/filecleaner/policy.toml --fail-on-freeze --json
```

Vérifiez que les deux commandes retournent le code `0` sans alerte `freeze` dans la sortie JSON.

## Retour arrière

Forcez immédiatement `dry_run = true` dans `/etc/filecleaner/policy.toml` si une anomalie persiste. Débloquez l'espace disque : déplacez des journaux non critiques, mais ne contournez pas `BackupGuard`.

## Références associées

- [Matrice des erreurs et alertes](../../03-reference/codes-erreurs/matrice-erreurs-et-alertes.md)
- [Chaîne fail-closed en 6 phases](../../04-explications/vue-architecture/pipeline-fail-closed-six-phases.md)
