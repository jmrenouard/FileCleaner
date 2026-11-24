# Appliquer les hooks pré-suppression et protections TOCTOU

Suivez cette procédure pour exécuter un hook pré-suppression sans shell et activer la détection TOCTOU.

## Prérequis

- Disposez d'un script exécutable ou binaire local avec droits d'exécution (`chmod 750`).
- Vérifiez que le système de fichiers prend en charge les horodatages `mtime` fiables.

## Configurer le hook pré-suppression sans shell

1. Créez un script de validation `/usr/local/libexec/fc-precheck.sh` qui renvoie `0` pour autoriser ou `42` pour ignorer :

```bash
#!/usr/bin/env bash
set -euo pipefail
TARGET_PATH="${1:?missing path}"
if [ -f "${TARGET_PATH}.hold" ]; then
  exit 42
fi
exit 0
```

> [!NOTE]
> FileCleaner invoque `command` et `args` sans shell (`sh -c`). Les caractères spéciaux dans un nom de fichier ne provoquent aucune injection de commande.

2. Ajoutez les blocs `[rules.pre_delete_hook]` et `[rules.in_use]` à votre configuration TOML :

```toml
dry_run = true

[[rules]]
name = "guarded-spool-clean"
path = "/var/spool/ingest"
enabled = true
follow_symlinks = false
include_globs = ["*.dat"]
time_basis = "modified"
max_age = "7d"
actions = ["delete"]

[rules.in_use]
enabled = true
check_sidecar_locks = true
sidecar_suffixes = [".lock", ".part", ".tmp", ".writing"]
check_os_locks = true
check_proc_fd = true
min_idle_age = "30s"
stability_window = "2s"

[rules.pre_delete_hook]
command = "/usr/local/libexec/fc-precheck.sh"
args = ["{path}"]
timeout = "10s"
on_failure = "abort_file"
skip_exit_code = 42
run_in_dry_run = false
```

3. Vérifiez la validité complète du fichier et les droits du script :

```bash
chmod 0750 /usr/local/libexec/fc-precheck.sh
filecleaner validate -c /etc/filecleaner/spool.toml
```

## Validation

Créez un fichier témoin `.hold`, puis testez le détecteur actif et le hook :

```bash
filecleaner check-in-use --idle-age 30s --stability 2s /var/spool/ingest/sample.dat
filecleaner run -c /etc/filecleaner/spool.toml --json
```

Vérifiez que `skipped_hook` incrémente dans le résumé JSON lorsque `.hold` existe et que `follow_symlinks = false` reste actif.

## Retour arrière

Passez `dry_run = true` ou retirez la section `[rules.pre_delete_hook]` dans `/etc/filecleaner/spool.toml`. Passez ensuite `on_failure = "abort_rule"` pour arrêter immédiatement toute règle en cas d'échec du hook.

## Références associées

- [Calcul de rétention et défenses TOCTOU](../../04-explications/analyses-internes/calcul-retention-et-defenses-toctou.md)
- [Commandes CLI et codes de sortie](../../03-reference/cli-et-scripts/commandes-cli-et-codes-sortie.md)
