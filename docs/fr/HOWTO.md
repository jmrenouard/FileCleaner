# FileCleaner v1.0.1 — Guide Pratique Simple en 5 Minutes (HOWTO)

> **Éditions linguistiques** : [English (`docs/en/HOWTO.md`)](../en/HOWTO.md) · **Français**  
> **Documents associés** : [README](../../README.fr.md) · [Guide utilisateur](GUIDE_UTILISATEUR.md) · [Exemples & Recettes](EXEMPLES_RECETTES.md) · [Spécifications formelles](SPECIFICATIONS.md)

Prenez **FileCleaner** en main en cinq minutes : simulez sans risque avec `plan`, appliquez une rétention déterministe via des commandes CLI directes ou un fichier `filecleaner.toml` de 10 lignes, protégez vos écritures actives et vos sauvegardes saines, puis automatisez vos passages.

---

## 1. Le réflexe sécurité en 3 étapes : Planifier -> Exécuter -> Automatiser

Ne lancez jamais une règle destructrice à l'aveugle. FileCleaner repose sur un workflow en trois temps :

1. **Prévisualiser (`plan`)** : Simule le scan, la détection d'utilisation active, les garde-fous de sauvegarde et le plan d'éviction sans modifier, déplacer, compresser ni supprimer aucun fichier.
2. **Appliquer (`run`)** : Exécute exactement le même plan déterministe dès que vous avez validé les fichiers conservés et évincés.
3. **Automatiser (`daemon` ou cron)** : Exécute des cycles récurrents ou déclenchés par le système de fichiers avec journalisation d'audit JSONL structurée.

> [!TIP]
> `filecleaner plan --config filecleaner.toml` et `filecleaner run --config filecleaner.toml --dry-run` calculent le même ensemble d'éviction, sans exécuter `pre_delete_hook` sauf si `run_in_dry_run = true`.

---

## 2. Six commandes CLI prêtes à l'emploi (sans fichier de config)

Passez directement `--dir` à `plan` ou `run` (`clean` est un alias de `run`) pour un nettoyage ponctuel sans fichier de configuration. Remplacez `plan` par `run` après vérification.

### 2.1 Conserver uniquement les `N` fichiers les plus récents (`--max-files`)

```bash
# Prévisualisation en conservant uniquement les 10 archives les plus récentes
filecleaner plan --dir /var/backups/app --include "*.tar.gz" --max-files 10

# Exécution réelle après contrôle
filecleaner run --dir /var/backups/app --include "*.tar.gz" --max-files 10
```

### 2.2 Purger les fichiers plus anciens qu'une durée (`--max-age`)

```bash
filecleaner plan --dir /var/log/app --include "*.log" --recursive --max-age 14d
```

### 2.3 Plafonner l'espace disque total d'un dossier (`--max-size`)

```bash
filecleaner plan --dir /var/cache/builds --max-size 2GiB
```

### 2.4 Mettre en quarantaine dans une corbeille (`--trash-dir`)

```bash
filecleaner run --dir /var/tmp/exports --max-age 7d --trash-dir /var/tmp/filecleaner-trash
```

### 2.5 Renommer par date, compresser et archiver (`--rename-by-date --compress --move-to`)

```bash
filecleaner run \
  --dir /var/log/nginx \
  --include "*.log" \
  --exclude "current/**" \
  --recursive \
  --max-age 1d \
  --rename-by-date \
  --compress gzip \
  --move-to /mnt/cold-logs
```

Remarque : l'option `--rename-template "%Y%m%d_%H%M%S"` active automatiquement `rename_by_date` même sans passer `--rename-by-date`.

### 2.6 Vérifier si un fichier est en cours d'écriture (`check-in-use`)

```bash
# Code 0 si libre ; code 10 si un verrou, un écrivain actif ou un délai de garde déclenche
filecleaner check-in-use /var/spool/incoming/upload.parquet --idle-age 30s --stability 2s
```

---

## 3. Votre premier fichier de configuration en 10 lignes (`filecleaner.toml`)

Pour gérer plusieurs dossiers ou des passages planifiés reproductibles, utilisez un fichier de configuration.

### 3.1 Générer un modèle prêt à l'emploi (`init`)

```bash
# Génère un modèle TOML ou YAML (profils `backups`, `logs` ou `spool`)
filecleaner init --profile backups --format toml --output filecleaner.toml
```

