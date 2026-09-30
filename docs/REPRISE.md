# REPRISE — à lire en entier avant toute action

Écrit le 2026-09-30 (heure de Bruxelles : 17h15) par Claude, pour une IA qui reprend **sans l'historique** (DeepSeek, MiMo ou autre).
Tu es probablement moins fiable que l'IA précédente sur les longs raisonnements : **suis ce document à la lettre, une tâche à la fois, et dans le doute ARRÊTE-TOI et demande à Wian.**

---

## 0. Les 12 règles absolues (les violer = travail à refaire)

1. **Wian n'est pas développeur et ne lit pas le code.** Tu lui parles en **français simple**, phrases courtes, sans jargon. Tu donnes les heures en **heure de Bruxelles** (UTC+2 en été, UTC+1 en hiver).
2. **Objectif de Wian : un port de cmux macOS sur Linux le plus fidèle possible, avec quelques améliorations** (à lui proposer **une par une**, jamais à implémenter sans son « oui »).
3. **Branche de travail réelle : `local/wian`** (branche par défaut). **Jamais de push sur `main`** (c'est l'amont macOS + des commits poussés par erreur). Ne touche jamais l'historique (pas de `--force`, pas de rebase de branches partagées).
4. **La ligne « Commit directly to main; no PRs » de `AGENTS.md` NE S'APPLIQUE PAS ici** : la CI ne se lance que sur les Pull Requests, donc on travaille par petites PR vers `local/wian`.
5. **Ne lance JAMAIS les tests en local** (règle du dépôt). Tout se vérifie par **GitHub Actions**. Le dépôt est public : **aucun secret, e-mail ou chemin personnel** dans le code, les tests, les messages de commit.
6. **Un test ne compte que s'il discrimine** : il doit **échouer quand on casse la fonction** (preuve par « mutation », voir §5) et passer sinon. **Interdit** : un test qui compare une chaîne écrite dans le test à la même chaîne écrite dans le code ; **interdit** d'assouplir, de désactiver, d'ignorer ou de « quarantainer » un test pour qu'il passe.
7. **Vérifier « comme macOS » dans le code amont** (`manaflow-ai/cmux`, à cloner en lecture seule avec `git clone --depth 1`), pas dans un message de commit ni de mémoire.
8. **Une tâche = une PR = un sujet.** Petite. Pas de refactor en passant. Ne lance jamais `cargo fmt` sur tout le dépôt.
9. **Tu ne fusionnes JAMAIS une PR sans que Wian ait écrit explicitement « fusionne »** (ou équivalent) pour **cette** PR. Un accord donné pour une PR ne vaut pas pour la suivante. Avant de lui demander : la CI complète doit être **verte** (voir §4).
10. **Ne prétends jamais qu'une chose est prouvée si tu ne l'as pas vue passer en CI.** Distingue toujours dans tes rapports : **prouvé** / **supposé** / **pas fait**.
11. **Un push sur une PR relance toute la CI (~20 min) et annule le run précédent.** Groupe tes changements, pousse une fois, **lis le résultat avant de repousser**.
12. Si un outil est indisponible, un résultat est incompréhensible ou une décision de produit se présente : **arrête-toi, résume en 5 lignes, demande à Wian**. Ne devine pas.

---

## 1. État exact au 30/09/2026 17h15 (Bruxelles)

**Fusionné dans `local/wian`** (PR 11 à 15) :
- `send "commande\n"` exécute la commande (chaque retour à la ligne devient la touche Entrée) ; `read-text` / `read-screen` affichent du texte simple (comme macOS) — PR 11.
- Panneau Fichiers (`src/file_explorer.rs`) : le contenu se met à jour en direct (relecture chaque seconde des dossiers ouverts) — PR 11. **Prouvé par test unitaire seulement.**
- Diff dans un navigateur étroit corrigé (`src/cli/diff.rs`) — PR 11.
- Délai de démarrage du navigateur 15 s → 30 s (`src/browser.rs`, champ `navigation_budget`) : Chromium met 14–15 s à froid en CI — PR 12.
- **Issue 3 corrigée** : `browser geolocation` accorde d'abord la permission `geolocation` à la page (action daemon `permissions`) — PR 13. Preuve avant/après en vrai navigateur.
- **Issue 2** : scénario réel `tests/test_linux_real_browser_verbs.py` (vrai daemon agent-browser 0.38.1 + vrai Chromium) pour `geolocation`, `offline`, `network route/unroute`, `trace`, `har`, avec banc de mutation `tests/browser_mutation_cases.json` (workflow « Browser mutation ») — PR 13-14.
- **Issue 5 (partie navigateur)** : `viewport`, `cookies set/get/clear`, `storage local`, `addstyle`, `addscript`, `addinitscript`, `download` prouvés en réel, 8 cassages de plus — PR 15. Scénario : 19 contrôles sur 19.

