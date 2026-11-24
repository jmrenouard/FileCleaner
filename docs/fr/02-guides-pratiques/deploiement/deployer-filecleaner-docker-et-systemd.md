# Déployer FileCleaner avec Docker et systemd

Suivez cette procédure pour installer FileCleaner en service systemd ou en conteneur Docker non privilégié.

## Prérequis

- Installez le binaire `filecleaner` dans `/usr/local/bin/filecleaner` ou installez Docker Engine.
- Créez un répertoire système `/etc/filecleaner` lisible par le compte de service.

## Déployer en service systemd

1. Générez une configuration initiale au format TOML :

```bash
filecleaner init --profile backups --format toml -o /etc/filecleaner/policy.toml
```

2. Adaptez le champ `path`, les seuils de rétention et le chemin d'audit dans `/etc/filecleaner/policy.toml`.
3. Validez la syntaxe et les invariants de sécurité :

```bash
filecleaner validate -c /etc/filecleaner/policy.toml
```

4. Générez le fichier d'unité systemd pour le compte de service `filecleaner` :

```bash
filecleaner daemon service-template \
  --init-system systemd \
  --config /etc/filecleaner/policy.toml \
  --user filecleaner \
  -o /etc/systemd/system/filecleaner.service
```

5. Rechargez systemd puis démarrez le service :

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now filecleaner.service
```

## Exécuter en conteneur Docker non-root

> [!NOTE]
> L'image officielle repose sur `debian:bookworm-slim` sous l'identifiant non-root `10001:10001` (`fcuser:fcgroup`). Montez vos dossiers cibles en lecture-écriture et votre configuration en lecture seule.

1. Appliquez les droits `10001:10001` sur les fichiers et répertoires hôtes :

```bash
sudo chown 10001:10001 /etc/filecleaner/policy.toml
sudo chown -R 10001:10001 /var/backups/app /var/log/filecleaner
```

2. Lancez un premier plan de simulation sans modification de fichiers :

```bash
docker run --rm \
  -v /etc/filecleaner/policy.toml:/etc/filecleaner/policy.toml:ro \
  -v /var/backups/app:/var/backups/app:rw \
  -v /var/log/filecleaner:/var/log/filecleaner:rw \
  jmrenouard/filecleaner:v1.0.1 \
  plan -c /etc/filecleaner/policy.toml
```

3. Vérifiez le plan affiché puis démarrez le service en tâche de fond avec `--restart unless-stopped`.

## Validation

Exécutez les commandes ci-dessous pour vérifier le service actif et son état JSON :

```bash
systemctl status filecleaner.service --no-pager
filecleaner daemon status -c /etc/filecleaner/policy.toml --json
```

Vérifiez que `systemctl status` affiche `active (running)` et que l'état JSON indique un PID vivant.

## Retour arrière

Arrêtez le démon avec la commande de terminaison coopérative, puis désactivez l'unité systemd :

```bash
filecleaner daemon stop -c /etc/filecleaner/policy.toml
sudo systemctl disable --now filecleaner.service
```

En mode Docker, supprimez le conteneur actif avec `docker rm -f filecleaner` et réactivez `dry_run = true`.

## Références associées

- [Commandes CLI et codes de sortie](../../03-reference/cli-et-scripts/commandes-cli-et-codes-sortie.md)
- [Schéma de configuration et invariants](../../03-reference/configuration/schema-configuration-et-invariants.md)
