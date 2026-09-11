# RestoreProof Core

*[English](README.md) · **Français***

**Arrêtez de dire que vos sauvegardes fonctionnent. Prouvez que votre application peut vraiment redémarrer.**

RestoreProof exécute un *exercice de reprise* : il restaure une sauvegarde dans
un environnement isolé, démarre vos services à partir de celle-ci, vérifie que
l'application répond et que les données sont bien là, mesure le temps que cela a
pris, et écrit un rapport que vous pouvez montrer à quelqu'un.

Une tâche de sauvegarde qui se termine par `0` vous dit qu'un fichier a été
écrit. Elle ne vous dit pas que le dump est complet, que le schéma se charge
encore, que l'application démarre dessus, ni combien de temps tout cela prend.
C'est cet écart que l'outil comble.

> **État : jeune projet.** Version 0.1.0. Le cœur est testé et le modèle de
> sécurité est réfléchi, mais le projet est récent. Il convient aux exercices de
> reprise en local et en intégration continue. Ce n'est pas un produit de
> sauvegarde, il n'en remplace aucun, et mieux vaut l'essayer d'abord sur
> quelque chose dont vous n'avez pas besoin.

---

## À quoi ressemble une exécution

```console
$ export RESTOREPROOF_DEMO_DATABASE_URL='postgres://restoreproof:restoreproof-local-drill@127.0.0.1:15432/app'
$ restoreproof run --config examples/postgres-local/restoreproof.yaml

PASS PASSED  postgres-local · 2026-09-11 15:03:29Z · 4.9 s

  ok   The dump was restored to di…     0 ms  `dump.sql` is present (806 bytes)
  ok   PostgreSQL reports healthy     108 ms  service `database` is running and healthy
  ok   The orders table contains d…    78 ms  query returned 1 row(s) as expected
  ok   The customers table contain…    70 ms  query returned 1 row(s) as expected
  ok   The most recent order came …    74 ms  query returned 1 row(s) as expected

  Checks   5 passed, 0 failed, 0 error, 0 skipped (of 5)
  RTO      4.3 s (target 10m 00s)  RTO_PASS
  RPO      12m 58s (target 24h 00m 00s)  RPO_PASS
  Backup   local · unknown snapshot · 806 B

  Report   examples/postgres-local/reports/20260911T150329Z-postgres-local-45efee6c.json
  Report   examples/postgres-local/reports/20260911T150329Z-postgres-local-45efee6c.md
```

<sub>Sortie réelle de `examples/postgres-local`, avertissements omis. L'outil
lui-même ne parle qu'anglais : voir « Limites ». Un enregistrement de session
manque ici ; c'est suivi dans [ROADMAP.md](ROADMAP.md).</sub>

Le code de sortie `0` signifie que toutes les vérifications obligatoires sont
passées. Le code `1` signifie que votre reprise ne fonctionne pas — ce qui est
précisément la réponse que vous vouliez connaître.

## Essayez en trois commandes

Il vous faut Rust et Docker avec le greffon Compose v2.

```bash
git clone https://github.com/nikko1700-52/RestoreProof-core
cd RestoreProof-core

export RESTOREPROOF_DEMO_DATABASE_URL='postgres://restoreproof:restoreproof-local-drill@127.0.0.1:15432/app'
cargo run -- run --config examples/postgres-local/restoreproof.yaml
```

Puis regardez-le attraper une vraie panne :

```bash
cargo run -- run --config examples/postgres-local/restoreproof.failing.yaml
# FAILED, code de sortie 1 : la table invoices n'est pas dans la sauvegarde
```

Pour partir de zéro sur votre propre projet :

```bash
cargo run -- init mon-exercice
cargo run -- plan --config mon-exercice/restoreproof.yaml
```

## Ce qu'il vérifie

| Vérification | Répond à |
|---|---|
| `file` | Le dump est-il bien là, à la bonne taille, avec la bonne empreinte ? |
| `container` | Le service a-t-il démarré, et son healthcheck passe-t-il ? |
| `http` | L'application répond-elle, avec le contenu attendu ? |
| `sql` | **Les données sont-elles là ?** Requêtes en lecture seule, avec assertions sur les lignes et les valeurs. |
| `command` | Tout ce que votre propre outillage sait affirmer. |
| `script` | Un script versionné dans votre projet, qui passe ou échoue par son code de sortie. |

### Où vont les résultats