> [!IMPORTANT]
> Les modèles générés par `filecleaner init` démarrent volontairement avec `dry_run = true` afin qu'un appel accidentel à `filecleaner run` ne modifie aucun fichier avant votre relecture de `plan` et le passage à `dry_run = false`.

### 3.2 Configuration TOML minimale en 10 lignes

Enregistrez cette configuration minimale dans `filecleaner.toml` :

```toml
dry_run = true

[[rules]]
name = "downloads-cleanup"
path = "/var/tmp/downloads"
max_age = "30d"
max_files = 50
trash_dir = "/var/tmp/filecleaner-trash"
actions = ["trash"]
```

Validez la syntaxe et les invariants, prévisualisez, puis passez `dry_run = false` pour exécuter :

```bash
filecleaner validate --config filecleaner.toml
filecleaner plan --config filecleaner.toml
# Après vérification, passez dry_run = false dans filecleaner.toml :
filecleaner run --config filecleaner.toml
```

---

## 4. Protéger ses sauvegardes avec `backup_guard`

Une purge classique par âge ou par nombre devient dangereuse si un cron de sauvegarde échoue silencieusement ou écrit des fichiers de `0` octet : le nettoyage finirait par détruire vos dernières copies exploitables. Le bloc `[rules.backup_guard]` gèle automatiquement toute éviction dès que le coffre se dégrade.

### 4.1 Les cinq paramètres essentiels de `backup_guard`

| Clé | Rôle | Valeur conseillée |
| :--- | :--- | :--- |
| `enabled` | Active (`true`) ou désactive (`false`) les garde-fous de sauvegarde | `true` |
| `min_keep_files` | Protège inconditionnellement les $N$ dernières sauvegardes valides | `3` |
| `min_valid_backup_bytes` | Ignore les ébauches vides ou corrompues sous ce seuil pour le comptage | `"1MiB"` |
| `require_newer_within` | Homme mort : gèle la règle si aucune sauvegarde valide récente n'existe | `"36h"` |
| `freeze_on_empty_valid` | Gèle toute éviction lorsqu'aucun fichier n'atteint `min_valid_backup_bytes` | `true` |

### 4.2 Exemple sécurisé pour un coffre PostgreSQL

```toml
dry_run = false

[[rules]]
name = "postgres-backups"
path = "/var/backups/postgres"
include_globs = ["pg_*.sql"]
max_files = 14
actions = ["compress", "move"]
compression = "gzip"
move_to = "/mnt/nas/postgres-archive"

[rules.backup_guard]
enabled = true
min_keep_files = 3
min_valid_backup_bytes = "1MiB"
require_newer_within = "36h"
freeze_on_empty_valid = true
```

Ajoutez `--fail-on-freeze` dans cron ou votre supervision pour remonter le code de sortie `3` dès que les sauvegardes fraîches cessent d'arriver :

```bash
filecleaner run --config filecleaner.toml --fail-on-freeze
```

---

## 5. Bloquer une suppression via `pre_delete_hook` (Bash & PowerShell)

Configurez `[rules.pre_delete_hook]` lorsqu'une vérification externe (accusé d'ingestion, réplication ou contrôle d'intégrité) doit approuver chaque fichier avant que FileCleaner ne le déplace, compresse ou supprime.

### 5.1 Variables d'environnement et codes de sortie

FileCleaner exécute les hooks directement sans passer par un shell (zéro injection shell) et expose les métadonnées du fichier via des variables telles que `$FILECLEANER_PATH`, `$FILECLEANER_NAME`, `$FILECLEANER_SIZE` et `$FILECLEANER_RULE` :

- Code `0` : Validé — exécute le pipeline `actions` de la règle.
- Code `42` (configurable via `skip_exit_code`) : Veto propre par fichier — ignore ce fichier tout en gardant un code retour global `0`.
- Tout autre code non nul (ou timeout) : Géré selon `on_failure` (`"abort_file"` par défaut, `"abort_rule"` pour stopper avec code `1`, ou `"warn_and_continue"`).

```toml
dry_run = false
global_trash_dir = "/var/tmp/filecleaner-trash"

[[rules]]
name = "spool-verified-cleanup"
path = "/var/spool/incoming"
max_age = "1h"
actions = ["trash"]

[rules.pre_delete_hook]
command = "/usr/local/bin/verify_receipt.sh"
args = ["{path}", "{rule}"]
timeout = "10s"
skip_exit_code = 42
on_failure = "abort_file"
```

