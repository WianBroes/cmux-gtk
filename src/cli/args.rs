//! Shared command schema for the CLI, completions and man-page generator.

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum DiffSource {
    Unstaged,
    Staged,
    Branch,
    LastTurn,
}

#[derive(Clone, Copy, Debug, serde::Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DiffLayout {
    Unified,
    Split,
}

/// Identity shape requested for JSON output through `--id-format`.
///
/// `Refs` and `Uuids` drop the redundant counterpart from every identity field
/// of a response; `Both` keeps `id` beside `ref`. Without the flag the response
/// is left untouched, so scripts keep reading stable UUIDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum IdFormat {
    /// Short `kind:N` references only
    Refs,
    /// Stable UUIDs only
    Uuids,
    /// References and UUIDs side by side
    Both,
}

#[derive(Subcommand)]
pub enum CommentCommands {
    /// List pending review comments for a Git repository
    List {
        #[arg(long)]
        repo: Option<std::path::PathBuf>,
        /// Include comments already marked consumed
        #[arg(long)]
        all: bool,
    },
    /// Add a durable line-anchored review comment
    Add {
        #[arg(long)]
        repo: Option<std::path::PathBuf>,
        #[arg(long)]
        file: String,
        #[arg(long, default_value = "new")]
        side: String,
        #[arg(long)]
        line: u32,
        #[arg(long)]
        end_line: Option<u32>,
        #[arg(long, default_value = "")]
        line_text: String,
        #[arg(long)]
        message: String,
    },
    /// Delete one review comment by UUID
    Delete {
        id: String,
        #[arg(long)]
        repo: Option<std::path::PathBuf>,
    },
    /// Mark selected or all pending comments as delivered to an agent
    Consume {
        ids: Vec<String>,
        #[arg(long)]
        repo: Option<std::path::PathBuf>,
        #[arg(long, conflicts_with = "ids")]
        all: bool,
    },
}

/// Parsed global flags and the selected terminal-multiplexer operation.
#[derive(Parser)]
#[command(name = "cmux", version = env!("CMUX_VERSION"), about = "Control cmux terminal multiplexer")]
pub struct Cli {
    /// Path to the cmux socket (overrides discovery; CMUX_SOCKET, then CMUX_SOCKET_PATH)
    #[arg(long, global = true, env = "CMUX_SOCKET")]
    pub(super) socket: Option<String>,

    /// Output raw JSON responses
    #[arg(long, global = true)]
    pub(super) json: bool,

    /// Shape identity fields in JSON output: refs, uuids or both
    #[arg(long, global = true, value_name = "FORMAT")]
    pub(super) id_format: Option<IdFormat>,

    /// Suppress JSON output for browser commands (browser defaults to JSON)
    #[arg(long, global = true)]
    pub(super) no_json: bool,

    /// Verbose output (connection info to stderr)
    #[arg(short, long, global = true)]
    pub(super) verbose: bool,

    /// Color mode: always, never, auto
    #[arg(long, global = true)]
    pub(super) color: Option<String>,

    #[command(subcommand)]
    pub(super) command: Commands,
}