**PR OUVERTE : PR 16** (branche `claude/verification-on-fork`) : « Issue 5 : cmux config / settings path ». Contenu : `tests/test_linux_config_cli.py` (vrai binaire, vrais fichiers, XDG_CONFIG_HOME isolé), étape CI correspondante, 4 cassages dans `tests/mutation_cases.json` (`config-validate-exit`, `config-set-value`, `config-unset-noop`, `config-get-key`), `docs/AUDIT-TESTS.md`, et cette note. **Sa CI (CI + Mutation + Probe) a redémarré à 17h07 : son résultat n'est pas lu.** Le scénario config avait passé du premier coup en sonde (17h06). Un cassage (`config-set-value`) avait un texte mal indenté : corrigé au dernier commit, **à vérifier**.

**Pas fait** (voir §6) : audit de `markdown`, `shortcuts`, `reload-config` ; scénario réel du panneau Fichiers ; lots 2 à 4 de la campagne ; tri des changements macOS ; commentaire et fermeture des issues.

---

## 2. Où sont les choses

| Fichier | À quoi il sert |
|---|---|
| `docs/VERIFICATION.md` | Registre : par fonction, scénario, statut, lien du run CI. **À mettre à jour à chaque preuve.** |
| `docs/CAMPAGNE-TESTS.md` | Suivi en français des 34 fonctionnalités F01–F34 (issues de `docs/NOS-MODIFICATIONS.md`), 4 lots. |
| `docs/AUDIT-TESTS.md` | Tableau commande / preuve réelle / test discriminant / trou (issue 5). |
| `tests/mutation_cases.json` + `.github/workflows/mutation.yml` | Banc de cassages volontaires du produit (hors navigateur). |
| `tests/browser_mutation_cases.json` + `.github/workflows/browser-mutation.yml` | Idem pour les verbes navigateur (nécessite agent-browser 0.38.1). |
| `tests/run_mutation.py` | Applique un cassage sur une copie, reconstruit, lance le test, exige qu'il échoue. Variable `MUTATION_CASES` pour choisir le fichier de cas. |
| `.github/workflows/probe.yml` + `tests/probe_tests.txt` | **Rejoue les tests listés** (un par ligne) et imprime les contrôles en échec + 3 passages des tests unitaires. Se lance quand `probe_tests.txt` change. **Remets la liste à `tests/test_linux_agent_cli_scenarios.py` + `tests/test_linux_shortcut_effect.py` après usage.** |
| `.github/workflows/browser-verbs.yml` | Lance `tests/test_linux_real_browser_verbs.py` (agent-browser 0.38.1) et imprime PASS/FAIL. |
| `tests/linux_app.py` | Outils de test : `running_app(...)`, `app.cli(...)`, `app.layout()` (liste des surfaces **sans** le titre, qui change tout seul). |
| `src/cli/mod.rs`, `src/cli/args.rs`, `src/cli/format.rs` | Le CLI `cmux` (arguments, exécution, affichage). |
| `src/socket/handlers.rs` | Réception des commandes côté application. |
| `src/browser.rs` | Traduction des verbes navigateur vers les actions du daemon agent-browser (`daemon_action`). |

---

## 3. Le cycle de travail (à répéter pour CHAQUE tâche)

