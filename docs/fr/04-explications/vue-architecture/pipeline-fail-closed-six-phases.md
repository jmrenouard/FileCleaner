# Pipeline fail-closed en 6 phases

FileCleaner exécute chaque balayage comme un pipeline synchrone et ordonné en six phases où toute incertitude de métadonnée, de verrouillage ou de vérification distante interrompt la mutation destructive avant qu'elle ne touche le disque.

## Topologie du pipeline

```mermaid
flowchart TD
    S["Phase 1 : Scan déterministe<br/>Globs, regex, follow_symlinks=false"] --> U["Phase 2 : Sondes In-Use en 6 couches<br/>Sidecar, idle_age, stabilité, flock, proc/fd, share_mode"]
    U --> BG["Phase 3 : Garde-fou BackupGuard<br/>min_keep_files et fenêtre require_newer_within"]
    BG -- "Gel déclenché" --> FR["Règle gelée : zéro suppression<br/>Code 3 si --fail-on-freeze"]
    BG -- "Plancher satisfait" --> RET["Phase 4 : Calcul de rétention<br/>max_files, max_age, max_total_size et tri"]
    RET --> HK["Phase 5 : Hook pré-suppression<br/>Exécution directe sans shell, skip=42"]
    HK --> EX["Phase 6 : Pipeline d'actions atomiques<br/>Compress, Upload S3 SHA-256, Move, Trash, Delete"]
    EX -- "Échec vérification" --> AB["Abandon fail-closed<br/>Fichier source préservé sur disque"]
```

## Principes de conception

### Séparation stricte entre filtrage, protection et mutation

Un nettoyeur classique mélange souvent la recherche de fichiers et la suppression immédiate dans un même parcours d'arborescence. Cette approche rend impossible le maintien d'un plancher global (`min_keep_files`) ou d'un budget cumulatif (`max_total_size`) car la décision sur le premier fichier dépend de l'ensemble complet des candidats valides.

FileCleaner matérialise d'abord la liste complète en mémoire lors de la phase 1 (`Scanner`), fige l'ensemble protégé lors des phases 2 et 3 (`InUseDetector` puis `BackupGuard`), et ne calcule les évictions qu'après avoir garanti les invariants globaux.

### Garanties fail-closed sur les actions composites

Lorsqu'une règle enchaîne `actions = ["compress", "upload_s3", "delete"]`, chaque étape agit sur l'artefact produit à l'étape précédente et vérifie son succès avant de passer à la suite :

1. **Compression pure-Rust (`compress`)** : écrit dans un fichier temporaire `.fc-tmp` puis renomme atomiquement ; en cas d'erreur de codec ou d'espace disque, le fichier source intact reste inchangé.
2. **Transfert S3 vérifié (`upload_s3`)** : calcule l'empreinte SHA-256 locale, signe la requête en SigV4 via `rustls` pur-Rust, puis vérifie côté serveur via `HeadObject` (`checksum_sha256`) ou lecture intégrale (`readback_sha256`).
3. **Suppression conditionnelle (`delete`)** : n'est jamais appelée si une étape précédente échoue ou si un verrou est détecté entre la planification et l'exécution.

## Lectures complémentaires

- [Calcul de rétention et défenses TOCTOU](../analyses-internes/calcul-retention-et-defenses-toctou.md)
- [ADR-001 : Moteur pur-Rust fail-closed](../decisions-architecture/adr-001-moteur-pur-rust-fail-closed.md)
- [Commandes CLI et codes de sortie](../../03-reference/cli-et-scripts/commandes-cli-et-codes-sortie.md)