/// Supported CLI operations, independent of socket transport and desktop state.
#[derive(Subcommand)]
pub enum Commands {
    /// Open a bounded patch or Git comparison in an agent-accessible diff surface
    Diff {
        /// Unified patch file, or '-' to read standard input
        input: Option<String>,
        /// Git source when no patch file is supplied
        #[arg(long, value_enum)]
        source: Option<DiffSource>,
        #[arg(long, conflicts_with_all = ["source", "staged", "branch", "last_turn", "input"])]
        unstaged: bool,
        #[arg(long, conflicts_with_all = ["source", "unstaged", "branch", "last_turn", "input"])]
        staged: bool,
        #[arg(long, conflicts_with_all = ["source", "unstaged", "staged", "last_turn", "input"])]
        branch: bool,
        #[arg(long, conflicts_with_all = ["source", "unstaged", "staged", "branch", "input"])]
        last_turn: bool,
        /// Destination workspace UUID; defaults to the caller or selected workspace
        #[arg(long)]
        workspace: Option<String>,
        /// Place the viewer immediately to the right of this surface UUID
        #[arg(long)]
        surface: Option<String>,
        /// Select a provider session-specific last-turn baseline
        #[arg(long, alias = "agent-session")]
        session: Option<String>,
        /// Repository or child path used by Git sources
        #[arg(long, alias = "repo", alias = "path")]
        cwd: Option<std::path::PathBuf>,
        /// Explicit base ref for a branch comparison
        #[arg(long, alias = "branch-base")]
        base: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, value_enum, default_value = "unified")]
        layout: DiffLayout,
        #[arg(long)]
        font_size: Option<f64>,
        /// Focus the new viewer after it opens
        #[arg(long)]
        focus: bool,
        /// Preserve the currently focused surface (the default)
        #[arg(long, conflicts_with = "focus")]
        no_focus: bool,
    },
    /// Keep terminal processes alive across cmux quit, crash and update with a private tmux server
    #[command(name = "local-tmux")]
    LocalTmux {
        #[command(subcommand)]
        command: LocalTmuxCommands,
    },
    /// Attach-only alias of `local-tmux attach`, for scripts using tmux vocabulary
    Tmux {
        #[command(subcommand)]
        command: TmuxAliasCommands,
    },
    /// Manage durable diff-review comments keyed by Git repository
    Comments {
        #[command(subcommand)]
        command: CommentCommands,
    },
    /// Open a bounded manifest-aware Linux project inspector
    Project {
        /// Project directory or manifest path
        #[arg(default_value = ".")]
        path: std::path::PathBuf,
        /// Destination workspace UUID; defaults to the caller or selected workspace
        #[arg(long)]
        workspace: Option<String>,
        /// Place the inspector immediately to the right of this surface UUID
        #[arg(long)]
        surface: Option<String>,
        /// Focus the new inspector after it opens
        #[arg(long)]
        focus: bool,
        /// Preserve the currently focused surface (the default)
        #[arg(long, conflicts_with = "focus")]
        no_focus: bool,
    },
    /// Launch Claude Code teams with teammate panes translated into native cmux splits
    ClaudeTeams {
        /// Arguments forwarded verbatim to Claude Code
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Private tmux compatibility endpoint used only by managed team launchers
    #[command(name = "tmux-compat-internal", hide = true, alias = "__tmux-compat")]
    TmuxCompat {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Execute an explicitly requested project command after checking its inspected fingerprint
    ProjectRun {
        action: String,
        #[arg(long)]
        fingerprint: String,
        #[arg(long)]
        workspace: Option<String>,
        /// Confirm a reviewed action that requires an additional destructive decision
        #[arg(long)]
        confirm: bool,
    },
    /// Inspect resolved project actions and their source files without running them
    ProjectActions {
        #[arg(long, conflicts_with = "workspace")]
        directory: Option<std::path::PathBuf>,
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Install and receive native agent session hooks
    Hooks {
        #[command(subcommand)]
        command: HookCommands,
    },
    /// Execute this terminal's saved manual resume command in the calling terminal
    Restore {
        #[arg(long, env = "CMUX_SURFACE_ID")]
        surface: Option<String>,
        #[arg(long)]
        checkpoint: Option<String>,
        /// Require a current application-signed approval before executing
        #[arg(long)]
        automatic: bool,
    },
    /// Manage persistent terminal surface state
    Surface {
        #[command(subcommand)]
        command: SurfaceCommands,
    },
    /// Update a self-managed cmux installation
    Update,
    /// Ping the running cmux instance
    Ping,
    /// Show cmux instance identity (version, platform, pid) and focused topology
    Identify {
        /// Workspace handle used as the caller anchor
        #[arg(long)]
        workspace: Option<String>,
        /// Surface handle used as the caller anchor
        #[arg(long)]
        surface: Option<String>,
        /// Window handle constraining the identification
        #[arg(long)]
        window: Option<String>,
        /// Do not send a caller anchor (skips CMUX_WORKSPACE_ID / CMUX_SURFACE_ID)
        #[arg(long)]
        no_caller: bool,
    },
    /// List supported socket commands
    Capabilities,
    /// Show process resources and diagnostic logging health
    Diagnostics,
    /// Stream workspace, notification and agent-hook events as newline-delimited JSON
    Events {
        /// Start after this sequence number
        #[arg(long, alias = "after-seq")]
        after: Option<u64>,
        /// Read the starting sequence from this file and update it after each event
        #[arg(long)]
        cursor_file: Option<std::path::PathBuf>,
        /// Only this event name (repeatable)
        #[arg(long)]
        name: Vec<String>,
        /// Only this category, such as workspace, notification or agent (repeatable)
        #[arg(long)]
        category: Vec<String>,
        /// Reconnect forever and resume from the last received event
        #[arg(long)]
        reconnect: bool,
        /// Exit after printing this many event frames
        #[arg(long)]
        limit: Option<u64>,
        /// Hide the initial ack frame
        #[arg(long)]
        no_ack: bool,
        /// Hide heartbeat frames
        #[arg(long)]
        no_heartbeat: bool,
    },
    /// List all workspaces
    ListWorkspaces,
    /// Show the current workspace
    CurrentWorkspace,
    /// Send an arbitrary JSON-RPC method
    Raw {
        /// The method name (e.g. "workspace.list")
        method: String,
        /// JSON params string
        #[arg(long, default_value = "{}")]
        params: String,
    },

    // -- Workspace management --
    /// Create a new workspace
    NewWorkspace {
        /// Display name (defaults to the selected folder name)
        #[arg(long)]
        name: Option<String>,
        /// Folder new terminals in this workspace start in
        #[arg(long, value_name = "PATH")]
        cwd: Option<String>,
        /// Text typed into the first terminal's shell, followed by Enter
        #[arg(long)]
        command: Option<String>,
    },
    /// Create a first-class remote workspace with SSH management
    Ssh {
        destination: String,
        #[arg(long, default_value = "ssh", value_parser = ["ssh", "mosh"])]
        transport: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        directory: Option<String>,
    },
    /// Create a remote workspace using Mosh for interactive terminals
    Mosh {
        destination: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        directory: Option<String>,
    },
    /// Create a roaming Mosh terminal attached to a named remote tmux session
    MoshTmux {
        destination: String,
        #[arg(long, default_value = "main")]
        session: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        directory: Option<String>,
    },
    /// Select a workspace by ID
    SelectWorkspace {
        /// Workspace UUID, ref (workspace:N) or index
        #[arg(conflicts_with = "workspace")]
        id: Option<String>,
        /// Workspace UUID, ref (workspace:N) or index; alternative to the positional handle
        #[arg(long, conflicts_with = "id", required_unless_present = "id")]
        workspace: Option<String>,
    },
    /// Close a workspace by ID
    CloseWorkspace {
        /// Workspace UUID, ref (workspace:N) or index
        #[arg(conflicts_with = "workspace")]
        id: Option<String>,
        /// Workspace UUID, ref (workspace:N) or index; alternative to the positional handle
        #[arg(long, conflicts_with = "id", required_unless_present = "id")]
        workspace: Option<String>,
    },
    /// Rename a workspace
    RenameWorkspace {
        /// Workspace UUID, ref (workspace:N) or index; with `--workspace` this slot holds the name
        id: Option<String>,
        /// New name
        #[arg(conflicts_with = "workspace", required_unless_present = "workspace")]
        name: Option<String>,
        /// Workspace UUID, ref (workspace:N) or index; alternative to the positional handle
        #[arg(long, required_unless_present = "id")]
        workspace: Option<String>,
    },
    /// Set (or replace) a workspace description; blank text clears it
    SetDescription {
        /// Workspace UUID, ref (workspace:N) or index; with `--workspace` this slot holds the text
        id: Option<String>,
        /// Markdown description shown under the workspace name
        #[arg(conflicts_with = "workspace", required_unless_present = "workspace")]
        description: Option<String>,
        /// Workspace UUID, ref (workspace:N) or index; alternative to the positional handle
        #[arg(long, required_unless_present = "id")]
        workspace: Option<String>,
    },
    /// Clear a workspace description
    ClearDescription {
        /// Workspace UUID, ref (workspace:N) or index
        #[arg(conflicts_with = "workspace")]
        id: Option<String>,
        /// Workspace UUID, ref (workspace:N) or index; alternative to the positional handle
        #[arg(long, conflicts_with = "id", required_unless_present = "id")]
        workspace: Option<String>,
    },
    /// Switch to next workspace
    NextWorkspace,
    /// Switch to previous workspace
    PrevWorkspace,
    /// Switch to last active workspace
    LastWorkspace,
    /// Reorder a workspace
    ReorderWorkspace {
        /// Workspace UUID, ref (workspace:N) or index; with `--workspace` this slot holds the position
        id: Option<String>,
        /// Target position (0-indexed)
        #[arg(conflicts_with = "workspace", required_unless_present = "workspace")]
        position: Option<usize>,
        /// Workspace UUID, ref (workspace:N) or index; alternative to the positional handle
        #[arg(long, required_unless_present = "id")]
        workspace: Option<String>,
    },

    // -- Surface commands --
    /// Reorder listed workspaces first, retaining the relative order of all others
    ReorderWorkspaces {
        #[arg(long, value_delimiter = ',', required = true)]
        order: Vec<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// List persistent workspace groups and their members
    ListWorkspaceGroups,
    /// Create an empty persistent workspace group
    CreateWorkspaceGroup {
        name: String,
        #[arg(long)]
        color: Option<String>,
    },
    /// Update a workspace group's presentation or collapse state
    UpdateWorkspaceGroup {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, conflicts_with = "clear_color")]
        color: Option<String>,
        #[arg(long)]
        clear_color: bool,
        #[arg(long)]
        collapsed: Option<bool>,
        #[arg(long)]
        position: Option<usize>,
    },
    /// Assign workspaces to a group; omit --group to make them ungrouped
    AssignWorkspaceGroup {
        #[arg(long)]
        group: Option<String>,
        #[arg(long, value_delimiter = ',', required = true)]
        workspaces: Vec<String>,
    },
    /// Delete a group while retaining its workspaces
    DeleteWorkspaceGroup { id: String },
    /// List surfaces (every workspace, or one with --workspace)
    #[command(visible_alias = "list-panels")]
    ListSurfaces {
        /// Workspace UUID, ref (workspace:N) or index
        #[arg(long)]
        workspace: Option<String>,
    },
    /// List the surfaces (tabs) of one pane, the focused pane by default
    ListPaneSurfaces {
        /// Pane ref (pane:N), UUID or index in the current workspace
        #[arg(long)]
        pane: Option<String>,
    },
    /// Show windows, workspaces, panes and surfaces as a tree
    Tree {
        /// Include every window
        #[arg(long)]
        all: bool,
        /// Only this workspace: UUID, ref (workspace:N) or index
        #[arg(long)]
        workspace: Option<String>,
        /// Only this window: id, ref (window:N) or index
        #[arg(long)]
        window: Option<String>,
    },
    /// Split a surface
    Split {
        /// Split direction: horizontal or vertical
        #[arg(long, default_value = "horizontal")]
        direction: String,
        /// Target surface ID, ref (surface:N) or index (default: focused)
        #[arg(long, visible_alias = "surface")]
        id: Option<String>,
    },
    /// Split a pane with a new terminal on one side (upstream `new-split`); focus stays put
    /// unless `--focus true`
    NewSplit {
        /// Side of the new pane: left, right, up or down
        #[arg(value_parser = ["left", "right", "up", "down", "l", "r", "u", "d"])]
        direction: String,
        /// Surface whose pane is split: UUID, ref (surface:N) or index (default: the calling
        /// terminal, else the focused one)
        #[arg(long, visible_alias = "panel")]
        surface: Option<String>,
        /// Workspace whose focused pane is split: UUID, ref (workspace:N) or index
        #[arg(long)]
        workspace: Option<String>,
        /// Text typed into the new shell, followed by Enter
        #[arg(long)]
        command: Option<String>,
        /// Move focus to the new pane
        #[arg(long, value_name = "true|false", value_parser = clap::builder::BoolishValueParser::new())]
        focus: Option<bool>,
    },
    /// Create a pane beside the focused pane of a workspace (upstream `new-pane`)
    NewPane {
        /// Surface type; only terminal is created here (browser: `cmux browser open`)
        #[arg(long, value_parser = ["terminal", "browser"])]
        r#type: Option<String>,
        /// Side of the new pane: left, right, up or down
        #[arg(long, default_value = "right", value_parser = ["left", "right", "up", "down", "l", "r", "u", "d"])]
        direction: String,
        /// Workspace: UUID, ref (workspace:N) or index (default: the calling terminal's, else
        /// the current one)
        #[arg(long)]
        workspace: Option<String>,
        /// Text typed into the new shell, followed by Enter
        #[arg(long)]
        command: Option<String>,
        /// Move focus to the new pane
        #[arg(long, value_name = "true|false", value_parser = clap::builder::BoolishValueParser::new())]
        focus: Option<bool>,
    },
    /// Add a terminal tab to a pane (upstream `new-surface`)
    NewSurface {
        /// Surface type; only terminal is created here (browser: `cmux browser open`)
        #[arg(long, value_parser = ["terminal", "browser"])]
        r#type: Option<String>,
        /// Pane: ref (pane:N), surface UUID or index (default: the workspace's focused pane)
        #[arg(long)]
        pane: Option<String>,
        /// Workspace: UUID, ref (workspace:N) or index (default: the calling terminal's, else
        /// the current one)
        #[arg(long)]
        workspace: Option<String>,
        /// Folder the new terminal starts in
        #[arg(long, visible_alias = "cwd", value_name = "PATH")]
        working_directory: Option<String>,
        /// Text typed into the new shell, followed by Enter
        #[arg(long)]
        command: Option<String>,
        /// Move focus to the new tab
        #[arg(long, value_name = "true|false", value_parser = clap::builder::BoolishValueParser::new())]
        focus: Option<bool>,
    },
    /// Focus a surface by ID
    FocusSurface {
        /// Surface UUID, ref (surface:N) or index
        #[arg(conflicts_with = "surface")]
        id: Option<String>,
        /// Surface UUID, ref (surface:N) or index; alternative to the positional handle
        #[arg(long, conflicts_with = "id", required_unless_present = "id")]
        surface: Option<String>,
    },
    /// Close a surface by ID
    CloseSurface {
        /// Surface UUID, ref (surface:N) or index
        #[arg(conflicts_with = "surface")]
        id: Option<String>,
        /// Surface UUID, ref (surface:N) or index; alternative to the positional handle
        #[arg(long, conflicts_with = "id", required_unless_present = "id")]
        surface: Option<String>,
    },
    /// Move a live surface tab into another pane in the same workspace
    MoveSurface {
        /// Surface UUID, ref (surface:N) or index
        #[arg(conflicts_with = "surface")]
        id: Option<String>,
        /// Surface UUID, ref (surface:N) or index; alternative to the positional handle
        #[arg(long, conflicts_with = "id", required_unless_present = "id")]
        surface: Option<String>,
        /// Destination pane reference (pane:N)
        #[arg(long)]
        pane: Option<String>,
        /// Destination workspace UUID; defaults to the pane owner or source workspace
        #[arg(long)]
        workspace: Option<String>,
        /// Zero-based insertion position; defaults to the end
        #[arg(long)]
        position: Option<usize>,
        /// Preserve current focus instead of selecting the moved surface
        #[arg(long)]
        no_focus: bool,
    },
    /// Reorder a surface tab inside its current pane
    ReorderSurface {
        /// Surface UUID, ref (surface:N) or index; with `--surface` this slot holds the position
        id: Option<String>,
        /// Target position (0-indexed)
        #[arg(conflicts_with = "surface", required_unless_present = "surface")]
        position: Option<usize>,
        /// Surface UUID, ref (surface:N) or index; alternative to the positional handle
        #[arg(long, required_unless_present = "id")]
        surface: Option<String>,
    },
    /// Move a surface into a newly split pane next to a target pane
    DragSurfaceToSplit {
        /// Surface UUID, ref (surface:N) or index
        #[arg(conflicts_with = "surface")]
        id: Option<String>,
        /// Surface UUID, ref (surface:N) or index; alternative to the positional handle
        #[arg(long, conflicts_with = "id", required_unless_present = "id")]
        surface: Option<String>,
        /// Destination pane reference (pane:N) or index
        #[arg(long)]
        pane: String,
        #[arg(long, value_parser = ["left", "right", "up", "down"])]
        direction: String,
    },
    /// Send text to a surface
    SendText {
        /// Text to send
        text: String,
        /// Target surface ID, ref (surface:N) or index (default: focused)
        #[arg(long, visible_alias = "surface")]
        id: Option<String>,
    },
    /// Send text to a surface, expanding `\n`, `\r` and `\t` escapes
    Send {
        /// Text to send; the remaining arguments are joined with a space
        text: Vec<String>,
        /// Target surface ID, ref (surface:N) or index (default: focused)
        #[arg(long, visible_alias = "surface")]
        id: Option<String>,
    },
    /// Send a key name or one literal character to a terminal surface
    SendKey {
        /// Key name (enter, tab, escape, up, ctrl-c…) or one literal character
        key: String,
        /// Target surface ID, ref (surface:N) or index (default: focused)
        #[arg(long, visible_alias = "surface")]
        id: Option<String>,
    },
    /// Read current terminal viewport text (up to 256 KiB)
    ReadText {
        /// Target surface ID, ref (surface:N) or index (default: focused)
        #[arg(long, visible_alias = "surface")]
        id: Option<String>,
    },
    /// Read the screen: the viewport, or the last lines of the scrollback with `--lines`
    ReadScreen {
        /// Target surface ID, ref (surface:N) or index (default: focused)
        #[arg(long, visible_alias = "surface")]
        id: Option<String>,
        /// Read the scrollback instead of the visible viewport
        #[arg(long)]
        scrollback: bool,
        /// Keep only the last N lines of the scrollback (implies `--scrollback`)
        #[arg(long, value_name = "N", allow_hyphen_values = true, value_parser = parse_line_count)]
        lines: Option<usize>,
    },
    /// Capture recent terminal history as bounded VT text (up to 2,000 rows and 256 KiB)
    ReadScrollback {
        /// Target surface ID, ref (surface:N) or index (default: focused)
        #[arg(long, visible_alias = "surface")]
        id: Option<String>,
    },
    /// Check native terminal availability and pane attention
    Health {
        /// Target surface ID, ref (surface:N) or index (default: focused)
        #[arg(long, visible_alias = "surface")]
        id: Option<String>,
    },
    /// Refresh a surface
    Refresh {
        /// Target surface ID, ref (surface:N) or index (default: focused)
        #[arg(long, visible_alias = "surface")]
        id: Option<String>,
    },

    // -- Pane commands --
    /// List panes (every workspace, or one with --workspace)
    ListPanes {
        /// Workspace UUID, ref (workspace:N) or index
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Focus a pane
    FocusPane {
        /// Pane reference (pane:N), surface UUID or index
        #[arg(conflicts_with = "pane")]
        id: Option<String>,
        /// Pane reference (pane:N) or index; alternative to the positional handle
        #[arg(long, conflicts_with = "id")]
        pane: Option<String>,
    },
    /// Switch to last focused pane
    LastPane,

    // -- Window commands --
    /// List all windows
    ListWindows,
    /// Show current window info
    CurrentWindow,

    // -- Debug commands --
    /// Show layout tree
    Layout,
    /// Type text into the focused terminal
    Type {
        /// Text to type
        text: String,
    },

    /// Set a keyed status in a workspace sidebar
    SetStatus {
        key: String,
        value: String,
        #[arg(long)]
        icon: Option<String>,
        #[arg(long)]
        color: Option<String>,
        #[arg(long, default_value_t = 0)]
        priority: i32,
        #[arg(long, default_value = "plain", value_parser = ["plain", "markdown"])]
        format: String,
        #[arg(long, alias = "link")]
        url: Option<String>,
        #[arg(long, alias = "tab", env = "CMUX_WORKSPACE_ID")]
        workspace: Option<String>,
    },
    /// Publish a keyed multiline Markdown summary
    ReportMetaBlock {
        key: String,
        markdown: String,
        #[arg(long, default_value_t = 0)]
        priority: i32,
        #[arg(long, alias = "tab", env = "CMUX_WORKSPACE_ID")]
        workspace: Option<String>,
    },
    /// Remove a keyed Markdown summary
    ClearMetaBlock {
        key: String,
        #[arg(long, alias = "tab", env = "CMUX_WORKSPACE_ID")]
        workspace: Option<String>,
    },
    /// List retained Markdown summaries
    ListMetaBlocks {
        #[arg(long, alias = "tab", env = "CMUX_WORKSPACE_ID")]
        workspace: Option<String>,
    },
    /// Clear one sidebar status key
    ClearStatus {
        key: String,
        #[arg(long, alias = "tab", env = "CMUX_WORKSPACE_ID")]
        workspace: Option<String>,
    },
    /// List attributed listening ports without changing workspace selection
    Ports {
        #[arg(long, alias = "tab", env = "CMUX_WORKSPACE_ID")]
        workspace: Option<String>,
        #[arg(long)]
        surface: Option<String>,
    },
    /// List workspace status entries and progress
    ListStatus {
        #[arg(long, alias = "tab", env = "CMUX_WORKSPACE_ID")]
        workspace: Option<String>,
    },
    /// Set determinate workspace progress from zero to one
    SetProgress {
        value: f64,
        #[arg(long, default_value = "")]
        label: String,
        #[arg(long, alias = "tab", env = "CMUX_WORKSPACE_ID")]
        workspace: Option<String>,
    },
    /// Clear workspace progress
    ClearProgress {
        #[arg(long, alias = "tab", env = "CMUX_WORKSPACE_ID")]
        workspace: Option<String>,
    },

    // -- Notification commands --
    /// Deliver a notification to a terminal without changing focus
    Notify {
        #[arg(long, default_value = "Notification")]
        title: String,
        #[arg(long, default_value = "")]
        subtitle: String,
        #[arg(long, default_value = "")]
        body: String,
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long)]
        surface: Option<String>,
    },
    /// Inspect, read, dismiss and navigate notification history
    Notifications {
        #[command(subcommand)]
        command: NotificationCommands,
    },
    /// List notifications
    ListNotifications,
    /// Clear a notification
    ClearNotification {
        /// Workspace UUID (legacy alias; notifications clear supports explicit scopes)
        id: String,
    },

    // -- Browser subcommand group (agent primary interface) --
    /// Browser automation (agent primary interface)
    #[command(subcommand)]
    Browser(BrowserCommand),
}

