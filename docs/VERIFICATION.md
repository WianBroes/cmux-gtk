# Verification ledger (fork `local/wian`)

Statuses: `vérifié` (replayable proof named), `cassé` (issue linked), `non vérifiable ici` (reason), `non vérifié` (no proof yet on the fork).
Proof is a CI step in `.github/workflows/ci.yml` that runs the real application under Xvfb; a unit test alone never counts.
**Every row starts as `non vérifié` on this branch.** A row becomes `vérifié` only after a green CI run on the fork's code, with its link, and after the mutation bench (`tests/mutation_cases.json`) shows its test fails when the function is broken.

Earlier results were obtained on `main` (upstream), not on this fork, and do not transfer.

| Fonction | Scénario | Statut | Preuve |
| --- | --- | --- | --- |
| `send-text` / `send-key` / `read-text` | Commande envoyée à un shell réel (focalisé et en arrière-plan), sortie relue, focus inchangé | non vérifié sur le fork | `tests/test_linux_agent_terminal_io.py` step 26. Pas encore de preuve par mutation. |
| `set-status` / `set-progress` | Statut et progression d'un workspace en arrière-plan via le vrai CLI sans changer le focus, formats plain/markdown, refus des entrées invalides (valeur trop longue, couleur, URL `file:`, id inconnu), limites de clés et de blocs, état conservé au redémarrage | non vérifié sur le fork | `tests/test_linux_sidebar_metadata.py` |
| Notifications OSC 9/99/777 | Vraies séquences OSC émises par un vrai terminal d'un onglet inactif : rafale, morceaux OSC 99 (base64, 1000 octets), trame surdimensionnée ignorée puis message suivant accepté, aucun changement de focus, métriques d'analyse | non vérifié sur le fork | `tests/test_linux_osc_notifications.py` |
| Notifications (CLI, panneau, non lu) | `notify`, marquer lu, ignorer, effacer, panneau ouvert qui se met à jour en direct, navigation par raccourci Alt+J, historique conservé après quit normal, borné à moins de 128 entrées | non vérifié sur le fork | `tests/test_linux_notifications.py` |
| Hooks d'agents (Stop/Notification/SessionStart/SessionEnd) | Commandes de hook réellement installées puis exécutées avec le JSON natif, état relu dans l'application. Claude : notifications tronquées à 8 Kio, routage, focus inchangé, `SessionEnd` d'une autre session refusé, binding de reprise créé puis effacé. Codex, Cursor, Pi, Amp, Rovo Dev, OpenCode, Kiro, Antigravity, Hermes, Kimi, OMP, Campfire : un événement Stop produit une notification lue par `notifications list` (lu en détail pour Codex, Cursor, Pi, Amp, Rovo Dev ; les autres seulement survolés) | non vérifié sur le fork | `tests/test_linux_*_hooks.py` |
| Reprise de session / `surface.resume` | Binding enregistré sans exécution, refus hors du terminal propriétaire, exécution dans le PTY avec environnement littéral (les secrets ne sont pas persistés) | non vérifié sur le fork | `tests/test_linux_resume.py` |
| Ports d'écoute | Écouteur d'un vrai enfant de terminal attribué, effacé à la sortie, sans sélectionner son workspace | non vérifié sur le fork | `tests/test_linux_ports.py` |
| Branche/dirty git | Découverte automatique sur de vrais dépôts isolés (51 assertions) | non vérifié sur le fork | `tests/test_linux_git_metadata.py` |
| Workspaces, groupes, reorder | Identité et appartenance de groupe persistantes, repli, réordonnancement atomique par lot avec plan à blanc et ordre après redémarrage, déplacement de surfaces et glisser vers un split | non vérifié sur le fork | `tests/test_workspace_groups.py`, `tests/test_linux_workspace_reorder_many.py`, `tests/test_linux_surface_move.py` |
| Panes/splits, focus | Routage PTY dans des panes imbriqués, fermeture d'un pane qui préserve ses voisins et libère exactement son enfant PTY | non vérifié sur le fork | `tests/test_linux_nested_split_routing.py`, `tests/test_linux_terminal_pane_close.py` |
| Navigateur intégré | Démarrage sans vol de focus, complétion différée (29 assertions) ; navigation réelle dans un vrai Chromium (voir la ligne Diff) | non vérifié sur le fork | `tests/test_linux_browser_lifecycle.py` |
| Diff / projet / markdown | Diff dans un vrai Chromium (clic sur un fichier), surface projet, ressources de navigateur distant | non vérifié sur le fork | `tests/test_linux_real_diff_viewer.py`, `tests/test_linux_real_project_view.py` |
| Config et raccourcis | Effet clavier en fenêtre principale | non vérifié sur le fork | — |
| SSH / mosh | Vrai SSH et PTY, héritage de script, ordre des workspaces, aller-retour de session | non vérifié sur le fork | `tests/test_linux_workspace_launch.py` |
| Session, quit, scrollback | Sauvegarde finale à la fermeture immédiate, récupération d'un workspace fermé, historique stylé conservé sur deux redémarrages | non vérifié sur le fork | `tests/test_linux_session_quit.py`, `tests/test_linux_previous_session.py`, `tests/test_linux_scrollback_restore.py` |
| État de la fenêtre | Position, taille et maximisation conservées après redémarrage, sous Openbox avec `xdotool`/`wmctrl` (aucun `assert`, mais chaque condition est attendue avec délai) | non vérifié sur le fork | `tests/test_linux_window_state.py` |
| `hook-stop-notification` | a Claude Stop/Notification hook creates an in-app notification | non vérifié sur le fork | KILLED |
| `hook-body-limit` | hook notification bodies are bounded to 8 KiB | non vérifié sur le fork | KILLED |
| `sidebar-status-limit` | a workspace holds at most 32 status entries | non vérifié sur le fork | KILLED |
| `osc-body-limit` | chunked OSC 99 bodies up to 8 KiB are assembled | non vérifié sur le fork | KILLED |
| `resume-secret-filter` | secret-like environment variables are not persisted in a resume binding | non vérifié sur le fork | KILLED |
| `send-key-field` | send-text and send-key reach the shell as typed input | non vérifié sur le fork | KILLED |
| `reorder-dry-run` | a dry-run reorder does not change workspace order | non vérifié sur le fork | KILLED |
| `window-maximized` | the maximized state survives a restart | non vérifié sur le fork | KILLED |
| `window-size-restore` | window size is restored after a restart | non vérifié sur le fork | KILLED |
| `git-untracked-dirty` | untracked files mark a workspace dirty | non vérifié sur le fork | KILLED |
| `group-collapse` | a workspace group can be collapsed | non vérifié sur le fork | KILLED |
| `ports-attribution` | a listener is attributed to its own terminal | non vérifié sur le fork | KILLED |

## Preuve par mutation

Banc : `tests/mutation_cases.json`, `tests/run_mutation.py`, `.github/workflows/mutation.yml` (12 cassages). Résultats sur le fork : à lancer (workflow `Mutation`, manuel ou sur PR touchant ces fichiers).
