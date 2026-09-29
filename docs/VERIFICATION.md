# Verification ledger

Status values: `vérifié` (replayable proof named), `cassé` (issue linked), `non vérifiable ici` (reason),
`à vérifier` (no real-usage scenario yet). Proof is a CI step in `.github/workflows/ci.yml` that runs the
real application under Xvfb; a unit test alone never counts. Rows are updated only after a green run.

| Fonction | Scénario | Statut | Preuve |
| --- | --- | --- | --- |
| `send-text` / `send-key` / `read-text` | Commande envoyée à un shell réel (focalisé et en arrière-plan), sortie relue, focus inchangé | vérifié (step CI vert, log du step non relu : le proxy bloque le téléchargement) | `tests/test_linux_agent_terminal_io.py`, [run 36589404657](https://github.com/WianBroes/cmux-gtk/actions/runs/36589404657) step 26. Pas encore de preuve par mutation. |
| `set-status` / `set-progress` | Métadonnées d'un workspace en arrière-plan, limites, restauration | à confirmer sur le dernier run CI | `tests/test_linux_sidebar_metadata.py` |
| Notifications OSC 9/99/777 | Séquences émises par un vrai terminal | à confirmer sur le dernier run CI | `tests/test_linux_osc_notifications.py` |
| Notifications (CLI, panneau, non lu) | `notify`, lecture/effacement | à confirmer sur le dernier run CI | `tests/test_linux_notifications.py`, `tests/test_linux_desktop_notifications.py` |
| Hooks d'agents (Stop/Notification/SessionStart/SessionEnd), Claude | Commandes de hook réellement installées puis exécutées avec le JSON natif ; notifications relues par `notifications list` (corps tronqué à 8 Kio, routage vers la bonne surface, focus inchangé) ; `SessionEnd` d'une autre session refusé ; binding de reprise créé puis effacé | vérifié pour Claude (CI vert). Un binaire `claude` factice remplace l'agent réel : l'agent réel n'a pas été lancé (non vérifiable ici). Autres agents : à confirmer, tests non relus. Pas de preuve par mutation. | `tests/test_linux_claude_hooks.py`, [run 36601225375](https://github.com/WianBroes/cmux-gtk/actions/runs/36601225375) |
| Reprise de session / `surface.resume` | Binding enregistré sans exécution, refus hors du terminal propriétaire, exécution dans le PTY avec environnement littéral (les secrets ne sont pas persistés) | vérifié (CI vert). Pas de preuve par mutation. Reprise après quit/réouverture : `tests/test_linux_session_quit.py`, non relu | `tests/test_linux_resume.py`, [run 36601225375](https://github.com/WianBroes/cmux-gtk/actions/runs/36601225375) |
| Ports d'écoute | Attribution aux descendants du terminal | à confirmer sur le dernier run CI | `tests/test_linux_ports.py` |
| Branche/PR/écart git | Métadonnées git automatiques | à confirmer sur le dernier run CI | `tests/test_linux_git_metadata.py` |
| Workspaces, groupes, reorder | Groupes, ordre, déplacement | à confirmer sur le dernier run CI | `tests/test_workspace_groups.py`, `tests/test_linux_workspace_reorder_many.py`, `tests/test_linux_surface_move.py` |
| Panes/splits, focus | Routage imbriqué, fermeture | à confirmer sur le dernier run CI | `tests/test_linux_nested_split_routing.py`, `tests/test_linux_terminal_pane_close.py` |
| Navigateur intégré | Cycle de vie asynchrone | à confirmer ; verbes réseau/géolocalisation : voir issues #2, #3, #5 | `tests/test_linux_browser_lifecycle.py` |
| Diff / projet / markdown | Diff dans un vrai Chromium (clic sur un fichier), surface projet, ressources de navigateur distant | vérifié pour le diff et le projet : échouait avant le correctif (runs 36589404657, 36592529751, 36593154132), vert après (run 36597934911, tous les steps exécutés). Markdown : non couvert ici | `tests/test_linux_real_diff_viewer.py`, `tests/test_linux_real_project_view.py`, [run 36597934911](https://github.com/WianBroes/cmux-gtk/actions/runs/36597934911) |
| Config et raccourcis | Effet clavier en fenêtre principale | à vérifier : voir issue #4 | — |
| SSH / mosh | Daemon distant | à vérifier (mosh : non vérifiable sans hôte distant) | `tests/test_ssh_remote_daemon_resize_stdio.py` |
