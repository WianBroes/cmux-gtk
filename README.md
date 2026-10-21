<h1 align="center">cmux GTK — parity fork</h1>
<p align="center">cmux for Linux, aiming at feature parity with the macOS original: a Ghostty-powered terminal multiplexer for running AI coding agents side by side</p>

<!-- Screenshot to refresh: docs/assets/ still shows the nitecon interface (two title rows). -->

## What this repository is

`WianBroes/cmux-gtk` is a fork of [nitecon/cmux-gtk](https://github.com/nitecon/cmux-gtk), the Rust + GTK4 port of [cmux](https://github.com/manaflow-ai/cmux) (a macOS Swift/AppKit app by manaflow-ai).

The goal is different from nitecon's: **behave like cmux on macOS**, so that agents, skills, hooks and scripts written for the original work unchanged on Linux. The command is still `cmux`, the socket protocol is the original's, and macOS shortcuts map `⌘` to `Ctrl`.

How parity is checked:

- **The original's own tests are the oracle.** The socket test suite of manaflow-ai/cmux (`tests_v2/`, 141 tests) is replayed against this build, one throwaway instance per test. A skipped test never counts as passing.
- **The interface is inventoried against upstream** (menus, title row, sidebar, tabs, panes, notifications: 488 elements), each with its state here and the file implementing it.
- **A feature is done when it is shown working**, not when a unit test passes: a real scenario that fails before the change, a test that fails when the feature is deliberately broken, and a second reviewer who tries to break it. Anything without such proof is marked "not verified" in the change log.

This fork no longer sends pull requests to nitecon (decision of 2026-09-28): the two code bases have diverged too far. Fixes useful to Ghostty or agent-browser are reported upstream.

### Branches

| Branch | Content |
|---|---|
| `local/wian` (default) | The build used every day. Only receives tested work. |
| `feat/lot1` | Work in progress on the current batch (menus, tabs, panes, notifications). |
| `main` | nitecon v0.2.1 plus a verification ledger of its features; not where development happens. |

## What differs from nitecon/cmux-gtk

About 190 commits since 2026-09-27. In short:

**Agents that survive**
- Agent sessions recorded by hooks are resumed automatically after a restart (toggle in Preferences). The resume binding is refreshed on every prompt, so a missed `SessionStart` is caught up.
- `cmux local-tmux`: sessions owned by a private tmux server that outlives cmux, with Preferences, list/status, attach, rename and kill — following upstream.
- Hooks fail open when the app is gone, as on macOS. Pi dialogs (confirm/select/input) are announced as notifications.

**Socket and CLI parity**
- Upstream handles everywhere (`window:N`, `workspace:N`, `surface:N`, `--id-format`), upstream parameter names (`workspace_id`, `surface_id`, `tab_id`) and response shapes.
- Workspace, tab and pane actions: `move-surface`, `reorder-surface`/`reorder-workspace`, `split-off`, `swap-pane`, `break-pane`, `join-pane`, `workspace-action`, `tab-action`, `rename-tab`, `move-tab-to-new-workspace`, `trigger-flash`, `surface-health`, `tree`.
- Agent I/O: `send` (line breaks run as Enter), `send-key` with named keys, `read-screen` as plain text, `new-split/new-pane/new-surface/new-workspace --command` without stealing focus.
- `cmux events`: a reconnectable event stream (workspaces, notifications, agent hooks).
- The `debug.*` methods the upstream tests rely on (`debug.type`, `debug.shortcut.*`, `debug.panel_snapshot`, `debug.terminal.render_stats`…).
- `cmux config` (JSONC read, validation against `cmux.schema.json`, set/unset keeping comments), `cmux settings`, `cmux shortcuts`, `reload-config`; the app reads `cmux.json` shortcuts over `config.toml`.

**Interface like macOS**
- A single title row: sidebar, notification bell, new-workspace menu, focus back/forward, the original's menu bar (cmux, File, Edit, View, Notifications, History, Window, Help), workspace name, file explorer toggle and window buttons. One embedded icon pack (Tabler).
- Sidebar: agent activity per workspace (spinner, unread badge, latest message, one status line per agent), adjustable width (120–260 px), no close cross on rows (close from the context menu).
- Notifications like upstream: one per terminal, focusing a terminal marks it read, one-line summaries of agent replies, a live Notifications menu (recent items, Toggle Unread, Mark All Read, Clear All). Closing a terminal or workspace purges its notifications.
- Tabs and panes: terminal titles on tabs, Close Tab, Close Other Tabs, Rename Tab, next/previous tab, reorder, Equalize Splits, Resize Pane, Zoom Pane.
- Find bar in the terminal (Edit › Find, on Ghostty's search), Ctrl+mouse-wheel text zoom with a percentage bubble, Preferences split into App, Terminal and Shortcuts pages (rebind, unbind, reset; keys reserved for terminals and agents are refused with a reason).
- File explorer in a right panel: tree of the workspace's folder, live updates, J/K/H/L navigation, `/` filter, context menu, drag a row onto a terminal to paste its path.

**Terminal input**
- Dead keys (through the GTK input method, and on Wayland without one).
- Touchpad scrolling scaled like Ghostty's GTK runtime, optional inverted scrolling.
- Files dropped on a terminal are pasted as shell-escaped paths.
- A Ghostty fix for lost pty writes (upstream `f948d420`) applied at build time from `patches/ghostty/`.

**Browser**
- The upstream browser socket methods are translated to [agent-browser](https://github.com/vercel-labs/agent-browser) actions, plus viewport, cookies, storage, init scripts and styles, downloads, offline, geolocation, trace, HAR and network routing.
- `cmux markdown <file>` renders a Markdown file in a browser surface.

**Verification**
- CI also runs on `local/wian`, every step runs even after a failure, real-daemon and real-binary scenarios, a mutation bench, and a ledger of what is verified (`docs/VERIFICATION.md`, `docs/NOS-MODIFICATIONS.md`).

## Build and run

There are no packages for this fork. The Homebrew tap, `.deb`/`.rpm` and the install script of nitecon/cmux-gtk install nitecon's version, not this one.

```bash
# Prerequisite: Rust toolchain (the setup script installs system libraries and Zig)
git clone --recurse-submodules https://github.com/WianBroes/cmux-gtk.git
cd cmux-gtk
./scripts/setup-linux-dev.sh   # dev dependencies, Ghostty patches, libghostty
cargo build --release --bin cmux --bin cmux-app
./target/release/cmux-app
```

`cmux` is the CLI, `cmux-app` the application. cmux reads your `~/.config/ghostty/config` for themes, fonts and colors.

Browser panes use an existing `agent-browser` from `CMUX_AGENT_BROWSER`, `PATH` or its package path. Without it, browser panes stay disabled and cmux prints the upstream installation command.

Desktop notifications use `notify-send` 0.8 or newer (`libnotify-bin` on Debian/Ubuntu, `libnotify` on Fedora/Arch). Messages stay in the app if desktop delivery fails.

## Keyboard shortcuts

macOS `⌘` becomes `Ctrl`. Shortcuts follow the **physical position of the key on a US keyboard**, like the original: on AZERTY, `Ctrl+]` is the key right of `P`. All of them can be changed in Preferences › Shortcuts or in `cmux.json`.

| Shortcut | Action |
|---|---|
| Ctrl+N / Ctrl+Shift+S | New workspace / new SSH workspace |
| Ctrl+1…9 | Go to workspace 1–9 |
| Ctrl+] / Ctrl+[ | Next / previous workspace |
| Ctrl+Shift+R | Rename workspace |
| Ctrl+Shift+W | Close workspace |
| Ctrl+D / Ctrl+Shift+D | Split right / split down |
| Ctrl+Shift+arrows | Focus pane in that direction |
| Ctrl+Alt+← / → | Focus back / forward |
| Ctrl+Shift+= | Equalize splits |
| Ctrl+Alt+Shift+arrows | Resize pane |
| Ctrl+Shift+Enter | Zoom pane / exit zoom |
| Ctrl+Shift+X | Close pane |
| Ctrl+W / Ctrl+Alt+T | Close tab / close other tabs in pane |
| Ctrl+R | Rename tab |
| Ctrl+Shift+] / [ | Next / previous tab |
| Ctrl+Alt+Shift+[ / ] | Move tab left / right |
| Ctrl+F, Ctrl+G | Find, find next |
| Ctrl+B / Ctrl+Alt+B | Toggle sidebar / file explorer |
| Ctrl+Shift+E | Focus the file explorer |
| Ctrl+Alt+U | Toggle unread |
| Ctrl+Shift+B / Ctrl+Shift+Q | Open / close browser |
| Ctrl+Shift+C / V | Copy / paste |

## Workspaces

**New Workspace** (`Ctrl+N`) chooses a local folder and an optional POSIX startup script, run in each new terminal and again after restart. **New SSH Workspace** (`Ctrl+Shift+S`) opens a daemon-backed SSH workspace (SSH config alias or `user@host`, key/agent authentication without prompts; prepare the daemon with `./scripts/install-cmuxd-remote.sh`). Splits and tabs stay remote; after a reconnect shells start fresh, so use `local-tmux` or a remote session manager for processes that must survive.

The whole layout (split tree, divider ratios, order, colors) is saved and restored. To recover the snapshot of the last normal launch, quit cmux and run `cmux-app --restore-previous-session`.

## CLI

```bash
cmux --help                         # all commands
cmux tree                           # windows, workspaces, panes and tabs
cmux new-split right --command htop # open beside the calling terminal, keep focus
cmux send --surface surface:2 'make test\n'
cmux read-screen --surface surface:2
cmux send-key --surface surface:2 ctrl-c
cmux events                         # stream of workspace, notification and hook events
cmux hooks setup                    # install hooks of every detected agent
cmux local-tmux list                # tmux-backed sessions that outlive cmux
cmux browser open example.com       # browser surface driven by agent-browser
cmux diff --unstaged                # review working-tree changes in a side pane
cmux markdown NOTES.md              # render a Markdown file
cmux config validate                # check cmux.json against the schema
cmux identify                       # instance info
```

Agent hooks exist for Claude Code, Codex, Grok, OpenCode, Pi, OMP, Campfire, Amp, Cursor, Gemini CLI, Kiro, Antigravity, Hermes Agent, Kimi, Rovo Dev, GitHub Copilot, CodeBuddy, Factory Droid and Qoder. Setup keeps unrelated provider configuration and binds resume and notification events to the originating terminal.

The socket is `$XDG_RUNTIME_DIR/cmux/cmux.sock` (override with `CMUX_SOCKET` or `--socket`): v2 JSON-RPC, wire-compatible with macOS cmux, with SO_PEERCRED authentication.

## Diagnostics

Logs: `$XDG_STATE_HOME/cmux/cmux.log` (normally `~/.local/state/cmux/cmux.log`, override with `CMUX_LOG`). `cmux diagnostics --json` reports process resources; `cmux --verbose ping` gives a trace ID to match CLI requests with GTK timings.

## Architecture

Rust, GTK4 (gtk4-rs), Ghostty through libghostty, tokio bridged to the GLib main loop, agent-browser over CDP, the Go remote daemon (`cmuxd-remote`) reused from macOS. See [Architecture](Docs/Architecture.md) and [Components](Docs/Components.md).

## Origin and credits

This code base has a long lineage, and parts of it come from other projects:

- **[manaflow-ai/cmux](https://github.com/manaflow-ai/cmux)** (Lawrence Chen and contributors, GPL-3.0-or-later): the original macOS app. Its history is in ours (about 1,400 commits). It is the reference for interface and behavior, and its `tests_v2` suite is replayed as the parity oracle (the tests are run, not copied).
- **[bradwilson331/cmux-linux](https://github.com/bradwilson331/cmux-linux)** (Brad): the first Linux port (March 2026), in our history.
- **[nitecon/cmux-gtk](https://github.com/nitecon/cmux-gtk)** (Will Hattingh): the GTK4 port this fork starts from (v0.2.1, September 2026).
- **[douglas/cmux-gtk](https://github.com/douglas/cmux-gtk)** (Douglas Soares de Andrade, AGPL-3.0): the file explorer tree structure (lazy loading with a placeholder row, `TreeStore` columns in `src/file_explorer.rs`) follows his implementation.
- **[Ghostty](https://github.com/ghostty-org/ghostty)** (MIT): the terminal engine, through the submodule nitecon/ghostty ← manaflow-ai/ghostty ← ghostty-org/ghostty.
- **[agent-browser](https://github.com/vercel-labs/agent-browser)** (Vercel Labs): the browser daemon, used as an external tool.
- **[Tabler Icons](https://github.com/tabler/tabler-icons)** (MIT), four **[Adwaita](https://gitlab.gnome.org/GNOME/adwaita-icon-theme)** symbolic icons as fallback, **[Bonsplit](https://github.com/almonk/bonsplit)** (MIT).

Full license texts: [`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md).

## License

GNU Affero General Public License v3.0 or later (`AGPL-3.0-or-later`), see `LICENSE`.
