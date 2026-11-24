# Guides pratiques FileCleaner

Utilisez ces modes opératoires pour déployer, configurer, verrouiller et dépanner FileCleaner en production. Chaque procédure suit des étapes courtes et vérifiables.

## Modes opératoires disponibles

| Domaine | Guide pratique | Objectif opérationnel |
| :--- | :--- | :--- |
| **Déploiement** | [Déployer avec Docker et systemd](deploiement/deployer-filecleaner-docker-et-systemd.md) | Générer un service systemd, exécuter le conteneur non-root et vérifier la supervision |
| **Exploitation** | [Configurer la rétention et l'archivage S3](exploitation/configurer-retention-et-archivage-s3.md) | Configurer S3 avec `checksum_sha256`, exporter les variables d'environnement et activer la purge |
| **Exploitation** | [Exécuter le démon réactif et la rotation des journaux](exploitation/executer-demon-reactif-et-rotation-journaux.md) | Démarrer le démon en mode hybride et contrôler son arrêt propre via fichier témoin |
| **Gestion des incidents** | [Résoudre un gel de sauvegarde ou un échec S3](gestion-incidents/resoudre-gel-sauvegarde-ou-echec-s3.md) | Diagnostiquer le code de sortie `3` ou `1`, restaurer une archive valide et relancer un balayage |
| **Sécurité** | [Appliquer les hooks pré-suppression et protections TOCTOU](securite/appliquer-hooks-pre-suppression-et-toctou.md) | Exécuter un hook synchrone sans shell et activer les 6 sondes de verrouillage |

Consultez les [Recettes de production](../EXEMPLES_RECETTES.md) pour copier des profils complets, et la [Référence](../03-reference/index.md) pour vérifier les clés de configuration exactes.
