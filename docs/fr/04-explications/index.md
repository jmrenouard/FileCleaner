# Explications et architecture FileCleaner

Cette section présente l'architecture interne du moteur, les arbitrages techniques formalisés dans les ADR et le modèle mathématique de rétention et de défense contre les conditions de concurrence TOCTOU.

## Documents d'architecture

| Thème | Document | Sujets abordés |
| :--- | :--- | :--- |
| **Vue d'architecture** | [Pipeline fail-closed en 6 phases](vue-architecture/pipeline-fail-closed-six-phases.md) | Séquence déterministe `Scan -> InUse -> BackupGuard -> Retention -> Hook -> Execute` et garantie fail-closed |
| **Décision ADR-001** | [ADR-001 : Moteur pur-Rust fail-closed](decisions-architecture/adr-001-moteur-pur-rust-fail-closed.md) | Dépendances pures Rust (`rustls-rustcrypto`, `flate2`, `lz4_flex`, `brotli`), zéro shell et zéro OpenSSL système |
| **Décision ADR-002** | [ADR-002 : Publication Diátaxis et MkDocs versionnée](decisions-architecture/adr-002-publication-diataxis-mkdocs-versionnee.md) | Séparation stricte EN/FR (`mkdocs.yml` + `mkdocs.fr.yml`), instantanés Git multi-versions et portail IONOS |
| **Analyses internes** | [Calcul de rétention et défenses TOCTOU](analyses-internes/calcul-retention-et-defenses-toctou.md) | Évaluation ensembliste des plafonds, formule de stabilité `(size, mtime)` et neutralisation des fenêtres de course |

Pour passer à la mise en pratique, ouvrez les [Tutoriels](../01-tutoriels/index.md) ou les [Guides pratiques](../02-guides-pratiques/index.md).
