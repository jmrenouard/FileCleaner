# Référence FileCleaner

Consultez cette section pour vérifier la syntaxe exacte des fichiers de configuration TOML/YAML, les options de ligne de commande, les codes de sortie, les invariants et la matrice d'erreurs.

## Tables de référence

| Document | Contenu couvert | Accès |
| :--- | :--- | :--- |
| **Schéma de configuration et invariants** | Clés globales, `[daemon]`, `[audit]`, `[s3]`, `[[rules]]`, `[rules.backup_guard]`, `[rules.in_use]`, `[rules.pre_delete_hook]` et règles de validation | [Schéma et invariants](configuration/schema-configuration-et-invariants.md) |
| **Commandes CLI et codes de sortie** | Sous-commandes `run`, `plan`, `check-in-use`, `validate`, `init`, `daemon`, options et codes `0`, `1`, `2`, `3`, `4`, `10` | [Commandes CLI et codes de sortie](cli-et-scripts/commandes-cli-et-codes-sortie.md) |
| **Matrice des erreurs et alertes** | Variantes `FileCleanerError`, événements JSONL critiques et actions de remédiation associées | [Matrice des erreurs et alertes](codes-erreurs/matrice-erreurs-et-alertes.md) |

## Manuels complets associés

- [Guide utilisateur complet](../GUIDE_UTILISATEUR.md)
- [Spécifications formelles](../SPECIFICATIONS.md)
