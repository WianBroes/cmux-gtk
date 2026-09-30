# Campagne de tests du fork

Objectif : chaque fonctionnalité de ton fork (F01 à F34, inventaire `docs/NOS-MODIFICATIONS.md`) a un test qui joue un vrai scénario dans l'application, et qui **échoue quand on casse la fonction** (banc de mutation). Ce fichier est tenu à jour à chaque lot ; le détail des preuves est dans `docs/VERIFICATION.md`.

Contrat de référence : le comportement de cmux sur macOS (code amont). Quand un choix du fork diffère volontairement, c'est écrit ici.

Légende : **prouvé** = un test de CI joue le scénario sur ton code et il échoue quand on casse la fonction ; **testé** = le scénario passe mais sans preuve par cassage ; **en cours** ; **à faire**.

## Lot 1. Commandes que les agents utilisent en continu

| Fonctionnalité | Ce que c'est | Avant la campagne | Maintenant |
| --- | --- | --- | --- |
| F12 | Titres de terminal sur les onglets, workspaces nommés d'après l'onglet actif, point non lu, anneau de pane visible, largeur des onglets | **test unitaire seulement** | scénario écrit, en attente du résultat CI |
| F19 | Création de terminaux sans voler le focus (`new-split/new-pane/new-surface/new-workspace --command`) et texte d'aide | **test unitaire seulement** | scénario écrit, en attente du résultat CI |
| F20 | `cmux tree`, `list-pane-surfaces` et indices par workspace | **test unitaire seulement** | scénario écrit, en attente du résultat CI |
| F21 | `send-key` avec noms de touches, `send` avec échappements, `read-screen` | **couvert par CI** | scénario écrit, en attente du résultat CI |
| F23 | Journal de barre latérale, `sidebar-state` et alias de notifications (lot 6) | **test unitaire seulement** | scénario écrit, en attente du résultat CI |
| F18 | Références de socket `window:N`/`workspace:N`/`surface:N`, drapeaux `--workspace/--surface/--pane`, `--id-format`, `identify`, repli `CMUX_SOCKET_PATH | **couvert par CI** | à faire |
| F09 | Flux d'événements reconnectable (`events.stream`, `cmux events`) | **test unitaire seulement** | à faire |
| F13 | Activité d'agent dans la barre latérale et sur l'onglet (spinner, badge non lu, dernier message, icône par agent) | **test unitaire seulement** | à faire |
| F06 | Les hooks d'agents relient la reprise à chaque prompt (rattrape un SessionStart manqué) | **couvert par CI** | à faire |
| F07 | Hooks d'agents : échec silencieux si l'application n'existe plus ; Pi annonce ses dialogues en Notification | **couvert par CI** | à faire |
| F11 | Notifications comme l'amont : une par terminal, focus = lu, cloche avec compteur, résumé d'une réponse d'agent, préférence pour couper les notificatio | **couvert par CI** | à faire |
| F25 | `workspace-action`, `tab-action`, `move-tab-to-new-workspace`, `trigger-flash`, `surface-health` | **test unitaire seulement** | à faire |

## Lot 2. Interface et clavier

| Fonctionnalité | Ce que c'est | Avant la campagne | Maintenant |
| --- | --- | --- | --- |
| F04 | Boutons de split par pane et bouton de barre latérale déplacé à gauche | **aucun test** | à faire |
| F14 | Barre latérale redimensionnable, hiérarchie de texte, panneaux à leur minimum quand un séparateur bouge | **test unitaire seulement** | à faire |
| F15 | Barre de titre comme l'amont (barre latérale, notifications, menu nouveau workspace, Retour/Avance du focus, flèches désactivées) et bouton d'explorat | **test unitaire seulement** | à faire |
| F16 | Preferences en onglets App/Terminal et fenêtre des raccourcis construite avec GtkBuilder | **aucun test** | à faire |
| F17 | Focus et glisser-déposer d'onglets : le clic résout le pane à l'événement, dépôt sur la barre d'onglets = rejoindre le pane, `pane.focus` change de wo | **couvert par CI** | à faire |
| F26 | Panneau droit et explorateur de fichiers (arbre, navigation J/K H/L, filtre, glisser vers un terminal) | **test unitaire seulement** | à faire |
| F27 | Description de workspace (champ, commandes, éditeur) | **test unitaire seulement** | à faire |
| F32 | L'application lit `cmux.json` (raccourcis) par-dessus `config.toml`, `reload-config`, `cmux settings | **test unitaire seulement** | en cours (issue 4) |
| F33 | Page Preferences > Shortcuts : changer, débrancher, réinitialiser ; refus des touches réservées aux terminaux et aux agents avec raison ; débranchemen | **test unitaire seulement** | à faire |
| F24 | Placement d'onglets et de workspaces (move-surface avant/après/index, reorder, split-off, orthographe `workspace create/close/select/rename`, liste/co | **couvert par CI** | à faire |