/// Inbox operations share the socket's exact notification and target identities.
#[derive(Subcommand)]
pub enum NotificationCommands {
    /// List retained messages and read state
    List,
    /// Remove all messages, or messages in an explicit workspace/surface scope
    Clear {
        /// Clear messages attributed to this calling terminal using native identity
        #[arg(long, conflicts_with_all = ["workspace", "surface"])]
        caller: bool,
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long)]
        surface: Option<String>,
    },
    /// Mark a message, a workspace/surface scope, or all messages read without focus changes
    MarkRead {
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long)]
        surface: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// Remove one message or all previously read messages
    Dismiss {
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        all_read: bool,
    },
    /// Focus the exact terminal referenced by a message
    Open { id: String },
    /// Focus the most recent unread message's terminal
    JumpToUnread,
}

/// Persistent local sessions owned by cmux's private tmux server.
#[derive(Subcommand)]
pub enum LocalTmuxCommands {
    /// Create a session and attach this terminal to it
    Start {
        name: String,
        /// Working directory (default: current)
        #[arg(long)]
        cwd: Option<std::path::PathBuf>,
        /// Command to run in the session instead of a shell
        #[arg(long)]
        command: Option<String>,
        /// Create the session without attaching
        #[arg(long)]
        detached: bool,
    },
    /// Attach this terminal to an existing session
    Attach {
        name: String,
        /// Attach without recording it for cmux restore (outside cmux)
        #[arg(long)]
        headless: bool,
    },
    /// List live sessions
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show one live session
    Status {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Detach every client from a session, leaving it running
    Detach { name: String },
    /// Terminate a session and its processes
    Close { name: String },
}

/// `cmux tmux` accepts only attach; lifecycle stays under `local-tmux`.
#[derive(Subcommand)]
pub enum TmuxAliasCommands {
    /// Attach this terminal to an existing local-tmux session
    Attach {
        name: String,
        #[arg(long)]
        headless: bool,
    },
}

/// Surface operations grouped to match upstream command spelling.
#[derive(Subcommand)]
pub enum SurfaceCommands {
    /// Register or inspect a saved resume command (does not execute it)
    Resume {
        #[command(subcommand)]
        command: ResumeCommands,
    },
}

/// Hook installation and provider events; other providers are added through the same ingestion boundary.
#[derive(Subcommand)]
pub enum HookCommands {
    /// Install supported hooks while preserving unrelated agent configuration
    Setup {
        /// Agent provider (currently claude); omitted discovers available supported providers
        agent: Option<String>,
    },
    /// Receive a Claude Code hook payload on stdin
    Claude {
        #[command(subcommand)]
        event: ClaudeHookEvent,
    },
    /// Receive a Codex lifecycle hook payload on stdin
    Codex {
        #[command(subcommand)]
        event: CodexHookEvent,
    },
    /// Receive a Grok lifecycle hook payload on stdin
    Grok {
        #[command(subcommand)]
        event: JsonHookEvent,
    },
    /// Receive a Gemini lifecycle hook payload on stdin
    Gemini {
        #[command(subcommand)]
        event: JsonHookEvent,
    },
    /// Receive a Kiro CLI agent hook payload on stdin
    Kiro {
        #[command(subcommand)]
        event: JsonHookEvent,
    },
    /// Receive an Antigravity hook payload on stdin
    Antigravity {
        #[command(subcommand)]
        event: JsonHookEvent,
    },
    /// Receive a Hermes Agent YAML hook payload on stdin
    HermesAgent {
        #[command(subcommand)]
        event: JsonHookEvent,
    },
    /// Receive a Kimi Code TOML hook payload on stdin
    Kimi {
        #[command(subcommand)]
        event: JsonHookEvent,
    },
    /// Receive a GitHub Copilot lifecycle hook payload on stdin
    Copilot {
        #[command(subcommand)]
        event: JsonHookEvent,
    },
    /// Receive a CodeBuddy lifecycle hook payload on stdin
    Codebuddy {
        #[command(subcommand)]
        event: JsonHookEvent,
    },
    /// Receive a Factory Droid lifecycle hook payload on stdin
    Factory {
        #[command(subcommand)]
        event: JsonHookEvent,
    },
    /// Receive a Qoder lifecycle hook payload on stdin
    Qoder {
        #[command(subcommand)]
        event: JsonHookEvent,
    },
    /// Receive an OpenCode plugin lifecycle payload on stdin
    Opencode {
        #[command(subcommand)]
        event: JsonHookEvent,
    },
    /// Receive a Cursor Agent lifecycle hook payload on stdin
    Cursor {
        #[command(subcommand)]
        event: JsonHookEvent,
    },
    /// Receive a Pi coding agent extension lifecycle payload on stdin
    Pi {
        #[command(subcommand)]
        event: JsonHookEvent,
    },
    /// Receive an OMP extension lifecycle payload on stdin
    Omp {
        #[command(subcommand)]
        event: JsonHookEvent,
    },
    /// Receive a Campfire extension lifecycle payload on stdin
    Campfire {
        #[command(subcommand)]
        event: JsonHookEvent,
    },
    /// Receive an Amp plugin lifecycle payload on stdin
    Amp {
        #[command(subcommand)]
        event: JsonHookEvent,
    },
    /// Receive a Rovo Dev YAML hook payload on stdin
    Rovodev {
        #[command(subcommand)]
        event: RovoHookEvent,
    },
}

/// Claude session lifecycle and per-turn attention events.
#[derive(Clone, Copy, Subcommand)]
pub enum ClaudeHookEvent {
    SessionStart,
    PromptSubmit,
    SessionEnd,
    Stop,
    Notification,
}

#[derive(Clone, Copy, Subcommand)]
pub enum CodexHookEvent {
    SessionStart,
    PromptSubmit,
    SessionEnd,
    Stop,
}

#[derive(Clone, Copy, Subcommand)]
pub enum JsonHookEvent {
    SessionStart,
    PromptSubmit,
    SessionEnd,
    Stop,
    Notification,
}

#[derive(Clone, Copy, Subcommand)]
pub enum RovoHookEvent {
    PromptSubmit,
    Stop,
}

/// Manual resume binding controls; automatic execution is a separate hook policy.
#[derive(Subcommand)]
pub enum ResumeCommands {
    Set {
        #[arg(long, env = "CMUX_SURFACE_ID")]
        surface: Option<String>,
        #[arg(long)]
        shell: String,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        checkpoint: Option<String>,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long)]
        name: Option<String>,
    },
    Show {
        #[arg(long, env = "CMUX_SURFACE_ID")]
        surface: Option<String>,
    },
    Clear {
        #[arg(long, env = "CMUX_SURFACE_ID")]
        surface: Option<String>,
        #[arg(long)]
        checkpoint: Option<String>,
    },
}

