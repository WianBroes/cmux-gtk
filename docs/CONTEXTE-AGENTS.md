# cmux-gtk : contexte à lire avant de travailler

Ce fichier est fait pour un agent (Claude cloud ou autre) qui reprend le projet sans avoir vu l'historique. Il est écrit à la date du 2026-09-29. Ce qui est daté peut avoir changé : vérifie l'état réel (issues, CI, `git log`) avant de t'y fier.

## Le projet
- Dépôt **`WianBroes/cmux-gtk`** (public) : fork de `nitecon/cmux-gtk`, lui-même un portage Linux/GTK4 de cmux, un terminal pensé pour les agents de code (source macOS : `manaflow-ai/cmux`). Rust, GTK4, libghostty (compilée avec zig).
- **Branche de travail : `local/wian`**, qui est la branche par défaut. C'est le code réellement utilisé par Wian, avec 89 commits d'avance sur l'amont. `main` est l'amont v0.2.1 plus 3 commits poussés là par erreur par un agent cloud. **Ne travaille pas sur `main`.**
- Wian ne lit pas le Rust : il compte sur des **preuves rejouables**, jamais sur une affirmation.
- Ses instructions plus générales (`AGENTS.md`) sont dans ce dépôt.

## Ce qui a été fait et où le trouver
- **`docs/NOS-MODIFICATIONS.md`** : l'inventaire de nos 89 commits regroupés en 34 fonctionnalités (F01 à F34), avec pour chacune ses fichiers, ses tests, l'étape de CI qui l'exécute, un statut de preuve et **un scénario de vérification à écrire**. C'est ta liste de travail. Lis aussi sa section « Dépendances hors dépôt ».
- Répartition des preuves : 19 fonctionnalités avec un test unitaire seulement, 9 couvertes par un scénario de CI, 5 sans aucun test, 1 avec un test d'identité qui ne prouve rien (F30 : offline, geolocation, trace, har, network).
- Travail récent : l'application lit `cmux.json` (raccourcis) par-dessus `config.toml` ; page Preferences > Shortcuts (changer, débrancher, réinitialiser, avec refus des touches dont les terminaux et les agents ont besoin) ; verbes `cmux browser` (viewport, cookies, storage, addinitscript, addstyle, addscript, download, download-wait, offline, geolocation, trace, har, network) ; `cmux markdown`. Une partie a été écrite par des modèles gratuits puis relue : leurs tests se sont révélés faibles.

## Ce qu'on a constaté (vérifié)
1. **La première CI sur `local/wian` a échoué** (run 36600296691) : deux étapes qui **réussissent sur `main` échouent sur `local/wian`**, donc des régressions de nos commits :
   - « Test terminal pane close lifecycle » : un `split` vers une cible inconnue doit échouer sans rien changer, mais modifie la sélection ou la mise en page ;
   - « Test script and SSH workspace launch and restore » : `timed out waiting for workspace launch state`.