## Lot 3. Navigateur (agent browser)

| Fonctionnalité | Ce que c'est | Avant la campagne | Maintenant |
| --- | --- | --- | --- |
| F03 | Le viewport du navigateur suit la taille de son pane | **aucun test** | à faire |
| F22 | Traduction des méthodes socket amont du navigateur vers les actions agent-browser (dont scroll --dx/--dy) et formes d'appel amont | **test unitaire seulement** | à faire |
| F29 | Verbes navigateur : viewport, cookies, storage, addinitscript, addstyle, addscript, download, download-wait (avec astuce d'échec) | **test unitaire seulement** | à faire |
| F30 | Verbes navigateur : offline, geolocation, trace, har, network (route/unroute/requests) | **test d'identité (ne prouve rien)** | à faire |
| F31 | `cmux markdown <fichier>` : rendu Markdown autonome dans une surface navigateur | **test unitaire seulement** | à faire |

## Lot 4. Terminal, reprise et le reste

| Fonctionnalité | Ce que c'est | Avant la campagne | Maintenant |
| --- | --- | --- | --- |
| F01 | Touches envoyées à Ghostty avec le codepoint sans Maj et les modificateurs consommés ; collage de fichiers déposés en chemins échappés | **test unitaire seulement** | à faire |
| F02 | Défilement tactile/molette mis à l'échelle comme Ghostty et préférence d'inversion | **aucun test** | à faire |
| F05 | Reprise automatique des sessions d'agents enregistrées par les hooks, avec bascule dans Preferences | **couvert par CI** | à faire |
| F08 | Sessions tmux locales (`cmux local-tmux`, menus Split Right in tmux, Rename, Kill, Preferences) et menu contextuel de terminal toujours cmux | **test unitaire seulement** | à faire |
| F28 | `cmux config` : lecture JSONC, validation contre le schéma, get/list-supported, docs, set/unset avec édition textuelle | **test unitaire seulement** | à faire |
| F10 | Feed (panneau droit des décisions Claude) ajouté puis retiré : effet net nul | **aucun test** | à faire |
| F34 | CI : `ci.yml` se lance aussi sur `local/wian` ; `.claude` ignoré par git | **couvert par CI** | à faire |

## Déjà prouvé sur ton fork (hors lots ci-dessus)

- Notifications OSC, panneau de notifications, hooks de tous les agents listés, reprise de session (manuelle et approuvée), ports, git, groupes, réordonnancement, session, scrollback, fenêtre, diff et projet dans un vrai navigateur, SSH : voir `docs/VERIFICATION.md`.

## Suite de la campagne

1. Lot 1 : lire les résultats CI, ajouter l'étape CI et les cassages du banc de mutation.
2. Lots 2 à 4 dans l'ordre. Les fonctions du navigateur (lot 3) demandent un vrai daemon agent-browser en version 0.38.1 ou plus.
3. Balayage final des fichiers touchés par plusieurs fonctionnalités à la fois (`src/cli/mod.rs`, `src/socket/handlers.rs`…) : c'est là qu'une régression se cache le plus facilement.
