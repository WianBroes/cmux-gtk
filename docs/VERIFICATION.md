# Verification ledger

Status values: `vérifié` (replayable proof named), `cassé` (issue linked), `non vérifiable ici` (reason),
`à vérifier` (no real-usage scenario yet). Proof is a CI step in `.github/workflows/ci.yml` that runs the
real application under Xvfb; a unit test alone never counts. Rows are updated only after a green run.

| Fonction | Scénario | Statut | Preuve |
| --- | --- | --- | --- |
| `send-text` / `send-key` / `read-text` | Commande envoyée à un shell réel (focalisé et en arrière-plan), sortie relue, focus inchangé | à vérifier (première exécution CI en attente) | `tests/test_linux_agent_terminal_io.py` |
| `set-status` / `set-progress` | Métadonnées d'un workspace en arrière-plan, limites, restauration | à confirmer sur le dernier run CI | `tests/test_linux_sidebar_metadata.py` |
| Notifications OSC 9/99/777 | Séquences émises par un vrai terminal | à confirmer sur le dernier run CI | `tests/test_linux_osc_notifications.py` |
| Notifications (CLI, panneau, non lu) | `notify`, lecture/effacement | à confirmer sur le dernier run CI | `tests/test_linux_notifications.py`, `tests/test_linux_desktop_notifications.py` |
| Hooks d'agents (Stop/Notification…) | Hooks par agent via le CLI | à confirmer sur le dernier run CI | `tests/test_linux_*_hooks.py` |
| Reprise de session / `surface.resume` | Reprise après quit/réouverture | à confirmer sur le dernier run CI | `tests/test_linux_resume.py`, `tests/test_linux_session_quit.py` |
| Ports d'écoute | Attribution aux descendants du terminal | à confirmer sur le dernier run CI | `tests/test_linux_ports.py` |
| Branche/PR/écart git | Métadonnées git automatiques | à confirmer sur le dernier run CI | `tests/test_linux_git_metadata.py` |
| Workspaces, groupes, reorder | Groupes, ordre, déplacement | à confirmer sur le dernier run CI | `tests/test_workspace_groups.py`, `tests/test_linux_workspace_reorder_many.py`, `tests/test_linux_surface_move.py` |
| Panes/splits, focus | Routage imbriqué, fermeture | à confirmer sur le dernier run CI | `tests/test_linux_nested_split_routing.py`, `tests/test_linux_terminal_pane_close.py` |
| Navigateur intégré | Cycle de vie asynchrone | à confirmer ; verbes réseau/géolocalisation : voir issues #2, #3, #5 | `tests/test_linux_browser_lifecycle.py` |
| Diff / projet / markdown | Viewers | à confirmer sur le dernier run CI | `tests/test_linux_diff_viewer.py`, `tests/test_linux_project_view.py` |
| Config et raccourcis | Effet clavier en fenêtre principale | à vérifier : voir issue #4 | — |
| SSH / mosh | Daemon distant | à vérifier (mosh : non vérifiable sans hôte distant) | `tests/test_ssh_remote_daemon_resize_stdio.py` |
