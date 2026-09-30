# Reprise de la campagne de vérification (à lire en premier)

Écrit le 2026-09-30 pour un agent qui reprend sans l'historique. Vérifie l'état réel (CI, `git log`, issues) avant de t'y fier.

## Le projet et les règles de Wian
- Dépôt `WianBroes/cmux-gtk` : fork Linux/GTK4 de cmux (amont macOS : `manaflow-ai/cmux`, cloné en lecture seule dans `/home/user/manaflow-ai/cmux` si présent : `git clone --depth 1`). **Branche de travail réelle : `local/wian`** (branche par défaut). `main` est l'amont plus des commits poussés par erreur : **ne jamais y pousser**.
- Wian n'est pas développeur et **ne relit pas le code**. Parle-lui en français simple, dis ce qui est prouvé, supposé, non fait. **Objectif : un port de cmux macOS le plus fidèle possible, avec quelques améliorations** (à proposer une par une). Après le bug de l'agent browser et le « code caché », rescan complet : **tout ce qui est présent et nouveau doit être testé**.
- « Fini » = un scénario réel en CI qui échoue avant/casse et passe après, plus une preuve par mutation. Pas de test d'identité, pas d'assouplissement d'un test pour qu'il passe. Vérifier « comme macOS » dans le code amont, pas dans un message de commit.
- Ne pas lancer de tests en local (règle du dépôt) : tout par GitHub Actions. Dépôt public : aucun secret ni chemin personnel.
- Autorisation reçue : je peux fusionner la PR 11 dans `local/wian` quand la CI est **entièrement verte**, en prévenant Wian avant (la question lui a été posée, la réponse « oui » n'a pas été confirmée explicitement : redemande si besoin). La PR modifie `.github/workflows/ci.yml` et ajoute des workflows.

## Où est le travail
- Branche `claude/verification-on-fork` (issue de `local/wian`), **PR 11** vers `local/wian`. Issue 9 (diff navigateur étroit) fermée. Issue 10 (régressions CI) : cause trouvée, voir plus bas ; à commenter/fermer une fois la PR fusionnée.
- Fichiers clés : `docs/VERIFICATION.md` (registre par fonction, statut et lien du run), `docs/CAMPAGNE-TESTS.md` (34 fonctionnalités F01-F34 de `docs/NOS-MODIFICATIONS.md`, en 4 lots), `tests/mutation_cases.json` + `tests/run_mutation.py` + `.github/workflows/mutation.yml` (cassages volontaires : chaque test doit échouer), `.github/workflows/probe.yml` + `tests/probe_tests.txt` (rejoue des tests listés et imprime la cause), `.github/workflows/bisect.yml` (manuel).
- Ce qui a été prouvé sur le fork (voir VERIFICATION.md) : `send-text`/`send-key`/`read-text`, sidebar status/progress, OSC 9/99/777, notifications, hooks de tous les agents (faux binaires), reprise manuelle et approuvée, ports, git, groupes/réordonnancement, session/scrollback/fermeture immédiate, fenêtre, diff et projet dans un vrai navigateur, SSH (3 étapes). 12+ cassages du banc de mutation attrapés.
- Un seul changement de code produit dans la PR : `src/cli/diff.rs` (liste de fichiers en bande sous 760 px ; en-tête qui passe à la ligne dans un volet étroit).

## Ce que la campagne a appris
- Les « régressions » de l'issue 10 étaient des **tests périmés** après des changements voulus alignés sur macOS (une notification par terminal ; `send-key` avec noms de touches ; résumé d'une ligne de 180/200 caractères pour les hooks), le **titre du terminal qui change tout seul** (comparaisons de mise en page sans le titre : `Application.layout()`), le premier workspace sans nom explicite qui suit le titre, et **Préférences en onglets** (App/Terminal/Raccourcis) : `Alt+A` d'approbation exige l'onglet Terminal (`ctrl+Next` d'abord).
- Ctrl+N ouvre un **dialogue** (pas de création directe). Ctrl+D par défaut = « diviser à droite » **et** EOF de terminal (choix du fork, non modifié).
- La frappe simulée `xdotool type` sous Openbox est **instable** (même test, résultats différents) : ne pas en déduire un bug ; pour remplir une ligne de commande utiliser `cmux send` (socket).
- Tests instables observés : cycle de vie du navigateur (1 échec sur ~7), benchmark navigateur (1 échec `Browser preview startup deadline exceeded`).

## En cours au moment de cette note
1. **Issue 4 (raccourcis)** : `tests/test_linux_shortcut_effect.py` (Ctrl+D divise, `cmux.json` rebranche `splitRight` en Ctrl+Maj+K, `reload-config`, nouvelle touche divise, ancienne inerte). La partie « saisie » a été retirée. À lire : dernier run Probe. Si PASS : lire le run Mutation (cassage `shortcut-live-reload` attendu KILLED) et mettre le registre à jour.
2. **Lot 1** `tests/test_linux_agent_cli_scenarios.py` (F12 titre→nom du workspace, F19 `new-split --command` sans vol de focus, F20 `tree`, F21 `send`+`read-screen`, F23 journal). Dernier constat : `send` puis `read-screen` n'a pas montré `CMUX_SEND_OK` sur sa ligne ; le test affiche maintenant l'écran brut en cas d'échec. Ajuster le test si c'est un format, signaler à Wian si c'est un défaut de la fonction. Quand vert : ajouter l'étape CI (avec `if: ${{ !cancelled() && steps.debug_build.conclusion == 'success' }}`) et des cassages au banc de mutation.
3. Lots 2 à 4 de `docs/CAMPAGNE-TESTS.md`, puis balayage des fichiers touchés par plusieurs fonctionnalités (`src/cli/mod.rs`, `src/socket/handlers.rs`…) ; verbes navigateur F29/F30/F31 = issues 2, 3, 5 (daemon agent-browser >= 0.38.1 patché via `CMUX_AGENT_BROWSER`) ; tri des changements macOS depuis le commit épinglé `7d78b6e` (l'amont est en 0.64.25).
4. Mettre à jour le registre et le suivi à chaque résultat ; garder la description de la PR à jour.

## Astuces pour lire la CI à peu de frais
- `get_job_logs` ne renvoie que la fin du log (plafond ~5000 lignes) : pour un échec ancien, rejouer le test avec le workflow **Probe** (édite `tests/probe_tests.txt`, un push sur la branche le lance ; il imprime la ligne d'assertion). Pour parser un gros log, il est sauvegardé dans un fichier : le lire avec Python, pas avec Read.
- `list_workflow_jobs` avec `perPage: 1` et `page` 1 ou 2 pour choisir le job (`linux-build` vs `remote-daemon-tests`). Les étapes CI sont numérotées (voir les noms dans `ci.yml`).
- Chaque push sur la branche relance CI, Mutation et Probe (concurrence : un nouveau push annule l'ancien run CI) : grouper les changements avant de pousser et lire les résultats avant de repousser.
- Le workflow `Mutation` se lance sur PR (fichiers du banc) ; `workflow_dispatch` est indisponible tant que ces workflows ne sont pas sur la branche par défaut.
