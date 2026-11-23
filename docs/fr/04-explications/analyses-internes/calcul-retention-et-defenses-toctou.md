# Calcul de rétention et défenses TOCTOU

Cette analyse formalise la logique mathématique appliquée lors des phases 2 à 4 du pipeline afin de garantir qu'aucune sauvegarde critique ou aucun fichier en cours d'écriture ne soit supprimé par erreur.

## Qualification et partition des candidats

Soit $\mathcal{C} = \{f_1, f_2, \dots, f_n\}$ l'ensemble des fichiers correspondant aux filtres de chemin, globs, regex et bornes de taille (`min_file_size` et `max_file_size`). Chaque fichier $f_i \in \mathcal{C}$ possède un horodatage effectif $t_i$ issu de `time_basis` (`modified`, `created` ou `filename_date`), une taille en octets $s_i \ge 0$, et un état d'occupation booléen $\text{in\_use}(f_i) \in \{\text{true}, \text{false}\}$.

Tout candidat pour lequel $\text{in\_use}(f_i) = \text{true}$ quitte immédiatement l'ensemble éligible à l'éviction et passe dans l'ensemble verrouillé $\mathcal{L}$.

## Protection `BackupGuard`

Lorsque `[rules.backup_guard]` est actif avec un seuil de validité $S_{\text{min}} = \text{min\_valid\_backup\_bytes}$, l'ensemble des sauvegardes valides vaut :

$$
\mathcal{V} = \{ f_i \in \mathcal{C} \mid s_i \ge S_{\text{min}} \}
$$

Deux conditions déclenchent un gel complet (`freeze`) de la règle avant même l'évaluation des plafonds de rétention :

1. **Absence d'archive valide** : $|\mathcal{V}| = 0$ lorsque `freeze_on_empty_valid = true`.
2. **Défaut de fraîcheur (dead-man's-switch)** : pour un instant de référence $t_{\text{now}}$ et une fenêtre $\Delta_{\text{fresh}} = \text{require\_newer\_within}$ :

$$
\max_{f_i \in \mathcal{V}} t_i < t_{\text{now}} - \Delta_{\text{fresh}}
$$

Hors situation de gel, les $K = \text{min\_keep\_files}$ sauvegardes les plus récentes de $\mathcal{V}$ forment le plancher protégé $\mathcal{P}_{\text{guard}}$, inaliénable quel que soit l'âge ou la taille des fichiers.

## Évaluation ensembliste des plafonds de rétention

Sur les candidats éligibles triés selon `eviction_order` (`oldest_first`, `largest_first` ou `smallest_first`), trois violations élémentaires évaluent si la conservation de $f_i$ dépasse un plafond :

- Dépassement de compte : le rang de récence excède `max_files`.
- Dépassement d'âge : $t_{\text{now}} - t_i > \text{max\_age}$.
- Dépassement de volume : l'accumulation des octets restants dépasse `max_total_size` :

$$
\sum_{j \in \mathcal{R}} s_j > B_{\text{max}}
$$

La décision suit `combine_mode` :

- Mode `any` : un fichier est candidat à l'éviction s'il enfreint au moins un des plafonds configurés (union).
- Mode `all` : un fichier n'est évincé que s'il enfreint simultanément tous les plafonds actifs (intersection).

En présence d'un conflit entre un plafond (`max_files`, `max_total_size`) et un plancher de sécurité ($\mathcal{L} \cup \mathcal{P}_{\text{guard}}$), les planchers priment systématiquement. L'option `--strict-constraints` signale ce résidu par le code de sortie `3` sans jamais enfreindre la protection.

## Défenses contre les conditions de concurrence TOCTOU

Une faille *Time-Of-Check to Time-Of-Use* (TOCTOU) survient si un processus externe ouvre, verrouille ou remplace un fichier entre le moment où FileCleaner l'analyse et le moment où il le compresse ou le supprime.

| Couche de défense | Mécanisme implémenté | Menace neutralisée |
| :--- | :--- | :--- |
| **Non-suivi des liens** | `follow_symlinks = false` au parcours | Traversée hors périmètre via remplacement par lien symbolique |
| **Stabilité deux points** | Double échantillonnage $(s_i, t_i)$ espacé de `stability_window` | Écriture silencieuse sans verrou explicite (`mtime` ou taille mouvante) |
| **Âge d'inactivité** | Condition $t_{\text{now}} - t_{\text{mtime}}(f_i) \ge \text{min\_idle\_age}$ | Fichier fraîchement créé en attente de finalisation |
| **Fichiers compagnons** | Inspection des suffixes `.lock`, `.lck`, `.part`, `.tmp`, `.writing` | Transferts SFTP/rsync/pg_dump signalés uniquement par sidecar |
| **Sondes noyau OS** | `flock` non bloquant / `/proc/*/fd` (Unix) et `share_mode(0)` (Windows) | Descripteur ouvert actif par un processus de production |
| **Re-vérification pré-action** | Contrôle réitéré juste avant mutation et rename atomique `.fc-tmp` | Fenêtre de course entre phase 4 (`plan`) et phase 6 (`execute`) |

## Références associées

- [Pipeline fail-closed en 6 phases](../vue-architecture/pipeline-fail-closed-six-phases.md)
- [Appliquer les hooks pré-suppression et protections TOCTOU](../../02-guides-pratiques/securite/appliquer-hooks-pre-suppression-et-toctou.md)