### 5.2 Script Bash Linux / macOS (`verify_receipt.sh`)

```bash
#!/usr/bin/env sh
# Autorise la mise en corbeille uniquement si <file>.ok existe
if [ -f "$FILECLEANER_PATH.ok" ]; then
  exit 0
fi
exit 42
```

### 5.3 Script PowerShell Windows (`Verify-Receipt.ps1`)

Indiquez `command = "powershell.exe"` et `args = ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", "C:\\Scripts\\Verify-Receipt.ps1"]` dans le TOML :

```powershell
# Autorise la mise en corbeille uniquement si <file>.ok existe
if (Test-Path "$env:FILECLEANER_PATH.ok") {
    exit 0
}
exit 42
```

---

## 6. Automatiser avec Cron ou le démon intégré

Choisissez entre une tâche planifiée OS simple ou le démon réactif intégré à FileCleaner.

### 6.1 Option A : Cron ou Planificateur de tâches simple

```bash
# Exécution toutes les 15 minutes avec journalisation structurée JSONL
*/15 * * * * /usr/local/bin/filecleaner run --config /etc/filecleaner/filecleaner.toml --audit-log /var/log/filecleaner/audit.jsonl
```

### 6.2 Option B : Démon réactif intégré (`daemon`)

```bash
# Démarrer au premier plan (mode `hybrid`, `notify` ou `interval` sous `[daemon]`)
filecleaner daemon run --config /etc/filecleaner/filecleaner.toml

# Vérifier le PID et l'état (`exit 0` actif, `exit 4` arrêté)
filecleaner daemon status --config /etc/filecleaner/filecleaner.toml

# Demander un arrêt propre sans interrompre une action en cours
filecleaner daemon stop --config /etc/filecleaner/filecleaner.toml

# Générer un service natif (`systemd`, `launchd` ou `windows-task`)
filecleaner daemon service-template --init-system systemd --config /etc/filecleaner/filecleaner.toml
```

---

## 7. Aide-mémoire : Unités, Codes de sortie & Dépannage (`-v`)

### 7.1 Formats de durées et de tailles

- **Durées** : Nombre entier en secondes (`60`) ou combinaisons avec `ms`, `s`, `m`, `h`, `d`, `w` (ex. `"30s"`, `"12h"`, `"1d 12h"`, `"2w"`).
- **Tailles** : Octets bruts (`1048576`), puissances décimales SI de $10^3$ (`KB`, `MB`, `GB`, `TB`, `PB`) ou binaires IEC de $2^{10}$ (`KiB`, `MiB`, `GiB`, `TiB`, `PiB`), décimales acceptées comme `"1.5 GiB"`.

### 7.2 Codes de sortie déterministes

| Code | Signification | Quand il apparaît |
| :--- | :--- | :--- |
| `0` | Succès | Passage terminé normalement (y compris veto propre `skip_exit_code = 42` et contrainte souple). |
| `1` | Erreur opérationnelle | Dossier racine absent/illisible, erreur E/S, collision cible ou hook `abort_rule`. |
| `2` | Erreur config / CLI | Syntaxe TOML/YAML invalide, regex/glob/strftime erroné, option manquante, échec `validate`. |
| `3` | Arrêt politique stricte | Gel backup avec `--fail-on-freeze`, ou plancher/verrou bloquant avec `--strict-constraints`. |
| `4` | Conflit / démon arrêté | Verrou PID déjà actif ou `daemon status` sur un démon arrêté. |
| `10` | Fichier occupé (`check-in-use`) | Au moins une cible correspond à un fichier verrou, descripteur `/proc` ouvert ou délai d'inactivité. |

### 7.3 Dépanner avec `-v` et `--json`

Lorsqu'un fichier est conservé ou ignoré de façon inattendue, relancez une simulation avec sortie détaillée ou JSON :

```bash
# Trace lisible des fichiers conservés, occupés ou candidats
filecleaner -v plan --config filecleaner.toml

# Rapport JSON exploitable par script ou diagnostic CI
filecleaner --json plan --config filecleaner.toml
```

Repérez dans le rapport les décisions `keep_floor` (`min_keep_files`), `skip_in_use` (verrou compagnon, descripteur ouvert ou `min_idle_age`), `skip_hook` (veto exit `42`) ou `freeze` (`backup_guard` a détecté une absence ou un vieillissement des sauvegardes).