```
git fetch origin local/wian
git checkout -B claude/verification-on-fork origin/local/wian   # repart de local/wian à jour
# ... modifie les fichiers de LA tâche uniquement ...
git add -A
git commit -m "Message court en anglais décrivant le changement"
git push -u origin claude/verification-on-fork
```
Puis **ouvre une PR** `claude/verification-on-fork` → `local/wian` (titre en français, corps : ce que ça mesure/corrige, « aucun changement de produit » si c'est le cas). Si la PR précédente n'est pas fusionnée, **ne la mélange pas** : demande à Wian.

Chaque push sur une PR lance : **CI** (~20 min, `.github/workflows/ci.yml`), **Mutation**, **Probe**, et si les fichiers concernés changent **Browser verbs** / **Browser mutation**.

---

## 4. Comment lire la CI sans te noyer

- Liste les runs de la branche (outil GitHub « actions list workflow runs », filtre `branch` + `event: pull_request`). **Un run « Probe », « Browser verbs » et « Browser mutation » finit souvent en « success » même si le test interne a échoué** (le workflow imprime seulement un résumé) : **il faut lire le résumé** (voir ci-dessous).
- **CI** : `conclusion: success` = tout vert. En cas d'échec : liste les jobs du run (`perPage: 1`, `page: 1` puis `2` pour choisir `linux-build` ou `remote-daemon-tests`) et repère l'étape en `failure`.
- **Le log d'un job est énorme** : l'outil ne renvoie que ~5000 dernières lignes ; si c'est trop gros il **sauvegarde dans un fichier** : lis ce fichier avec **Python** (pas avec un outil de lecture par lignes), et filtre sur `FAIL`, `PASS`, `Error`, `panicked`. Pour un échec ancien : **rejoue le test avec la sonde** (édite `tests/probe_tests.txt`, pousse).
- **Mutation** : chaque cassage est un job de la matrice. **Job vert = le test a bien échoué avec le cassage (« KILLED »)**. Job rouge = « SURVIVED » (le test ne prouve rien) ou « INVALID » (texte à casser introuvable ou code cassé qui ne compile pas).
- **Browser verbs** : le résumé imprime `N/N verb checks passed` et les lignes `PASS`/`FAIL` avec la valeur brute.
- **Instabilités connues** : le démarrage de Chromium en CI (étapes « real browser », corrigé en PR 12 : si une étape « real browser » échoue avec `Browser preview startup deadline exceeded`, relance **une seule fois** les jobs échoués ; si ça échoue encore, c'est réel). Ne relance **jamais plus d'une fois** ; ne dis jamais « c'est un flake » sans cause.
- **Définition de « prêt à demander la fusion »** : CI verte **et** workflow Mutation vert **et** (si concernés) Browser verbs à `N/N` **et** Browser mutation vert **et** Probe sans `FAIL`.

---

## 5. Modèles à copier

**Un scénario réel** : copie `tests/test_linux_real_browser_verbs.py` (navigateur) ou `tests/test_linux_config_cli.py` (CLI sans application). Règles : chaque contrôle s'exécute même si un précédent échoue ; chaque contrôle **imprime la valeur brute** qu'il juge ; l'échec final liste les contrôles faux ; jamais `sleep` long pour « laisser passer » un problème.

**Un cassage (mutation)** = une entrée de JSON :
```json
{"id":"nom-court","file":"src/browser.rs","old":"<texte EXACT, une seule occurrence dans le fichier>","new":"<texte cassé qui compile>","test":"tests/le_test.py","claim":"phrase en français simple de ce que le test doit protéger"}
```
Vérifie avant de pousser que `old` apparaît **exactement une fois** dans `file` (indentation comprise). Le cassage doit **compiler** et **changer le comportement observable**.

**Un rapport à Wian** (toujours ce plan, en français simple, heures de Bruxelles) :
1. **Ce qui est prouvé** (avec le résultat concret et l'heure du run).
2. **Ce qui est supposé / pas prouvé**.
3. **Ce que j'ai fait / ce qui reste**.
4. **Ce dont j'ai besoin de lui** (accord pour fusionner, décision de produit). Une seule question claire.

---

## 6. File d'attente des tâches (dans l'ordre ; ne saute pas d'étape)

### T0 — Terminer la PR 16 (config)
1. Lis la CI de la PR 16 (§4). 2. Si un cassage `config-*` est « INVALID » ou « SURVIVED » : corrige le cas (ou le test), repousse. 3. Si `docs/AUDIT-TESTS.md` dit « oui (cassage…) » pour un cassage qui a survécu : corrige le document. 4. Rapport à Wian, **demande son accord**, fusionne seulement s'il dit « fusionne ».

### T1 — Auditer `markdown`, `shortcuts`, `reload-config` (issue 5)
- `cmux markdown <fichier>` : ouvre une surface navigateur à droite du terminal appelant. Scénario : vrai app + agent-browser (modèle : `tests/test_linux_real_project_view.py` et `tests/test_linux_real_browser_verbs.py`) : fichier `.md` avec un titre et une liste → la page rendue contient le titre en `<h1>` ; le focus reste sur le terminal ; + un cassage.
- `cmux reload-config` : scénario dans `tests/test_linux_shortcut_effect.py` (déjà prouvé pour un raccourci rebranché). Ajoute le cas « `cmux.json` invalide » : comportement attendu à vérifier **dans le code amont macOS** ; si incertain, demande à Wian.
- `cmux shortcuts` / `settings` : ouvrent la fenêtre Préférences (pas de page Raccourcis séparée dans ce port) ; ne teste que ce qui est observable.
- Complète `docs/AUDIT-TESTS.md`. Ajoute aussi des cassages pour `cookies get`, `config path`, `config set --file` (lignes « non » du tableau).

### T2 — Panneau Fichiers (F26) : scénario réel
Problème : aucune commande CLI ne lit le contenu du panneau. **Décision de produit à demander à Wian** avant de coder : (a) ajouter un accès en lecture seule (par ex. `cmux debug files`), ou (b) rester sur le test unitaire. Ne choisis pas seul.

### T3 — Lots 2 à 4 de `docs/CAMPAGNE-TESTS.md`
Prends **une fonctionnalité à la fois** (F01…F34, dans l'ordre du fichier, celles marquées « à faire »). Pour chacune : scénario réel en CI + cassage ; mets à jour `docs/VERIFICATION.md` et `docs/CAMPAGNE-TESTS.md`. Si le scénario révèle un écart avec macOS : **c'est un défaut à signaler à Wian** (explique-le simplement), pas à cacher en adaptant le test.

### T4 — Reliquats de l'issue 2
- **Demande d'abord à Wian** avant de supprimer les anciens tests d'identité (`network_verbs_translate_to_daemon_actions`, `offline_and_geolocation_translate_to_daemon_actions` dans `src/browser.rs`) : c'est retirer des tests.
- `network route --resource-type` : ajouter un contrôle + cassage.

### T5 — Issues GitHub
Quand une issue est traitée : commente-la avec le rapport (avant/après, liens de runs) et **propose** à Wian de la fermer. Issue 10 (régressions CI) : cause connue (tests périmés, voir §7), à commenter/fermer. Issues 2, 3, 5 : à fermer après T1 et T4.

### T6 — Tri des changements macOS depuis le point de fork
Le fork est épinglé à l'amont `7d78b6e` (début septembre) ; l'amont est en 0.64.25. Compare les changements amont (`git log 7d78b6e..HEAD` dans le clone amont), classe-les : **à reprendre / déjà présent / sans objet**, et **propose à Wian les améliorations une par une**, sans rien implémenter avant son « oui ».

---

## 7. Ce que la campagne a déjà appris (évite de refaire ces erreurs)

- Les échecs de l'issue 10 venaient de **tests périmés** après des changements voulus alignés sur macOS (une notification par terminal ; `send-key` avec noms de touches ; résumé de 180 caractères pour les hooks), pas de régressions produit.
- **Le titre d'un terminal change tout seul** : compare les surfaces avec `app.layout()` (sans titre), jamais avec `app.surfaces()` brut.
- Le premier espace de travail n'a pas de nom explicite : il suit le titre du terminal. Pour tester le titre (OSC 0), garde le shell **occupé** (`printf ...; sleep 30`) sinon le shell remet son propre titre.
- `xdotool type` sous Openbox est **instable** : ne conclus pas à un bug. Pour écrire dans un terminal, utilise `cmux send` (socket). `Ctrl+N` ouvre un **dialogue** ; `Ctrl+D` = diviser à droite **et** EOF (choix du fork). `Alt+A` d'approbation exige l'onglet Terminal des Préférences (`ctrl+Next` d'abord).
- Le daemon agent-browser 0.38.1 : `set geo` n'accorde pas la permission → action `permissions` avec `{"permissions":["geolocation"]}`. Les tests navigateur utilisent `CMUX_TEST_AGENT_BROWSER` (chemin du binaire 0.38.1) ; la CI principale a encore 0.31.1 pour les autres étapes.
- Le workflow `workflow_dispatch` n'est pas disponible pour les nouveaux workflows tant qu'ils ne sont pas sur la branche par défaut : ils se lancent par **chemins de PR**.
- Les tests de longue durée (mémoire, benchmarks) sont lents mais stables ; une étape « real browser » qui échoue après plus de 15 s est le vieux problème de délai (corrigé).

---

## 8. Quand tu ne sais pas

Écris à Wian, en français simple, **5 lignes maximum** : ce que tu as essayé, ce que tu as vu (copie la ligne brute), ce que tu proposes, et ta question. Attends sa réponse. C'est toujours mieux qu'un test « qui passe » sans preuve.