/// Browser subcommands for `cmux browser <action>` / `cmux browser <surface> <action>`.
/// Browser operations translated to the socket protocol by the command runner.
#[derive(Subcommand)]
pub enum BrowserCommand {
    /// Open a URL in the browser pane
    Open {
        /// URL to open
        url: String,
        /// Target workspace ID
        #[arg(long)]
        workspace: Option<String>,
        /// Chrome profile name or persistent profile directory used by agent-browser
        #[arg(long)]
        profile: Option<String>,
    },
    /// List browser surfaces
    List,
    /// Close browser surface(s)
    Close {
        /// Surface reference (surface:N or UUID); closes all if omitted
        #[arg(long)]
        surface: Option<String>,
    },
    /// Take a browser snapshot (accessibility tree / DOM text)
    Snapshot {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Include interactive element annotations
        #[arg(long)]
        interactive: bool,
        /// Compact output
        #[arg(long)]
        compact: bool,
        /// Maximum depth
        #[arg(long)]
        max_depth: Option<u32>,
    },
    /// Click an element
    Click {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Target element (e1 or CSS selector)
        target: String,
        /// Take snapshot after action
        #[arg(long)]
        snapshot_after: bool,
    },
    /// Fill an input field (clears first, then types)
    Fill {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Target element (CSS selector)
        target: String,
        /// Value to fill
        text: String,
        /// Take snapshot after action
        #[arg(long)]
        snapshot_after: bool,
    },
    /// Type text into an element
    #[command(name = "type")]
    BrowserType {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// CSS selector of the element
        selector: String,
        /// Text to type
        text: String,
    },
    /// Press a key (e.g. "Enter", "Tab", "Escape")
    Press {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Key name
        key: String,
    },
    /// Hover over an element
    Hover {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// CSS selector of the element
        selector: String,
    },
    /// Scroll the page
    Scroll {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Direction: up, down, left, right
        direction: String,
        /// Amount in pixels
        #[arg(long, default_value = "300")]
        amount: i32,
    },
    /// Select an option from a dropdown
    #[command(name = "select")]
    Select {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// CSS selector of the select element
        selector: String,
        /// Value to select
        value: String,
    },
    /// Evaluate JavaScript in the browser
    Eval {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// JavaScript expression to evaluate
        expression: String,
    },
    /// Wait for a condition
    Wait {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// CSS selector to wait for
        #[arg(long)]
        selector: Option<String>,
        /// Text to wait for
        #[arg(long)]
        text: Option<String>,
        /// URL substring to wait for
        #[arg(long)]
        url_contains: Option<String>,
        /// Load state to wait for
        #[arg(long)]
        load_state: Option<String>,
        /// JavaScript function to wait for
        #[arg(long)]
        function: Option<String>,
        /// Timeout in milliseconds
        #[arg(long, default_value = "30000")]
        timeout_ms: u64,
    },
    /// Navigate to a URL
    Goto {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// URL to navigate to
        url: String,
    },
    /// Go back in browser history
    Back {
        /// Surface reference (surface:N or UUID)
        surface: String,
    },
    /// Go forward in browser history
    Forward {
        /// Surface reference (surface:N or UUID)
        surface: String,
    },
    /// Reload the current page
    Reload {
        /// Surface reference (surface:N or UUID)
        surface: String,
    },
    /// Get the current page URL
    #[command(name = "get-url")]
    GetUrl {
        /// Surface reference (surface:N or UUID)
        surface: String,
    },
    /// Get the current page title
    #[command(name = "get-title")]
    GetTitle {
        /// Surface reference (surface:N or UUID)
        surface: String,
    },
    /// Get text content of an element
    #[command(name = "get-text")]
    GetText {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// CSS selector of the element
        selector: String,
    },
    /// Get HTML content of an element
    #[command(name = "get-html")]
    GetHtml {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// CSS selector of the element
        selector: String,
    },
    /// Take a browser screenshot (base64 PNG)
    Screenshot {
        /// Surface reference (surface:N or UUID)
        surface: String,
    },
    /// Enable browser streaming
    #[command(name = "stream-enable")]
    StreamEnable,
    /// Disable browser streaming
    #[command(name = "stream-disable")]
    StreamDisable,
}