| Format | Pour |
|---|---|
| Terminal | la personne qui lance l'exercice |
| JSON | l'enregistrement de référence ; c'est lui que couvre l'empreinte d'intégrité |
| Markdown | coller dans un ticket ou envoyer à un client |
| **JUnit XML** | le rapport de tests de votre CI — chaque vérification devient un cas de test |
| **Prometheus** | le collecteur *textfile* de `node_exporter` — pour alerter sur la reprise |

Surveiller un exercice nocturne ne demande ni serveur ni compte :

```promql
# La reprise est cassée.
restoreproof_drill_success == 0

# Aucun exercice depuis un jour. Le silence n'est pas un succès.
time() - restoreproof_drill_completed_timestamp_seconds > 86400
```

Et `restoreproof diff` répond à la question qu'un seul rapport ne peut pas
trancher :

```console
$ restoreproof diff reports/lundi.json reports/mardi.json

  Status   PASSED → ERROR   WORSE

  Checks
    ! The orders table contains d… PASSED → ERROR

  Recovery got worse between these two drills.
```

Il sort en `1` en cas de régression : il sert donc de garde-fou en CI à lui seul.

Et il mesure :

* **RTO** — combien de temps la reprise a réellement pris, comparé à votre
  objectif.
* **RPO** — quel âge ont les données restaurées, comparé à votre objectif.

Quand une valeur ne peut pas être déterminée, le rapport indique `UNKNOWN`. Elle
n'est jamais devinée, ni discrètement remplacée par zéro.

## Sources de sauvegarde

| Type | Nécessite | Remarques |
|---|---|---|
| `local` | rien | un répertoire ou un fichier produit par votre propre tâche de dump |
| `restic` | `restic` | lecture seule : `snapshots` et `restore` uniquement |
| `borg` | `borg` | expérimental |

Les stockages objet, les snapshots d'hyperviseur et les API d'éditeurs sont hors
périmètre ici ; voir [PREMIUM.md](PREMIUM.md).

## Modèle de sécurité

Un exercice de reprise restaure des données de production et démarre des
conteneurs à partir de celles-ci. Mal fait, l'exercice *devient* l'incident. La
conception part de ce constat et pousse dans l'autre sens :

| Risque | Ce que fait l'outil |
|---|---|
| Injection de commande | Jamais de shell. Les programmes sont lancés depuis un tableau `argv`. |
| Injection d'argument | Les valeurs qu'un outil externe lirait comme des options sont refusées dès la validation. |
| Traversée de chemin | Chaque chemin configuré est canonicalisé — liens symboliques compris — et confiné au répertoire du projet. |
| « Vérifications » destructrices | Le SQL s'exécute dans une transaction `READ ONLY` avec un délai côté serveur, toujours annulée, et seules les instructions `SELECT`/`WITH` uniques sont acceptées. |
| Falsification de requête (SSRF) | Les vérifications HTTP ne visent que la boucle locale, sauf autorisation explicite. Les proxys sont ignorés, les redirections revalidées. |
| Évasion de conteneur | Le fichier Compose est audité avant tout démarrage : conteneurs privilégiés, socket Docker, espaces de noms de l'hôte, capacités dangereuses et montages de chemins hôte sont refusés. |
| Exposition des données | Les ports publiés sur `0.0.0.0` sont refusés ; la boucle locale convient. Les rapports sont en `0600`, les données restaurées dans un espace de travail en `0700`. |
| Restes après `Ctrl-C` | `SIGINT` et `SIGTERM` sont interceptés : l'environnement est détruit *avant* la sortie du processus, et tout ce qui n'a pas pu l'être est nommé. |
| Fuite de secret | Les secrets viennent de fichiers ou de variables d'environnement, jamais du YAML. Ils n'atteignent ni `argv`, ni un journal, ni un rapport. |
| Environnements oubliés | La destruction s'exécute sur tous les chemins, y compris les panics et les dépassements de délai. |

Le code `unsafe` est interdit dans tout l'espace de travail, et
`unwrap`/`expect`/`panic` sont refusés dans le code de bibliothèque.

Le modèle de menace complet, y compris **ce qui n'est pas couvert**, se trouve
dans [SECURITY.md](SECURITY.md) et [docs/security.md](docs/security.md).

## Comment tout s'assemble

