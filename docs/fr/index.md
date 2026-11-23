<p align="center">
  <img src="../assets/header-banner.svg" alt="FileCleaner — Moteur déterministe de rétention de répertoires en Rust pur" width="100%" />
</p>

# Documentation FileCleaner (Français)

**FileCleaner** est un moteur déterministe et pur-Rust de rétention de répertoires, doté de garde-fous de sauvegarde, de détection d'utilisation active en 6 couches, d'archivage S3 vérifié par SHA-256 et d'un démon temps réel.

> [!NOTE]
> La documentation est structurée selon l'architecture **Diátaxis** afin de séparer l'apprentissage, les procédures d'astreinte, les tables de référence et les choix d'architecture. Pour la version anglaise, ouvrez le [portail anglais](../index.md).

## Quadrants Diátaxis

| Quadrant | Objectif | Quand l'utiliser | Point d'entrée |
| :--- | :--- | :--- | :--- |
| **Tutoriels** | Apprendre par la pratique | Premiers pas en bac à sable isolé, tests BackupGuard et labo PostgreSQL WAL/S3 | [Tutoriels](01-tutoriels/index.md) |
| **Guides pratiques** | Accomplir une tâche opérationnelle | Déployer Docker/systemd, configurer S3, gérer un gel d'astreinte | [Guides pratiques](02-guides-pratiques/index.md) |
| **Référence** | Vérifier des faits exacts | Clés TOML/YAML, sous-commandes CLI, codes de sortie, matrice d'alertes | [Référence](03-reference/index.md) |
| **Explications** | Comprendre la conception | Pipeline fail-closed en 6 phases, ADR-001/002, calcul TOCTOU | [Explications](04-explications/index.md) |

## Manuels complets et gouvernance

| Ressource | Description | Accès |
| :--- | :--- | :--- |
| **Présentation générale** | Vue d'ensemble bilingue et installation rapide (`v1.0.1` / `main`) | [Aperçu du projet](../../README.fr.md) |
| **HOWTO 5 minutes** | Démarrage rapide en une page avec exemples prêts à copier | [HOWTO](HOWTO.md) |
| **Guide utilisateur** | Manuel complet d'utilisation, démon et archivage objet | [Guide utilisateur](GUIDE_UTILISATEUR.md) |
| **Exemples et recettes** | Recettes de production TOML et YAML commentées | [Exemples et recettes](EXEMPLES_RECETTES.md) |
| **Spécifications formelles** | Spécifications techniques détaillées et diagrammes de séquence | [Spécifications](SPECIFICATIONS.md) |
| **Notes et historique** | Journal des versions et notes de publication officielles | [Journal des versions](../../CHANGELOG.md) · [Notes v1.0.1](../RELEASE_NOTES_v1.0.1.md) |