2. **Ce premier run avait sauté 64 étapes**, dont `cargo test --workspace` : nos 198 tests unitaires ajoutés n'avaient donc jamais tourné en CI. Depuis le commit `ebb74e37`, les étapes après le build s'exécutent même si une précédente échoue. Un run a été lancé avec ce workflow (numéro 36612708633) : **son résultat n'était pas connu au moment de l'écriture de ce fichier. Lis-le en premier** : il donne enfin le tableau complet des échecs.
3. Deux tests de traduction des verbes navigateur comparent une chaîne du test à la même chaîne du code : ils ne prouvent rien. Seuls des rejeux à la main ont montré que `offline` et `route` marchent.
4. `cmux browser geolocation` répond `success`, mais une page ne peut pas lire la position (la permission n'est jamais accordée).
5. `route --resource-type fetch` ne mocke rien : le daemon agent-browser classe `fetch()` en `xhr`.
6. Dans le banc local (Xvfb), les touches arrivent bien au terminal, mais les raccourcis configurables (`Ctrl+N`, `Ctrl+D`, `Ctrl+Maj+D`, contrôleur de capture de `src/shortcuts.rs`) ne font rien. Cause non établie. `cmux new-workspace` par le CLI marche.

## Règles de travail (à respecter)
**« Fini » = prouvé, pas « les tests passent ».**
1. **Le test d'abord, vu échouer** : le scénario réel (vrai daemon, vraie instance) est joué et échoue avant le code. Pour un bug : reproduis le symptôme d'abord. Colle la sortie avant et après.
2. **Le test doit discriminer** : casse volontairement la fonction (nom d'action, paramètre, règle) sur une copie, montre que le test échoue. Interdits : un test qui compare une chaîne du test à la même chaîne du code, une assertion « non nul », un test écrit après coup sur le seul chemin imaginé.
3. **L'auteur n'est pas le vérificateur** : liste ce que tu n'as pas pu essayer de faire échouer ; un autre agent tentera de casser ton résultat.
4. **Statut honnête** : sans preuve rejouable, écris « non vérifié », jamais « vérifié ».
5. Ne corrige pas un test pour qu'il passe : corrige le code. Ne désactive ni n'assouplis aucun test, aucune étape de CI.

Autres règles :
- Ne lance jamais `cargo fmt` sur tout le dépôt (il reformate tout). Pas de `--force`.
- **Dépôt public : aucun secret, e-mail, chemin personnel ou nom de machine** dans le code, les issues, les commits, la CI ou les logs.
- Ne pousse jamais directement sur `local/wian` ni sur `main` : une branche, une PR, fusion seulement si la CI est verte.
- Ne fusionne pas de travail qui touche `ci.yml` sans le relire.

## Issues ouvertes (`WianBroes/cmux-gtk`)
- **#10** — les 2 régressions de la CI. **À faire en premier.** Piste : la fonctionnalité F18 (références de socket, drapeaux `--workspace/--surface/--pane`) a réécrit la résolution des cibles ; hypothèse à confirmer par `git bisect` entre `e9c62ef` (passe) et `local/wian` (échoue).
- **#4** — l'effet clavier de la page Shortcuts ; pourquoi les raccourcis configurables sont muets dans le banc.
- **#1** — campagne de vérification de tout le fork : produire `docs/VERIFICATION.md`.
- **#2** — remplacer les tests d'identité de offline/network/trace/har par des tests de comportement.
- **#3** — `geolocation` sans permission. **#5** — audit des tests des autres commandes. **#6** — les 12 raccourcis restants. **#7** — documenter `fetch()` classé `xhr`. **#8** — CI et banc de scénarios (une CI existe déjà : ne la refais pas, étends-la). **#9** — le test du diff dans un vrai navigateur échoue en CI, y compris sur `main`.
- Commence par lire le dernier run de CI sur `local/wian` et par ouvrir **une issue par échec distinct** avant de corriger.

## Dépendances hors dépôt (à connaître pour tester)
- **agent-browser >= 0.38.1 patché** pour `download-wait` : sans le correctif (proposé en amont, `vercel-labs/agent-browser`, PR 2023, non fusionné à la date d'écriture), il échoue. Le binaire se choisit par la variable **`CMUX_AGENT_BROWSER`**. Le daemon 0.27.0 du PATH fait que `route` n'intercepte rien : n'y fais aucun essai.
- La configuration de l'utilisateur (`config.toml`, `cmux.json`, hooks) n'est pas dans le dépôt.
- Une CI existe : `.github/workflows/ci.yml` (Xvfb, openbox, xdotool, des dizaines de scénarios Python dans `tests/`). Le build à froid dure quelques minutes ; les caches sont configurés.

## Contraintes
- Compte GitHub gratuit, aucun budget. Dépôt public : les minutes de CI standard sont gratuites, la protection de branche est possible.
- Ne demande pas de créer des dépôts, de changer la visibilité, de supprimer ni d'installer quoi que ce soit sans que Wian l'ait dit.
- Sois concis dans tes rapports : dis ce que tu as fait, ce que tu as prouvé et comment, et ce que tu n'as **pas** pu établir.