```
restoreproof-cli        analyse des arguments et sortie terminal, rien d'autre
      │
restoreproof-runner     restaurer → démarrer → attendre → vérifier → détruire → rapporter
      │
      ├── restoreproof-storage   BackupSource : local, restic, borg
      ├── restoreproof-checks    les six exécuteurs de vérifications
      ├── restoreproof-config    YAML strict, validation, gardes chemins/SQL/URL/Compose
      ├── restoreproof-report    JSON, Markdown, terminal
      └── restoreproof-core      statuts, codes de sortie, secrets, rédaction, RTO/RPO,
                                 exécution de processus sous contrainte
```

Rien ne dépend du CLI, à part le CLI. Un planificateur, une API HTTP ou une
console web se placeraient là où se trouve `restoreproof-cli` et réutiliseraient
tout ce qui est en dessous, sans modification. Voir
[docs/architecture.md](docs/architecture.md).

## Documentation

La documentation détaillée est en anglais.

* [Démarrer](docs/getting-started.md)
* [Référence de configuration](docs/configuration.md)
* [Types de vérifications](docs/checks.md)
* [Exploiter les exercices en production](docs/operations.md) — planification, rétention, alertes
* [Sécurité](docs/security.md)
* [Architecture](docs/architecture.md)
* [Dépannage](docs/troubleshooting.md)

## Codes de sortie

| Code | Signification |
|---|---|
| 0 | toutes les vérifications obligatoires sont passées |
| 1 | au moins une vérification obligatoire a échoué |
| 2 | configuration invalide |
| 3 | une dépendance externe requise est absente |
| 4 | la sauvegarde n'a pas pu être restaurée |
| 5 | un délai a expiré |
| 6 | erreur interne |
| 7 | usage incorrect de la ligne de commande |

C'est un contrat stable : les chaînes d'intégration continue sont censées s'y
fier.

## Limites

À savoir avant de compter dessus :

* Les environnements de reprise sont **Docker Compose** uniquement. Ni
  Kubernetes, ni VMware, ni Proxmox.
* Un scénario à la fois, sur une seule machine. Pas de planificateur, pas
  d'historique centralisé, pas d'interface web.
* L'audit Compose est **statique**. Il raisonne sur le fichier Compose, pas sur
  ce que font vos images à l'exécution. C'est un garde-fou, pas un bac à sable
  de conteneurs.
* Les empreintes de rapport détectent une modification accidentelle. Ce ne sont
  **pas des signatures**.
* Les vérifications s'exécutent séquentiellement, un scénario à la fois. Rien
  n'est parallélisé.
* La prise en charge de BorgBackup est expérimentale, et l'exemple restic n'est
  pas exécuté par la CI (celle-ci n'installe pas restic).
* Testé sur Linux (Debian/Ubuntu). Les autres plateformes ne sont pas vérifiées.
* **L'outil ne parle qu'anglais** : messages du CLI, rapports et documentation
  détaillée. Seuls les fichiers README existent en français. Traduire la sortie
  demanderait un vrai travail d'internationalisation, et prétendre le contraire
  serait malhonnête.

## L'utiliser pour de vrai

Un exercice lancé une fois à la main prouve que la procédure fonctionne. Un
exercice qui tourne chaque nuit prouve que les sauvegardes fonctionnent.
[docs/operations.md](docs/operations.md) traite du second cas : minuteries
systemd, où vont les rapports et pour combien de temps, règles d'alerte qui se
déclenchent aussi quand l'exercice devient *silencieux*, et une liste de
contrôle à parcourir avant de faire confiance à tout cela — en commençant par
« faites-le échouer exprès et vérifiez qu'il passe au rouge ».

## Contribuer

Les tickets et les *pull requests* sont bienvenus — en particulier les rapports
de bogue accompagnés d'un scénario qui les reproduit. Commencez par
[CONTRIBUTING.md](CONTRIBUTING.md). Les échanges se font en anglais, mais un
ticket en français ne sera pas rejeté pour autant.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

## Licence

[Apache-2.0](LICENSE).

Une édition commerciale est prévue pour ce qui a été délibérément laissé de côté
ici — planification centralisée, historique multi-client, rapports signés,
environnements Kubernetes et hyperviseurs. Ce qui relève de son périmètre, et
pourquoi, est écrit dans [PREMIUM.md](PREMIUM.md). Rien dans ce dépôt n'est
limité dans le temps, verrouillé par fonctionnalité ou connecté à un serveur
distant, et rien ici ne le deviendra.