/// Accept `read-screen --lines N` only for a positive line count.
fn parse_line_count(value: &str) -> Result<usize, String> {
    let lines: i64 = value
        .parse()
        .map_err(|_| format!("line count must be a number: {value}"))?;
    if lines <= 0 {
        return Err("--lines must be greater than 0".to_string());
    }
    Ok(lines as usize)
}

#[cfg(test)]
mod color_argument_tests {
    use super::*;

    /// Output-color fallback must not propagate as an explicit status text color.
    #[test]
    fn unstyled_status_has_no_inherited_color() {
        let cli = Cli::try_parse_from(["cmux", "set-status", "agent", "working"]).unwrap();
        let Commands::SetStatus { color, .. } = cli.command else {
            panic!("wrong command");
        };
        assert!(color.is_none());
        assert!(cli.color.is_none());
        let cli = Cli::try_parse_from([
            "cmux",
            "set-status",
            "agent",
            "working",
            "--color",
            "#123456",
        ])
        .unwrap();
        let Commands::SetStatus { color, .. } = cli.command else {
            panic!("wrong command");
        };
        assert_eq!(color.as_deref(), Some("#123456"));
        let cli = Cli::try_parse_from(["cmux", "list-workspaces", "--color", "never"]).unwrap();
        assert_eq!(cli.color.as_deref(), Some("never"));
    }
}

