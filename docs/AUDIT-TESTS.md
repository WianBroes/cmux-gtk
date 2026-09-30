# Audit des tests des commandes navigateur et de configuration (issue 5)

Méthode : chaque commande est jouée contre le vrai programme (vrai daemon agent-browser 0.38.1 et vrai Chromium pour le navigateur ; vrai binaire `cmux` et vrais fichiers pour la configuration). Le contrôle regarde l'**effet réel** (ce que voit la page, ce qui est écrit sur disque, le code de sortie). La preuve « le test discrimine » consiste à casser une ligne du produit et à exiger que le test échoue.

Scénarios : `tests/test_linux_real_browser_verbs.py` (banc `tests/browser_mutation_cases.json`, workflow « Browser mutation »), `tests/test_linux_markdown_render.py` (sans application ni navigateur : juge la page écrite sur disque ; banc `tests/mutation_cases.json`, étape CI « Test the page cmux markdown writes »), `tests/test_linux_real_markdown_view.py` (vraie page dans un vrai Chromium ; étape CI « Test real browser markdown surface DOM ») et `tests/test_linux_config_cli.py` (banc `tests/mutation_cases.json`).

| Commande | Preuve réelle | Test discriminant | Trou |
|---|---|---|---|
| `browser geolocation` | la page lit la position (`48.85, 2.35`) ; **défaut trouvé et corrigé** (permission jamais accordée) | oui (cassage : mauvaise permission) | aucun |
| `browser offline on/off` | la requête de la page échoue puis remarche | oui | aucun |
| `browser network route --abort` | la requête de la page échoue | oui (cassage de `route`) | `--resource-type` non testé |
| `browser network route --body` | la page reçoit le contenu simulé | oui (cassage de `route`) | aucun |
| `browser network unroute` | la vraie réponse revient | oui | aucun |
| `browser trace start/stop` | fichier de trace non vide (200 Ko) | oui (cassage de `trace stop`) | contenu du fichier non analysé |
| `browser har start/stop` | fichier HAR valide contenant la requête | oui (cassage de `har stop`) | aucun |
| `browser viewport` | la page voit 800 par 600 | oui | `viewport reset` volontairement refusé, non testé |
| `browser cookies set` | la page voit le cookie | oui | options `--domain`, `--secure`, `--sameSite`, `--expires` non testées |
| `browser cookies get` | le cookie est listé | oui (cassage `verb-cookies-get`) | aucun |
| `browser cookies clear` | la page n'a plus de cookie | oui | aucun |
| `browser storage local` | la clé stockée est renvoyée | oui | `storage session` non testé |
| `browser addstyle` | le style de la page change | oui | aucun |
| `browser addscript` | le script tourne dans la page | oui | aucun |
| `browser addinitscript` | le script s'exécute avant ceux de la page après un rechargement | oui | aucun |
| `browser download` | le lien cliqué est enregistré dans le fichier | oui | `download-wait` non testé |
| `config path` / `settings path` | les deux affichent le même chemin isolé | oui (cassage `config-path-location`) | aucun |
| `config set` | valeur écrite, commentaires et clés voisines conservés | oui (cassage de la valeur écrite) | types non chaîne (`true`, `12`) non testés |
| `config get` | valeur relue | oui (cassage du chemin lu) | chemin absent non testé |
| `config unset` | valeur retirée, puis « unchanged » | oui (cassage : unset sans effet) | aucun |
| `config validate` | code 0 si valide, 1 si valeur hors liste, échec si JSON cassé | oui (cassage du code de sortie) | avertissements (code 0) non testés |
| `config set --file` | seul le fichier nommé est écrit | oui (cassage `config-explicit-file`) | `config get --file` sans cassage dédié |
| `config list-supported`, `config docs` | affichent leur contenu | non | contenu vérifié minimalement |
| `markdown` | la page écrite porte le titre du fichier en `<h1>`, sa liste en `<li>`, un seul lien cliquable (le `javascript:` reste du texte), aucun `<script>` ; la même page est ensuite rendue dans un vrai Chromium, où le terminal garde le focus | oui (cassage `markdown-heading`) | titres de niveau 2 et suivants, tableaux, notes de bas de page non testés |
| `settings` / `shortcuts` | les deux ouvrent **une seule** fenêtre Préférences (ce port n'a pas de page Raccourcis séparée) : la deuxième commande ramène la fenêtre déjà ouverte au premier plan, et la réponse de la commande dit `opened` ; **défaut trouvé et corrigé** (la deuxième commande empilait une seconde fenêtre) | oui (cassage `settings-window-action`) | contenu des onglets non vérifié (ni lisible par le gestionnaire de fenêtres ni par AT-SPI : `GTK_A11Y=none`) |

## Pas encore audité

`reload-config` en cas de fichier cassé (l'effet d'un raccourci changé puis rechargé est prouvé par `tests/test_linux_shortcut_effect.py`, mais pas ce que devient le raccourci déjà appliqué quand le fichier ne se relit plus ; le comportement voulu doit être tranché).

## Défauts par gravité

1. **Corrigé** : `browser geolocation` répondait « succès » alors que la page ne pouvait pas lire la position.
2. **Corrigé** : demander les Préférences deux fois (`cmux settings` puis `cmux shortcuts`, ou `Ctrl+,`) empilait une deuxième fenêtre Préférences au lieu de ramener celle déjà ouverte. `src/preferences.rs` garde maintenant la fenêtre vivante et la remet au premier plan.
3. Aucun autre défaut de produit trouvé sur les commandes auditées ci-dessus.
4. **Ouvert, à trancher** : `cmux.json` invalide. D'après la lecture du code (non vérifié en CI), au rechargement le fichier illisible est signalé sur la sortie d'erreur puis ignoré, donc la carte des raccourcis est reconstruite **sans** lui : un raccourci posé par `cmux.json` reviendrait à sa valeur par défaut au lieu de rester tel quel. Comportement voulu à confirmer (voir `docs/REPRISE.md`, T1).
