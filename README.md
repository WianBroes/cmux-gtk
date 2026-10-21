<h1 align="center">cmux GTK — parity fork</h1>
<p align="center">cmux for Linux, aiming at feature parity with the macOS original: a Ghostty-powered terminal multiplexer for running AI coding agents side by side</p>

<!-- Screenshot to refresh: docs/assets/ still shows the nitecon interface (two title rows). -->

## What this repository is

`WianBroes/cmux-gtk` is a fork of [nitecon/cmux-gtk](https://github.com/nitecon/cmux-gtk), the Rust + GTK4 port of [cmux](https://github.com/manaflow-ai/cmux) (a macOS Swift/AppKit app by Manaflow).

The goal is different from nitecon's: **behave like cmux on macOS**, so that agents, skills, hooks and scripts written for the original work unchanged on Linux. The command is still `cmux`, the socket follows the original's v2 JSON-RPC protocol, and macOS shortcuts mostly map `⌘` to `Ctrl` (a few differ where keys collide).

How parity is checked:

- **The original's own tests are the oracle.** Upstream's socket test suite (`tests_v2/` of manaflow-ai/cmux, 141 tests — not vendored here; the `tests_v2/` folder of this repository is a few upstream tests kept by nitecon) is replayed against this build, one throwaway instance per test. A skipped test never counts as passing. Parity is a goal, not a state: many of those tests still fail, and the remaining gaps are tracked against them.
- **The interface is inventoried against upstream**: menu bar, title row, context menus and settings, 488 elements, each with its state here and the file implementing it.
- **A feature is done when it is shown working**, not when a unit test passes: a real scenario that fails before the change, a test that fails when the feature is deliberately broken, and, before merging, a second reviewer who tries to break it.

This fork no longer sends pull requests to nitecon (decision of 2026-09-28): the two code bases have diverged too far. Fixes useful to agent-browser are proposed upstream.

### Branches

| Branch | Content |
|---|---|
| `local/wian` (default) | The build used every day. Only receives tested work. |
| `feat/lot1` | Work in progress on the current batch (menus, title row, tabs, panes, notifications). Merged into `local/wian` once tested. |
| `main` | nitecon v0.2.1 plus a verification ledger, a mutation bench and one diff-viewer fix; not where development happens. |

## What differs from nitecon/cmux-gtk

About 190 commits since 2026-09-27. In short:

**Agents that survive**
- Agent sessions recorded by hooks are resumed automatically when cmux reopens (toggle in Preferences). The resume binding is refreshed on every prompt, so a missed `SessionStart` is caught up.
- `cmux local-tmux` (`start`, `attach`, `list`, `status`, `detach`, `close`) and its Preferences section, following upstream: local sessions owned by a private tmux server that outlives cmux. The terminal menu adds Split Right in tmux, Rename tmux Session and Kill tmux Session (Linux additions).
- Hooks fail open when the app is gone, as on macOS. Pi dialogs (confirm/select/input) are announced as notifications.

**Socket and CLI parity**
- Upstream handles everywhere (`window:N`, `workspace:N`, `surface:N`, `--id-format`), upstream parameter names (`workspace_id`, `surface_id`, `tab_id`) and response shapes.
- Workspace, tab and pane actions: `move-surface`, `reorder-surface` and `reorder-workspace` with upstream placement options (`--before/--after/--index/--window`, and `--focus` on surfaces), and new `split-off`, `swap-pane`, `break-pane`, `join-pane`, `workspace-action`, `tab-action`, `rename-tab`, `move-tab-to-new-workspace`, `trigger-flash`, `surface-health`, `tree`.
- Agent I/O: `send` (line breaks run as Enter), `send-key` with named keys, `read-screen` as plain text, `new-split/new-pane/new-surface/new-workspace --command` without stealing focus.
- `cmux events`: a reconnectable event stream (workspaces, notifications, agent hooks).
- The `debug.*` methods the upstream tests rely on (`debug.shortcut.*`, `debug.panel_snapshot`, `debug.terminal.render_stats`, `debug.terminal.is_focused`), and `debug.type` now types text like upstream instead of pasting it.
- `cmux config` (JSONC read, validation against `cmux.schema.json`, set/unset keeping comments), `cmux settings`, `cmux shortcuts`, `reload-config`; the app reads `cmux.json` shortcuts over `config.toml`.
- Workspace descriptions (`set-description`, `clear-description` and an in-app editor).

**Interface like macOS**
- A single title row: sidebar, notification bell, new-workspace menu, focus back/forward, the original's menu bar (cmux, File, Edit, View, Notifications, History, Window, Help), workspace name, file explorer toggle and window buttons. One embedded icon pack (Tabler).
- Sidebar: agent activity per workspace (spinner, unread badge, latest message, one status line per agent), draggable width with an adjustable minimum (120–260), no close cross on rows (close from the context menu).
- Notifications like upstream: one per terminal, focusing a terminal marks it read, one-line summaries of agent replies, a live Notifications menu (recent items, Toggle Unread, Mark All Read, Clear All), a switch for desktop notifications. Closing a terminal or workspace purges its notifications.
- Tabs and panes: terminal titles and the running agent's mark on tabs, Close Tab, Close Other Tabs, Rename Tab, next/previous tab, reorder, Equalize Splits, Resize Pane, Zoom Pane, per-pane split buttons, a tab dropped on a tab strip joins that pane.
- Find bar in the terminal (Edit › Find, on Ghostty's search), Preferences split into App, Terminal and Shortcuts pages (rebind, unbind, reset; keys reserved for terminals and agents are refused with a reason).
- File explorer in a right panel: tree of the workspace's folder, live updates, J/K/H/L navigation, `/` filter, context menu, drag a row onto a terminal to paste its path.

**Linux additions** (not in the original)
- Ctrl+mouse-wheel text zoom with a percentage bubble.
- Split Right in tmux, Rename tmux Session and Kill tmux Session in the terminal menu.

**Terminal input and robustness**
- Dead keys (through the GTK input method, and on Wayland without one).
- Touchpad scrolling scaled like Ghostty's GTK runtime, optional inverted scrolling.
- Files dropped on a terminal are pasted as shell-escaped paths, as upstream.
- A Ghostty fix for lost pty writes (upstream `f948d420`) applied at build time from `patches/ghostty/`.
- Background terminals start when they are created, as upstream; a terminal that fails to start no longer closes the whole app.

**Browser**
- The upstream browser socket methods are translated to [agent-browser](https://github.com/vercel-labs/agent-browser) actions, plus viewport, cookies, storage, init scripts and styles, downloads, offline, geolocation, trace, HAR and network routing. `download-wait` never completes with agent-browser up to 0.38.1 (a fix is in review upstream).
- `cmux markdown <file>` renders a Markdown file in a browser surface.

**Verification**
- CI also runs on `local/wian`, every step runs even after a failure, real-daemon and real-binary scenarios, a mutation bench.

## Things to know

- **Ctrl+W and Ctrl+R belong to cmux** (Close Tab, Rename Tab), so shells lose word-erase and history search on those keys. Rebind them in Preferences › Shortcuts if you need them in the shell.
- **Right-click always opens the cmux terminal menu**, even in programs that use the mouse (unlike macOS, where the program gets it).
- **`cmux update` is disabled**: this fork publishes no releases yet (the inherited updater would have installed nitecon's). Update from source: `git pull`, `./scripts/setup-linux-dev.sh`, then rebuild. `scripts/install-linux.sh` stops for the same reason.
- Not yet ported: multiple windows, upstream's tmux compatibility layer, parts of the browser engine. The file explorer's interface is in French.

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

This builds `local/wian`. To try the batch in progress before it is merged, add `-b feat/lot1` to the clone.

`cmux` is the CLI, `cmux-app` the application. cmux reads your `~/.config/ghostty/config` for themes, fonts and colors.

Browser panes use an existing `agent-browser` from `CMUX_AGENT_BROWSER`, `PATH` or its package path. Without it, browser panes stay disabled and cmux prints the upstream installation command.

Desktop notifications use `notify-send` 0.8 or newer (`libnotify-bin` on Debian/Ubuntu, `libnotify` on Fedora/Arch). Messages stay in the app if desktop delivery fails.

## Keyboard shortcuts

Bracket and `=` shortcuts follow the key's position on a US keyboard when your layout has no such key: on AZERTY, `Ctrl+[` is the key right of `P` (`^`) and `Ctrl+]` the one after it (`$`). Other keys follow your layout.

Where to change a shortcut: **P** = Preferences › Shortcuts or `cmux.json`, **T** = `config.toml` only, **—** = fixed.

| Shortcut | Action | |
|---|---|---|
| Ctrl+N | New workspace | P |
| Ctrl+Shift+S | New SSH workspace | T |
| Ctrl+1…9 | Go to workspace 1–9 | T |
| Ctrl+] / Ctrl+[ | Next / previous workspace | P |
| Ctrl+Shift+Page Up / Down | Move workspace up / down | P |
| Ctrl+Alt+. | Collapse / expand workspace group | P |
| Ctrl+Shift+R | Rename workspace | P |
| Ctrl+Shift+W | Close workspace | P |
| Ctrl+T / Ctrl+Shift+L | New terminal tab / new browser tab | — |
| Ctrl+D / Ctrl+Shift+D | Split right / split down | P |
| Ctrl+Shift+arrows | Focus pane in that direction | P |
| Ctrl+Alt+← / → | Focus back / forward | P |
| Ctrl+Shift+= | Equalize splits | P |
| Ctrl+Alt+Shift+arrows | Resize pane | P |
| Ctrl+Shift+Enter | Zoom pane / exit zoom | P |
| Ctrl+Shift+X | Close pane | T |
| Ctrl+W / Ctrl+Alt+T | Close tab / close other tabs in pane | P |
| Ctrl+R | Rename tab | P |
| Ctrl+Shift+] / [ | Next / previous tab | P |
| Ctrl+Alt+Shift+[ / ] | Move tab left / right | P |
| Ctrl+F | Find | — |
| Ctrl+G / Ctrl+Alt+G | Find next / previous (find bar open) | — |
| Ctrl+E | Use selection for find (with a selection) | — |
| Ctrl+Alt+Shift+F | Hide find bar | — |
| Ctrl+B | Toggle sidebar | P |
| Ctrl+Alt+B | Toggle file explorer | T |
| Ctrl+Shift+E | Focus the file explorer | P |
| Ctrl+Shift+I | Notifications | — |
| Ctrl+Shift+U | Jump to latest unread | — |
| Ctrl+Alt+U | Toggle unread | — |
| Ctrl+Shift+B / Ctrl+Shift+Q | Open / close browser | P / T |
| Ctrl+Shift+C / V | Copy / paste | — |
| Ctrl+, | Preferences | — |
| Ctrl+Q | Quit | — |

Selecting text also makes it available to middle-click paste (PRIMARY selection); Ghostty's `copy-on-select` still applies.

## Workspaces

**New Workspace** (`Ctrl+N`) chooses a local folder and an optional POSIX startup script, run in each new terminal and again after restart. **New SSH Workspace** (`Ctrl+Shift+S`) opens a daemon-backed SSH workspace (SSH config alias or `user@host`, key/agent authentication without prompts; prepare the daemon with `./scripts/install-cmuxd-remote.sh`). Splits and tabs stay remote; after a reconnect shells start fresh, so keep remote processes in tmux on the host, or use `cmux mosh-tmux`.

Sidebar rows can be dragged to reorder, moved with Move Up/Down, colored with Background Color. The whole layout (split tree, divider ratios, order, colors) and the window size are saved and restored. To recover the snapshot of the last normal launch, quit cmux and run `cmux-app --restore-previous-session` (the backup is `session.previous.json` beside `session.json`).

The terminal font size (6–72 points) is set in Preferences › Terminal; until it is set, terminals use their Ghostty configuration.

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
cmux markdown NOTES.md              # render a Markdown file
cmux config validate                # check cmux.json against the schema
cmux identify                       # instance info
```

Agent hooks exist for Claude Code, Codex, Grok, OpenCode, Pi, OMP, Campfire, Amp, Cursor, Gemini CLI, Kiro, Antigravity, Hermes Agent, Kimi, Rovo Dev, GitHub Copilot, CodeBuddy, Factory Droid and Qoder. Setup keeps unrelated provider configuration and binds resume and notification events to the originating terminal. `cmux claude-teams` opens Claude Code teammates as native cmux panes.

### Browser

```bash
cmux browser open example.com                 # returns a surface:N handle
cmux browser open gmail.com --profile Default # reuse an agent-browser Chrome profile
cmux browser surface:1 snapshot --interactive # accessibility tree with element refs
cmux browser surface:1 click e3
cmux browser surface:1 fill e5 "search term"
cmux browser surface:1 eval 'document.title'
cmux browser list
```

Browser commands print JSON by default (agents are the main users); add `--no-json` for human-readable output.

### Diff review

```bash
cmux diff --unstaged                # working-tree changes in a right-hand pane
git diff | cmux diff - --layout split
cmux diff --branch --base origin/main
cmux diff --last-turn               # changes since this agent's latest prompt
cmux comments add --file src/main.rs --line 42 --message "Handle this error"
cmux comments consume --all
cmux project .                      # manifest-aware project inspector
```

### Socket

The app listens on `$XDG_RUNTIME_DIR/cmux/cmux.sock` (v2 JSON-RPC, SO_PEERCRED authentication). The CLI connects to another socket with `CMUX_SOCKET` or `--socket`.

## Diagnostics

Logs: `$XDG_STATE_HOME/cmux/cmux.log` (normally `~/.local/state/cmux/cmux.log`, override with `CMUX_LOG`; rotates at 8 MiB). Desktop-launch output is also in `journalctl --user`. `cmux diagnostics --json` reports process resources; `cmux --verbose ping` gives a trace ID to match CLI requests with GTK timings.

## Architecture

Rust, GTK4 (gtk4-rs), Ghostty through libghostty, tokio bridged to the GLib main loop, agent-browser over CDP, the Go remote daemon (`cmuxd-remote`) reused from macOS. See [Architecture](Docs/Architecture.md) and [Components](Docs/Components.md).

## Origin and credits

This code base has a long lineage, and parts of it come from other projects:

- **[manaflow-ai/cmux](https://github.com/manaflow-ai/cmux)** (Manaflow, Inc. — Lawrence Chen and contributors; GPL-3.0-or-later for the app): the original macOS app. Its history is in ours (about 1,750 commits). It is the reference for interface and behavior, and its `tests_v2` suite is replayed as the parity oracle (the tests are run, not copied).
- **[bradwilson331/cmux-linux](https://github.com/bradwilson331/cmux-linux)** (Brad, AGPL-3.0): the first Linux port (March 2026), in our history.
- **[nitecon/cmux-gtk](https://github.com/nitecon/cmux-gtk)** (Will Hattingh, AGPL-3.0): the GTK4 port this fork starts from (v0.2.1, September 2026).
- **[douglas/cmux-gtk](https://github.com/douglas/cmux-gtk)** (Douglas Soares de Andrade, AGPL-3.0-or-later): the file explorer tree structure (lazy loading with a placeholder row, `TreeStore` columns in `src/file_explorer.rs`) follows his implementation.
- **[Ghostty](https://github.com/ghostty-org/ghostty)** (MIT): the terminal engine, through the submodule nitecon/ghostty ← manaflow-ai/ghostty ← ghostty-org/ghostty.
- **[agent-browser](https://github.com/vercel-labs/agent-browser)** (Vercel Labs, Apache-2.0): the browser daemon, used as an external tool.
- **[Bonsplit](https://github.com/almonk/bonsplit)** (MIT): the split tree is a Rust port of it.
- **[Tabler Icons](https://github.com/tabler/tabler-icons)** (MIT), and four **[Adwaita](https://gitlab.gnome.org/GNOME/adwaita-icon-theme)** symbolic icons as fallback (CC-BY-SA-3.0 or LGPL-3.0-only).

Full license texts: [`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md).

## License

GNU Affero General Public License v3.0 or later (`AGPL-3.0-or-later`), see `LICENSE`.