#[cfg(test)]
mod handle_argument_tests {
    use super::*;

    const WORKSPACE: &str = "20000000-0000-4000-8000-000000000002";
    const SURFACE: &str = "30000000-0000-4000-8000-000000000003";
    const PANE: &str = "40000000-0000-4000-8000-000000000004";

    /// Parse one invocation and return its command, reporting the parser diagnostics on failure.
    fn parse(arguments: &[&str]) -> Commands {
        let mut invocation = vec!["cmux"];
        invocation.extend_from_slice(arguments);
        Cli::try_parse_from(invocation)
            .unwrap_or_else(|error| panic!("{arguments:?} should parse: {error}"))
            .command
    }

    /// Every previously supported target spelling keeps parsing to the same handle.
    #[test]
    fn legacy_target_forms_still_parse() {
        match parse(&["select-workspace", WORKSPACE]) {
            Commands::SelectWorkspace { id, workspace } => {
                assert_eq!(id.as_deref(), Some(WORKSPACE));
                assert!(workspace.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["close-workspace", WORKSPACE]) {
            Commands::CloseWorkspace { id, .. } => assert_eq!(id.as_deref(), Some(WORKSPACE)),
            _ => panic!("wrong command variant"),
        }
        match parse(&["rename-workspace", WORKSPACE, "New name"]) {
            Commands::RenameWorkspace {
                id,
                name,
                workspace,
            } => {
                assert_eq!(id.as_deref(), Some(WORKSPACE));
                assert_eq!(name.as_deref(), Some("New name"));
                assert!(workspace.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["set-description", WORKSPACE, "Shipping list"]) {
            Commands::SetDescription {
                id, description, ..
            } => {
                assert_eq!(id.as_deref(), Some(WORKSPACE));
                assert_eq!(description.as_deref(), Some("Shipping list"));
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["clear-description", WORKSPACE]) {
            Commands::ClearDescription { id, .. } => assert_eq!(id.as_deref(), Some(WORKSPACE)),
            _ => panic!("wrong command variant"),
        }
        match parse(&["reorder-workspace", WORKSPACE, "3"]) {
            Commands::ReorderWorkspace { id, position, .. } => {
                assert_eq!(id.as_deref(), Some(WORKSPACE));
                assert_eq!(position, Some(3));
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["focus-surface", SURFACE]) {
            Commands::FocusSurface { id, surface } => {
                assert_eq!(id.as_deref(), Some(SURFACE));
                assert!(surface.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["close-surface", SURFACE]) {
            Commands::CloseSurface { id, .. } => assert_eq!(id.as_deref(), Some(SURFACE)),
            _ => panic!("wrong command variant"),
        }
        match parse(&["reorder-surface", SURFACE, "2"]) {
            Commands::ReorderSurface {
                id,
                position,
                surface,
            } => {
                assert_eq!(id.as_deref(), Some(SURFACE));
                assert_eq!(position, Some(2));
                assert!(surface.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["move-surface", SURFACE, "--pane", "pane:2"]) {
            Commands::MoveSurface { id, pane, .. } => {
                assert_eq!(id.as_deref(), Some(SURFACE));
                assert_eq!(pane.as_deref(), Some("pane:2"));
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&[
            "drag-surface-to-split",
            SURFACE,
            "--pane",
            PANE,
            "--direction",
            "down",
        ]) {
            Commands::DragSurfaceToSplit { id, pane, .. } => {
                assert_eq!(id.as_deref(), Some(SURFACE));
                assert_eq!(pane, PANE);
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["focus-pane", "pane:1"]) {
            Commands::FocusPane { id, pane } => {
                assert_eq!(id.as_deref(), Some("pane:1"));
                assert!(pane.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["send-text", "hello", "--id", SURFACE]) {
            Commands::SendText { text, id } => {
                assert_eq!(text, "hello");
                assert_eq!(id.as_deref(), Some(SURFACE));
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["split", "--id", SURFACE]) {
            Commands::Split { id, .. } => assert_eq!(id.as_deref(), Some(SURFACE)),
            _ => panic!("wrong command variant"),
        }
        match parse(&["health", "--id", SURFACE]) {
            Commands::Health { id } => assert_eq!(id.as_deref(), Some(SURFACE)),
            _ => panic!("wrong command variant"),
        }
        match parse(&["notify", "--workspace", WORKSPACE]) {
            Commands::Notify { workspace, .. } => assert_eq!(workspace.as_deref(), Some(WORKSPACE)),
            _ => panic!("wrong command variant"),
        }
    }

    /// Flag forms accept handles wherever a positional handle was required before.
    #[test]
    fn flagged_target_forms_parse() {
        match parse(&["select-workspace", "--workspace", "workspace:2"]) {
            Commands::SelectWorkspace { id, workspace } => {
                assert!(id.is_none());
                assert_eq!(workspace.as_deref(), Some("workspace:2"));
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["close-workspace", "--workspace", "workspace:2"]) {
            Commands::CloseWorkspace { workspace, .. } => {
                assert_eq!(workspace.as_deref(), Some("workspace:2"))
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["clear-description", "--workspace", "workspace:2"]) {
            Commands::ClearDescription { workspace, .. } => {
                assert_eq!(workspace.as_deref(), Some("workspace:2"))
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["focus-surface", "--surface", "surface:1"]) {
            Commands::FocusSurface { id, surface } => {
                assert!(id.is_none());
                assert_eq!(surface.as_deref(), Some("surface:1"));
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["close-surface", "--surface", "surface:1"]) {
            Commands::CloseSurface { surface, .. } => {
                assert_eq!(surface.as_deref(), Some("surface:1"))
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&[
            "move-surface",
            "--surface",
            SURFACE,
            "--workspace",
            WORKSPACE,
        ]) {
            Commands::MoveSurface {
                id,
                surface,
                workspace,
                ..
            } => {
                assert!(id.is_none());
                assert_eq!(surface.as_deref(), Some(SURFACE));
                assert_eq!(workspace.as_deref(), Some(WORKSPACE));
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&[
            "drag-surface-to-split",
            "--surface",
            SURFACE,
            "--pane",
            "2",
            "--direction",
            "up",
        ]) {
            Commands::DragSurfaceToSplit {
                id, surface, pane, ..
            } => {
                assert!(id.is_none());
                assert_eq!(surface.as_deref(), Some(SURFACE));
                assert_eq!(pane, "2");
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["focus-pane", "--pane", "pane:3"]) {
            Commands::FocusPane { id, pane } => {
                assert!(id.is_none());
                assert_eq!(pane.as_deref(), Some("pane:3"));
            }
            _ => panic!("wrong command variant"),
        }
        // `--surface` is an alias of `--id` on the surface readers and writers.
        for command in [
            vec!["send-text", "hello", "--surface", SURFACE],
            vec!["send-key", "q", "--surface", SURFACE],
            vec!["read-text", "--surface", SURFACE],
            vec!["read-scrollback", "--surface", SURFACE],
            vec!["split", "--surface", SURFACE],
            vec!["health", "--surface", SURFACE],
            vec!["refresh", "--surface", SURFACE],
        ] {
            let parsed = parse(&command);
            let id = match &parsed {
                Commands::SendText { id, .. }
                | Commands::SendKey { id, .. }
                | Commands::ReadText { id }
                | Commands::ReadScrollback { id }
                | Commands::Split { id, .. }
                | Commands::Health { id }
                | Commands::Refresh { id } => id.as_deref(),
                _ => panic!("wrong command for {command:?}"),
            };
            assert_eq!(id, Some(SURFACE), "{command:?}");
        }
        match parse(&["identify", "--workspace", "workspace:2", "--no-caller"]) {
            Commands::Identify {
                workspace,
                surface,
                window,
                no_caller,
            } => {
                assert_eq!(workspace.as_deref(), Some("workspace:2"));
                assert!(surface.is_none() && window.is_none());
                assert!(no_caller);
            }
            _ => panic!("wrong command variant"),
        }
    }

    /// macOS-style `send`, the `read-screen` options and a named `send-key` all parse.
    #[test]
    fn send_read_screen_and_named_key_forms_parse() {
        match parse(&["send", "--surface", "surface:2", "echo", "hi\\n"]) {
            Commands::Send { text, id } => {
                assert_eq!(text, vec!["echo", "hi\\n"]);
                assert_eq!(id.as_deref(), Some("surface:2"));
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["send", "--", "-n"]) {
            Commands::Send { text, id } => {
                assert_eq!(text, vec!["-n"]);
                assert!(id.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["read-screen", "--lines", "20"]) {
            Commands::ReadScreen {
                id,
                scrollback,
                lines,
            } => {
                assert!(id.is_none());
                assert!(!scrollback);
                assert_eq!(lines, Some(20));
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["read-screen", "--scrollback"]) {
            Commands::ReadScreen {
                scrollback, lines, ..
            } => {
                assert!(scrollback);
                assert!(lines.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["send-key", "--surface", "surface:1", "enter"]) {
            Commands::SendKey { key, id } => {
                assert_eq!(key, "enter");
                assert_eq!(id.as_deref(), Some("surface:1"));
            }
            _ => panic!("wrong command variant"),
        }

        // A non-positive line count is refused before any read is issued.
        let error = Cli::try_parse_from(["cmux", "read-screen", "--lines", "0"])
            .err()
            .expect("--lines 0 must be refused");
        assert!(error.to_string().contains("--lines must be greater than 0"));
        assert!(Cli::try_parse_from(["cmux", "read-screen", "--lines", "-1"]).is_err());
    }

    /// A handle flag leaves the trailing positional for the value that follows it.
    #[test]
    fn trailing_positional_follows_the_flag() {
        match parse(&[
            "rename-workspace",
            "--workspace",
            "workspace:2",
            "--",
            "New name",
        ]) {
            Commands::RenameWorkspace {
                id,
                name,
                workspace,
            } => {
                assert_eq!(workspace.as_deref(), Some("workspace:2"));
                assert_eq!(id.as_deref(), Some("New name"));
                assert!(name.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["set-description", "--workspace", "workspace:2", "Shipping"]) {
            Commands::SetDescription {
                id,
                description,
                workspace,
            } => {
                assert_eq!(workspace.as_deref(), Some("workspace:2"));
                assert_eq!(id.as_deref(), Some("Shipping"));
                assert!(description.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["reorder-workspace", "--workspace", "workspace:2", "4"]) {
            Commands::ReorderWorkspace {
                id,
                position,
                workspace,
            } => {
                assert_eq!(workspace.as_deref(), Some("workspace:2"));
                assert_eq!(id.as_deref(), Some("4"));
                assert_eq!(position, None);
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["reorder-surface", "--surface", "surface:1", "3"]) {
            Commands::ReorderSurface {
                id,
                position,
                surface,
            } => {
                assert_eq!(surface.as_deref(), Some("surface:1"));
                assert_eq!(id.as_deref(), Some("3"));
                assert_eq!(position, None);
            }
            _ => panic!("wrong command variant"),
        }
    }

    /// A positional handle and its flag describe the same target, so both together are refused.
    #[test]
    fn positional_and_flag_target_conflict() {
        for arguments in [
            vec!["select-workspace", WORKSPACE, "--workspace", "workspace:2"],
            vec!["close-workspace", WORKSPACE, "--workspace", "workspace:2"],
            vec!["clear-description", WORKSPACE, "--workspace", "workspace:2"],
            vec![
                "rename-workspace",
                WORKSPACE,
                "New name",
                "--workspace",
                "workspace:2",
            ],
            vec![
                "set-description",
                WORKSPACE,
                "Shipping",
                "--workspace",
                "workspace:2",
            ],
            vec![
                "reorder-workspace",
                WORKSPACE,
                "2",
                "--workspace",
                "workspace:2",
            ],
            vec!["focus-surface", SURFACE, "--surface", "surface:1"],
            vec!["close-surface", SURFACE, "--surface", "surface:1"],
            vec!["reorder-surface", SURFACE, "2", "--surface", "surface:1"],
            vec!["move-surface", SURFACE, "--surface", "surface:1"],
            vec![
                "drag-surface-to-split",
                SURFACE,
                "--surface",
                "surface:1",
                "--pane",
                PANE,
                "--direction",
                "down",
            ],
            vec!["focus-pane", "pane:1", "--pane", "pane:2"],
        ] {
            assert!(
                Cli::try_parse_from(arguments.iter().collect::<Vec<_>>()).is_err(),
                "{arguments:?} must be refused"
            );
        }
    }

    /// Commands that always needed a target still refuse an invocation without one.
    #[test]
    fn missing_target_is_rejected() {
        for arguments in [
            vec!["select-workspace"],
            vec!["close-workspace"],
            vec!["clear-description"],
            vec!["rename-workspace"],
            vec!["rename-workspace", "--workspace", "workspace:2"],
            vec!["set-description"],
            vec!["reorder-workspace"],
            vec!["reorder-workspace", "--workspace", "workspace:2"],
            vec!["focus-surface"],
            vec!["close-surface"],
            vec!["reorder-surface"],
            vec!["move-surface"],
            vec![
                "drag-surface-to-split",
                "--pane",
                PANE,
                "--direction",
                "down",
            ],
        ] {
            assert!(
                Cli::try_parse_from(arguments.iter().collect::<Vec<_>>()).is_err(),
                "{arguments:?} must be refused"
            );
        }
        // `focus-pane` without a target still means "the current pane".
        assert!(matches!(
            parse(&["focus-pane"]),
            Commands::FocusPane {
                id: None,
                pane: None
            }
        ));
    }

    /// `--id-format` is global, validates its value and leaves the default output untouched.
    #[test]
    fn id_format_is_global_and_validated() {
        match Cli::try_parse_from(["cmux", "--id-format", "both", "identify"])
            .expect("leading --id-format should parse")
            .command
        {
            Commands::Identify { .. } => {}
            _ => panic!("wrong command variant"),
        }
        let cli =
            Cli::try_parse_from(["cmux", "notify", "--title", "Done", "--id-format", "uuids"])
                .expect("trailing --id-format should parse");
        assert!(matches!(cli.id_format, Some(IdFormat::Uuids)));
        assert!(matches!(
            Cli::try_parse_from(["cmux", "list-workspaces", "--id-format", "refs"])
                .expect("refs mode should parse")
                .id_format,
            Some(IdFormat::Refs)
        ));
        assert!(Cli::try_parse_from(["cmux", "--id-format", "short", "identify"]).is_err());
        let cli = Cli::try_parse_from(["cmux", "ping"]).expect("plain ping should parse");
        assert!(cli.id_format.is_none());
    }
}
