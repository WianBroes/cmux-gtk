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
    /// Render a Markdown file in a browser surface right of the calling terminal
    Markdown {
        /// Markdown file to render
        file: std::path::PathBuf,
        /// Destination workspace UUID; defaults to the caller or selected workspace
        #[arg(long)]
        workspace: Option<String>,
        /// Place the viewer immediately to the right of this surface UUID
        #[arg(long)]
        surface: Option<String>,
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
    /// Workspace verbs in their upstream `cmux workspace …` spelling
    #[command(subcommand)]
    Workspace(WorkspaceCommands),
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
        #[arg(
            conflicts_with_all = ["workspace", "index", "before", "after"],
            required_unless_present_any = ["workspace", "index", "before", "after"]
        )]
        position: Option<usize>,
        /// Workspace UUID, ref (workspace:N) or index; alternative to the positional handle
        #[arg(long, required_unless_present = "id")]
        workspace: Option<String>,
        /// Target position (0-indexed), upstream's spelling of the trailing position
        #[arg(long, conflicts_with_all = ["position", "before", "after"])]
        index: Option<usize>,
        /// Place the workspace before this workspace
        #[arg(long, visible_alias = "before-workspace", conflicts_with = "after")]
        before: Option<String>,
        /// Place the workspace after this workspace
        #[arg(long, visible_alias = "after-workspace")]
        after: Option<String>,
        /// Window owning the order; this build has a single window (`window:1`)
        #[arg(long)]
        window: Option<String>,
        /// Print the resolved final index without applying
        #[arg(long)]
        dry_run: bool,
    },
    /// Apply an action to a workspace (upstream `workspace-action`)
    WorkspaceAction {
        /// Action name; may also be given as the first positional argument
        #[arg(long)]
        action: Option<String>,
        /// Workspace UUID, ref (workspace:N) or index; defaults to the active workspace
        #[arg(long)]
        workspace: Option<String>,
        /// Window context; this build has a single window (`window:1`)
        #[arg(long)]
        window: Option<String>,
        /// Title given to `rename` (upstream's spelling; `--name` is an alias)
        #[arg(long, visible_alias = "name")]
        title: Option<String>,
        /// Color given to `set-color` (#RRGGBB)
        #[arg(long)]
        color: Option<String>,
        /// Description given to `set-description`
        #[arg(long)]
        description: Option<String>,
        /// Action, or the first word of a positional value, passed without `--action`
        #[arg(value_name = "ACTION")]
        action_positional: Option<String>,
        /// Remaining words of a positional value (`rename My title`)
        #[arg(num_args = 0.., value_name = "TEXT")]
        value_words: Vec<String>,
    },
    /// Perform a horizontal tab context-menu action (upstream `tab-action`)
    TabAction {
        /// Action name; may also be given as the first positional argument
        #[arg(long)]
        action: Option<String>,
        /// Target tab: surface UUID, ref (surface:N) or index; default is the focused tab
        #[arg(long, visible_alias = "tab")]
        surface: Option<String>,
        /// Workspace owning the tab context; defaults to the tab's, else the active one
        #[arg(long)]
        workspace: Option<String>,
        /// Window context; this build has a single window (`window:1`)
        #[arg(long)]
        window: Option<String>,
        /// Title given to `rename` (rejected on Linux: tabs have no custom title)
        #[arg(long)]
        title: Option<String>,
        /// URL given to `new-browser-right` (rejected on Linux)
        #[arg(long)]
        url: Option<String>,
        /// Focus the destination where the action supports it (creation only)
        #[arg(
            long,
            value_name = "true|false",
            value_parser = clap::builder::BoolishValueParser::new()
        )]
        focus: Option<bool>,
        /// Action, or the first word of a positional title, passed without `--action`
        #[arg(value_name = "ACTION")]
        action_positional: Option<String>,
        /// Remaining words of a positional title (`rename build logs`)
        #[arg(num_args = 0.., value_name = "TEXT")]
        value_words: Vec<String>,
    },
    /// Move a tab into a newly created workspace (upstream also spells it `detach-tab`)
    #[command(visible_alias = "detach-tab")]
    MoveTabToNewWorkspace {
        /// Tab to move: surface UUID, ref (surface:N) or index; default is the focused tab
        #[arg(long, visible_alias = "tab")]
        surface: Option<String>,
        /// Source workspace; defaults to the tab's workspace, else the active one
        #[arg(long)]
        workspace: Option<String>,
        /// Window context; this build has a single window (`window:1`)
        #[arg(long)]
        window: Option<String>,
        /// Title of the workspace created to receive the tab
        #[arg(long)]
        title: Option<String>,
        /// Select the new workspace and focus the moved tab (upstream defaults to false)
        #[arg(
            long,
            value_name = "true|false",
            value_parser = clap::builder::BoolishValueParser::new()
        )]
        focus: Option<bool>,
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
        /// Destination window; this build has a single window (`window:1`)
        #[arg(long)]
        window: Option<String>,
        /// Place the surface before this surface (same pane/workspace context)
        #[arg(long, visible_alias = "before-surface", conflicts_with = "after")]
        before: Option<String>,
        /// Place the surface after this surface (same pane/workspace context)
        #[arg(long, visible_alias = "after-surface")]
        after: Option<String>,
        /// Zero-based insertion position; defaults to the end
        #[arg(long, visible_alias = "index", conflicts_with_all = ["before", "after"])]
        position: Option<usize>,
        /// Preserve current focus instead of selecting the moved surface
        #[arg(long, conflicts_with = "focus")]
        no_focus: bool,
        /// Focus the surface after moving; `--focus false` equals `--no-focus`
        #[arg(
            long,
            value_name = "true|false",
            value_parser = clap::builder::BoolishValueParser::new(),
            conflicts_with = "no_focus"
        )]
        focus: Option<bool>,
    },
    /// Reorder a surface tab inside its current pane
    ReorderSurface {
        /// Surface UUID, ref (surface:N) or index; with `--surface` this slot holds the position
        id: Option<String>,
        /// Target position (0-indexed)
        #[arg(
            conflicts_with_all = ["surface", "index", "before", "after"],
            required_unless_present_any = ["surface", "index", "before", "after"]
        )]
        position: Option<usize>,
        /// Surface UUID, ref (surface:N) or index; alternative to the positional handle
        #[arg(long, required_unless_present = "id")]
        surface: Option<String>,
        /// Target position (0-indexed), upstream's spelling of the trailing position
        #[arg(long, conflicts_with_all = ["position", "before", "after"])]
        index: Option<usize>,
        /// Place the surface before this surface of the same pane
        #[arg(long, visible_alias = "before-surface", conflicts_with = "after")]
        before: Option<String>,
        /// Place the surface after this surface of the same pane
        #[arg(long, visible_alias = "after-surface")]
        after: Option<String>,
        /// Window owning the pane; this build has a single window (`window:1`)
        #[arg(long)]
        window: Option<String>,
        /// Select the reordered surface; by default the current focus is kept
        #[arg(
            long,
            value_name = "true|false",
            value_parser = clap::builder::BoolishValueParser::new()
        )]
        focus: Option<bool>,
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
    /// Split the surface's own pane and move the tab into the new split (upstream `split-off`)
    SplitOff {
        /// Surface to split out: UUID, ref (surface:N) or index
        #[arg(long, visible_alias = "panel")]
        surface: String,
        /// Side of the new split: left, right, up or down
        #[arg(value_parser = ["left", "right", "up", "down"])]
        direction: String,
        /// Focus the surface after splitting it out (upstream defaults to keeping focus)
        #[arg(
            long,
            value_name = "true|false",
            value_parser = clap::builder::BoolishValueParser::new()
        )]
        focus: Option<bool>,
    },
    /// Flash the attention markers of a surface (upstream `trigger-flash`)
    TriggerFlash {
        /// Target surface: UUID, ref (surface:N) or index; default is the focused tab
        #[arg(long, visible_alias = "panel")]
        surface: Option<String>,
        /// Workspace context; defaults to the tab's workspace, else the active one
        #[arg(long)]
        workspace: Option<String>,
        /// Window context; this build has a single window (`window:1`)
        #[arg(long)]
        window: Option<String>,
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
    /// List health details for every surface of a workspace (upstream `surface-health`)
    SurfaceHealth {
        /// Workspace whose surfaces are listed; default is the active one
        #[arg(long)]
        workspace: Option<String>,
        /// Window context; this build has a single window (`window:1`)
        #[arg(long)]
        window: Option<String>,
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
    /// Append a line to the workspace log; the sidebar shows the latest
    Log {
        /// info, progress, success, warning or error
        #[arg(long, default_value = "info")]
        level: String,
        /// Label naming who wrote the line
        #[arg(long)]
        source: Option<String>,
        #[arg(long, alias = "tab", env = "CMUX_WORKSPACE_ID")]
        workspace: Option<String>,
        /// Message (put it after `--` when it starts with a dash)
        #[arg(required = true, num_args = 1.., allow_hyphen_values = true, trailing_var_arg = true)]
        message: Vec<String>,
    },
    /// List the workspace log, oldest first
    ListLog {
        /// Only the last N entries
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long, alias = "tab", env = "CMUX_WORKSPACE_ID")]
        workspace: Option<String>,
    },
    /// Clear the workspace log
    ClearLog {
        #[arg(long, alias = "tab", env = "CMUX_WORKSPACE_ID")]
        workspace: Option<String>,
    },
    /// Show everything the sidebar knows about a workspace
    SidebarState {
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
        /// Clear this terminal's (or the given target's) notifications instead
        #[arg(long, conflicts_with_all = ["title", "subtitle", "body", "message"])]
        clear: bool,
        /// Body text, when --body is not given
        #[arg(conflicts_with = "body")]
        message: Option<String>,
    },
    /// Inspect, read, dismiss and navigate notification history
    Notifications {
        #[command(subcommand)]
        command: NotificationCommands,
    },
    /// List notifications
    ListNotifications,
    /// Remove one notification, or all read ones (same as `notifications dismiss`)
    DismissNotification {
        #[arg(
            long,
            required_unless_present = "all_read",
            conflicts_with = "all_read"
        )]
        id: Option<String>,
        #[arg(long)]
        all_read: bool,
    },
    /// Clear notifications of a workspace/surface, or all of them (same as `notifications clear`)
    ClearNotifications {
        #[arg(long, alias = "tab", env = "CMUX_WORKSPACE_ID")]
        workspace: Option<String>,
        #[arg(long, requires = "workspace")]
        surface: Option<String>,
    },
    /// Clear a notification
    ClearNotification {
        /// Workspace UUID (legacy alias; notifications clear supports explicit scopes)
        id: String,
    },

    // -- Right sidebar (Files panel) --
    /// Show, hide or toggle the right sidebar (Files panel)
    RightSidebar {
        #[command(subcommand)]
        command: RightSidebarCommands,
    },

    // -- Browser subcommand group (agent primary interface) --
    /// Browser automation (agent primary interface)
    #[command(subcommand)]
    Browser(BrowserCommand),

    // -- Settings file (cmux.json) --
    /// Inspect the cmux.json settings file without contacting the running app
    #[command(subcommand)]
    Config(ConfigCommands),
}

/// `--scope` values, mirroring `settings_json::Scope` without coupling the parser to it.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum ScopeArg {
    /// The user-wide `~/.config/cmux/cmux.json`.
    Global,
    /// A `cmux.json` in the working directory or one of its parents.
    Project,
}

/// `cmux config <sub>`: read, validate and edit `cmux.json` with the app closed.
///
/// These verbs only read or write the file and the bundled schema; none of them opens the
/// socket, so they work while cmux is not running. `validate` exits 1 when the file has an
/// error and 0 when it only has warnings, matching the macOS `cmux config doctor` contract.
/// `set` and `unset` print `{"status":"persisted",…}` on success and `{"status":"unchanged",…}`
/// when an `unset` finds nothing to remove.
#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Print the path of the global cmux.json
    Path,
    /// Validate the settings file against the bundled cmux schema
    #[command(aliases = ["doctor", "check"])]
    Validate {
        /// Settings file to validate; defaults to the global cmux.json
        #[arg(long, value_name = "FILE")]
        file: Option<std::path::PathBuf>,
        /// Scope the file is read as; inferred from its path when absent
        #[arg(long, value_name = "SCOPE")]
        scope: Option<ScopeArg>,
    },
    /// Print the value at a dotted settings path
    Get {
        /// Dotted settings path, for example sidebar.branchLayout
        path: String,
        /// Settings file to read; defaults to the global cmux.json
        #[arg(long, value_name = "FILE")]
        file: Option<std::path::PathBuf>,
    },
    /// Print the cmux.json settings reference: paths, format, scopes and example keys
    #[command(aliases = ["documentation"])]
    Docs,
    /// Set the value at a dotted settings path, keeping the file's comments
    Set {
        /// Dotted settings path, for example app.appearance
        path: String,
        /// Value as JSON, for example true, 12 or "dark"; an unquoted word becomes a string
        #[arg(allow_hyphen_values = true)]
        value: String,
        /// Settings file to write; defaults to the global cmux.json
        #[arg(long, value_name = "FILE")]
        file: Option<std::path::PathBuf>,
        /// Scope the file is written as; inferred from its path when absent
        #[arg(long, value_name = "SCOPE")]
        scope: Option<ScopeArg>,
    },
    /// Remove the value at a dotted settings path, reverting it to the built-in default
    Unset {
        /// Dotted settings path, for example app.appearance
        path: String,
        /// Settings file to write; defaults to the global cmux.json
        #[arg(long, value_name = "FILE")]
        file: Option<std::path::PathBuf>,
        /// Scope the file is written as; inferred from its path when absent
        #[arg(long, value_name = "SCOPE")]
        scope: Option<ScopeArg>,
    },
    /// List every settings path the bundled schema recognizes
    ListSupported,
}

/// `cmux workspace <sub>`: upstream spelling of the legacy workspace verbs, same arguments.
///
/// The parser mirrors the legacy fields one for one so `run` can expand the namespace onto
/// `new-workspace`, `close-workspace`, `select-workspace`, `rename-workspace`,
/// `list-workspaces` and `current-workspace` and keep a single canonicalization and
/// dispatch path.
#[derive(Subcommand)]
pub enum WorkspaceCommands {
    /// Create a new workspace (same options as `new-workspace`)
    Create {
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
    /// Close a workspace (same options as `close-workspace`)
    Close {
        /// Workspace UUID, ref (workspace:N) or index
        #[arg(conflicts_with = "workspace")]
        id: Option<String>,
        /// Workspace UUID, ref (workspace:N) or index; alternative to the positional handle
        #[arg(long, conflicts_with = "id", required_unless_present = "id")]
        workspace: Option<String>,
    },
    /// Select a workspace (same options as `select-workspace`)
    Select {
        /// Workspace UUID, ref (workspace:N) or index
        #[arg(conflicts_with = "workspace")]
        id: Option<String>,
        /// Workspace UUID, ref (workspace:N) or index; alternative to the positional handle
        #[arg(long, conflicts_with = "id", required_unless_present = "id")]
        workspace: Option<String>,
    },
    /// Rename a workspace (same options as `rename-workspace`)
    Rename {
        /// Workspace UUID, ref (workspace:N) or index; with `--workspace` this slot holds the name
        id: Option<String>,
        /// New name
        #[arg(conflicts_with = "workspace", required_unless_present = "workspace")]
        name: Option<String>,
        /// Workspace UUID, ref (workspace:N) or index; alternative to the positional handle
        #[arg(long, required_unless_present = "id")]
        workspace: Option<String>,
    },
    /// List all workspaces (same output as `list-workspaces`)
    List,
    /// Show the current workspace (same output as `current-workspace`)
    Current,
}

/// Right sidebar operations; only the Files mode exists on Linux so far.
#[derive(Subcommand)]
pub enum RightSidebarCommands {
    /// Show or hide the right sidebar
    Toggle,
    /// Show the right sidebar
    Show,
    /// Hide the right sidebar
    Hide,
    /// Show the right sidebar and select a mode (only `files` exists for now)
    Set { mode: String },
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
        #[arg(conflicts_with = "agent_flag")]
        agent: Option<String>,
        /// Same as the positional agent (upstream form)
        #[arg(long = "agent", value_name = "AGENT")]
        agent_flag: Option<String>,
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
        #[arg(long, short = 'i')]
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
        /// Value to fill (empty clears the field)
        #[arg(default_value = "")]
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
    #[command(alias = "key")]
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
    // `allow_negative_numbers` lets the bare integer form (`scroll S -120`) parse as `direction`.
    #[command(allow_negative_numbers = true)]
    Scroll {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Direction: up, down, left, right (an integer scrolls vertically by that many pixels)
        direction: Option<String>,
        /// Amount in pixels
        #[arg(long, default_value = "300")]
        amount: i32,
        /// Scroll relative to an element instead of the page
        #[arg(long)]
        selector: Option<String>,
        /// Horizontal offset in pixels
        #[arg(long, allow_hyphen_values = true)]
        dx: Option<i32>,
        /// Vertical offset in pixels
        #[arg(long, allow_hyphen_values = true)]
        dy: Option<i32>,
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
    #[command(alias = "navigate")]
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
    #[command(name = "get-url", alias = "url")]
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
        /// Write the PNG to this path instead of printing it
        #[arg(long)]
        out: Option<String>,
    },
    /// Enable browser streaming
    #[command(name = "stream-enable")]
    StreamEnable,
    /// Disable browser streaming
    #[command(name = "stream-disable")]
    StreamDisable,

    // -- Upstream verbs and grouped readers --
    /// Identify the browser and read the page of one surface
    Identify {
        /// Surface reference (surface:N or UUID)
        #[arg(long)]
        surface: Option<String>,
    },
    /// Double-click an element
    Dblclick {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Target element (CSS selector)
        selector: String,
    },
    /// Focus an element
    Focus {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Target element (CSS selector)
        selector: String,
    },
    /// Tick a checkbox
    Check {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Target element (CSS selector)
        selector: String,
    },
    /// Untick a checkbox
    Uncheck {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Target element (CSS selector)
        selector: String,
    },
    /// Hold a key down
    Keydown {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Key name
        key: String,
    },
    /// Release a key
    Keyup {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Key name
        key: String,
    },
    /// Outline matching elements
    Highlight {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Target element (CSS selector)
        selector: String,
    },
    /// Switch to the main frame or a matching frame
    Frame {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Frame to select ("main" or a CSS selector)
        target: String,
    },
    /// List or clear browser console messages
    Console {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Action: list (default) or clear
        #[arg(default_value = "list", value_parser = ["list", "clear"])]
        action: String,
    },
    /// List or clear captured page errors
    Errors {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Action: list (default) or clear
        #[arg(default_value = "list", value_parser = ["list", "clear"])]
        action: String,
    },
    /// Read a value from the page
    Get {
        #[command(subcommand)]
        command: BrowserGetCommand,
    },
    /// Probe element state
    Is {
        #[command(subcommand)]
        command: BrowserIsCommand,
    },
    /// Answer the open dialog
    Dialog {
        #[command(subcommand)]
        command: BrowserDialogCommand,
    },
    /// Save or restore page state
    State {
        #[command(subcommand)]
        command: BrowserStateCommand,
    },
    /// Resize the browser viewport
    // `allow_negative_numbers` lets a negative size parse so validation reports it clearly.
    #[command(allow_negative_numbers = true)]
    Viewport {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Width in pixels, or `reset` (agent-browser documents no reset)
        width: String,
        /// Height in pixels
        height: Option<i32>,
    },
    /// Manage browser cookies
    Cookies {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Action: get (default), set or clear
        #[arg(default_value = "get", value_parser = ["get", "set", "clear"])]
        action: String,
        /// Cookie name (set only)
        name: Option<String>,
        /// Cookie value (set only)
        value: Option<String>,
        /// URL to set the cookie for
        #[arg(long)]
        url: Option<String>,
        /// Cookie domain
        #[arg(long)]
        domain: Option<String>,
        /// Cookie path
        #[arg(long)]
        path: Option<String>,
        /// HttpOnly flag
        #[arg(long = "httpOnly")]
        http_only: bool,
        /// Secure flag
        #[arg(long)]
        secure: bool,
        /// SameSite attribute
        #[arg(long = "sameSite")]
        same_site: Option<String>,
        /// Expiration as a Unix timestamp in seconds (the daemon rejects a string)
        #[arg(long)]
        expires: Option<i64>,
    },
    /// Read browser web storage
    Storage {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Store to read: local or session
        #[arg(value_parser = ["local", "session"])]
        store: String,
    },
    /// Register a script that runs before page scripts on every navigation
    #[command(name = "addinitscript")]
    AddInitScript {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// JavaScript source run before page scripts
        script: String,
    },
    /// Insert CSS into the page
    #[command(name = "addstyle")]
    AddStyle {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// CSS source inserted into the page
        css: String,
    },
    /// Insert JavaScript into the page
    #[command(name = "addscript")]
    AddScript {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// JavaScript source inserted into the page
        script: String,
    },
    /// Click an element and save the download to a file
    Download {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Target element (CSS selector)
        selector: String,
        /// File the download is written to
        path: String,
    },
    /// Wait for a download started by a previous action
    #[command(name = "download-wait")]
    DownloadWait {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Timeout in milliseconds
        #[arg(long)]
        timeout: Option<u64>,
        /// File the download is written to
        #[arg(long)]
        path: Option<String>,
    },
}

/// Page values readable through `cmux browser get <what> <surface>`.
#[derive(Subcommand)]
pub enum BrowserGetCommand {
    /// Get the current page URL
    Url {
        /// Surface reference (surface:N or UUID)
        surface: String,
    },
    /// Get the current page title
    Title {
        /// Surface reference (surface:N or UUID)
        surface: String,
    },
    /// Get text content of an element
    Text {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// CSS selector of the element
        selector: String,
    },
    /// Get HTML content of an element
    Html {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// CSS selector of the element
        selector: String,
    },
    /// Get the value of an input element
    Value {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// CSS selector of the element
        selector: String,
    },
    /// Get an attribute of an element
    Attr {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// CSS selector of the element
        selector: String,
        /// Attribute name (alternative to --attr)
        name: Option<String>,
        /// Attribute name, winning over the positional one
        #[arg(long)]
        attr: Option<String>,
    },
    /// Count matching elements
    Count {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// CSS selector of the elements
        selector: String,
    },
    /// Get the bounding box of an element
    Box {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// CSS selector of the element
        selector: String,
    },
    /// Get computed styles of an element
    Styles {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// CSS selector of the element
        selector: String,
        /// CSS property to read (every property if omitted)
        #[arg(long)]
        property: Option<String>,
    },
}

/// Element probes readable through `cmux browser is <what> <surface>`.
#[derive(Subcommand)]
pub enum BrowserIsCommand {
    /// Whether an element is visible
    Visible {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// CSS selector of the element
        selector: String,
    },
    /// Whether an element is enabled
    Enabled {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// CSS selector of the element
        selector: String,
    },
    /// Whether an element is checked
    Checked {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// CSS selector of the element
        selector: String,
    },
}

/// Dialog answers through `cmux browser dialog <action> <surface>`.
#[derive(Subcommand)]
pub enum BrowserDialogCommand {
    /// Accept the dialog, optionally with prompt text
    Accept {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// Words of the response, joined with a space
        text: Vec<String>,
    },
    /// Dismiss the dialog
    Dismiss {
        /// Surface reference (surface:N or UUID)
        surface: String,
    },
}

/// Page state persistence through `cmux browser state <action> <surface>`.
#[derive(Subcommand)]
pub enum BrowserStateCommand {
    /// Save page state to a file
    Save {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// File the state is written to
        path: String,
    },
    /// Load page state from a file
    Load {
        /// Surface reference (surface:N or UUID)
        surface: String,
        /// File the state is read from
        path: String,
    },
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
                ..
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
                ..
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
                ..
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

    /// Upstream's placement, window and focus flags reach the same move command.
    #[test]
    fn move_surface_placement_window_and_focus_forms_parse() {
        match parse(&["move-surface", "surface:3", "--before", "surface:1"]) {
            Commands::MoveSurface {
                id,
                before,
                after,
                position,
                ..
            } => {
                assert_eq!(id.as_deref(), Some("surface:3"));
                assert_eq!(before.as_deref(), Some("surface:1"));
                assert!(after.is_none());
                assert!(position.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&[
            "move-surface",
            SURFACE,
            "--window",
            "window:1",
            "--index",
            "0",
            "--focus",
            "false",
        ]) {
            Commands::MoveSurface {
                window,
                position,
                focus,
                no_focus,
                ..
            } => {
                assert_eq!(window.as_deref(), Some("window:1"));
                assert_eq!(position, Some(0));
                assert_eq!(focus, Some(false));
                assert!(!no_focus);
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["move-surface", SURFACE, "--no-focus"]) {
            Commands::MoveSurface {
                focus, no_focus, ..
            } => {
                assert!(focus.is_none());
                assert!(no_focus);
            }
            _ => panic!("wrong command variant"),
        }

        // Contradicting placements, and a focus spelling given twice, are refused.
        for arguments in [
            vec![
                "move-surface",
                SURFACE,
                "--before",
                "surface:1",
                "--after",
                "surface:2",
            ],
            vec![
                "move-surface",
                SURFACE,
                "--index",
                "1",
                "--before",
                "surface:1",
            ],
            vec![
                "move-surface",
                SURFACE,
                "--position",
                "1",
                "--after",
                "surface:2",
            ],
            vec!["move-surface", SURFACE, "--focus", "true", "--no-focus"],
        ] {
            assert!(
                Cli::try_parse_from(arguments.iter().collect::<Vec<_>>()).is_err(),
                "{arguments:?} must be refused"
            );
        }
    }

    /// Upstream's relative placement, `--index` and `--window` reach the reorder commands.
    #[test]
    fn reorder_relative_placement_forms_parse() {
        match parse(&["reorder-surface", "surface:3", "--after", "surface:1"]) {
            Commands::ReorderSurface {
                id,
                position,
                index,
                before,
                after,
                window,
                ..
            } => {
                assert_eq!(id.as_deref(), Some("surface:3"));
                assert_eq!(after.as_deref(), Some("surface:1"));
                assert!(position.is_none() && index.is_none());
                assert!(before.is_none() && window.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&[
            "reorder-surface",
            "--surface",
            "surface:2",
            "--index",
            "0",
            "--window",
            "window:1",
        ]) {
            Commands::ReorderSurface {
                id,
                position,
                index,
                surface,
                window,
                ..
            } => {
                assert_eq!(surface.as_deref(), Some("surface:2"));
                assert_eq!(index, Some(0));
                assert_eq!(window.as_deref(), Some("window:1"));
                assert!(id.is_none() && position.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&[
            "reorder-workspace",
            "--workspace",
            "workspace:2",
            "--before",
            "workspace:1",
        ]) {
            Commands::ReorderWorkspace {
                id,
                position,
                workspace,
                before,
                after,
                ..
            } => {
                assert_eq!(workspace.as_deref(), Some("workspace:2"));
                assert_eq!(before.as_deref(), Some("workspace:1"));
                assert!(id.is_none() && position.is_none() && after.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&[
            "reorder-workspace",
            WORKSPACE,
            "--after-workspace",
            "workspace:1",
        ]) {
            Commands::ReorderWorkspace {
                id,
                after,
                position,
                ..
            } => {
                assert_eq!(id.as_deref(), Some(WORKSPACE));
                assert_eq!(after.as_deref(), Some("workspace:1"));
                assert!(position.is_none());
            }
            _ => panic!("wrong command variant"),
        }

        // A slot is named once: anchors, indexes and trailing positions contradict each other.
        for arguments in [
            vec!["reorder-surface", SURFACE, "1", "--before", "surface:2"],
            vec![
                "reorder-surface",
                "--surface",
                SURFACE,
                "--before",
                "surface:2",
                "--after",
                "surface:3",
            ],
            vec![
                "reorder-surface",
                "--surface",
                SURFACE,
                "--index",
                "1",
                "--after",
                "surface:3",
            ],
            vec!["reorder-workspace", WORKSPACE, "2", "--index", "1"],
            vec![
                "reorder-workspace",
                "--workspace",
                "workspace:2",
                "--before",
                "workspace:1",
                "--after",
                "workspace:3",
            ],
        ] {
            assert!(
                Cli::try_parse_from(arguments.iter().collect::<Vec<_>>()).is_err(),
                "{arguments:?} must be refused"
            );
        }
    }

    /// `reorder-surface --focus` and `reorder-workspace --dry-run` parse upstream's flags.
    #[test]
    fn reorder_focus_and_dry_run_parse() {
        match parse(&[
            "reorder-surface",
            "surface:2",
            "--index",
            "0",
            "--focus",
            "true",
        ]) {
            Commands::ReorderSurface { focus, .. } => assert_eq!(focus, Some(true)),
            _ => panic!("wrong command variant"),
        }
        // The flag takes both boolean spellings and nothing else; absent means "keep focus".
        match parse(&[
            "reorder-surface",
            "--surface",
            SURFACE,
            "1",
            "--focus",
            "false",
        ]) {
            Commands::ReorderSurface { focus, .. } => assert_eq!(focus, Some(false)),
            _ => panic!("wrong command variant"),
        }
        match parse(&["reorder-surface", SURFACE, "1"]) {
            Commands::ReorderSurface { focus, .. } => assert!(focus.is_none()),
            _ => panic!("wrong command variant"),
        }
        assert!(
            Cli::try_parse_from(["cmux", "reorder-surface", SURFACE, "1", "--focus", "maybe"])
                .is_err()
        );

        match parse(&["reorder-workspace", WORKSPACE, "0", "--dry-run"]) {
            Commands::ReorderWorkspace { dry_run, .. } => assert!(dry_run),
            _ => panic!("wrong command variant"),
        }
        match parse(&["reorder-workspace", WORKSPACE, "0"]) {
            Commands::ReorderWorkspace { dry_run, .. } => assert!(!dry_run),
            _ => panic!("wrong command variant"),
        }
    }

    /// Upstream's `split-off --surface <handle> <direction>` parses with its focus flag.
    #[test]
    fn split_off_form_parses() {
        match parse(&["split-off", "--surface", "surface:2", "right"]) {
            Commands::SplitOff {
                surface,
                direction,
                focus,
            } => {
                assert_eq!(surface, "surface:2");
                assert_eq!(direction, "right");
                assert!(focus.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["split-off", "--panel", SURFACE, "up", "--focus", "true"]) {
            Commands::SplitOff {
                surface,
                direction,
                focus,
            } => {
                assert_eq!(surface, SURFACE);
                assert_eq!(direction, "up");
                assert_eq!(focus, Some(true));
            }
            _ => panic!("wrong command variant"),
        }
        // Both halves are required, and the direction is a fixed vocabulary.
        for arguments in [
            vec!["split-off", "--surface", SURFACE],
            vec!["split-off", "right"],
            vec!["split-off", "--surface", SURFACE, "sideways"],
        ] {
            assert!(
                Cli::try_parse_from(arguments.iter().collect::<Vec<_>>()).is_err(),
                "{arguments:?} must be refused"
            );
        }
    }

    /// The upstream `workspace <sub>` spelling parses with the legacy verbs' options.
    #[test]
    fn workspace_namespace_forms_parse() {
        match parse(&[
            "workspace",
            "create",
            "--name",
            "Plan",
            "--cwd",
            "/srv",
            "--command",
            "ls",
        ]) {
            Commands::Workspace(WorkspaceCommands::Create { name, cwd, command }) => {
                assert_eq!(name.as_deref(), Some("Plan"));
                assert_eq!(cwd.as_deref(), Some("/srv"));
                assert_eq!(command.as_deref(), Some("ls"));
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["workspace", "select", "--workspace", "workspace:2"]) {
            Commands::Workspace(WorkspaceCommands::Select { id, workspace }) => {
                assert!(id.is_none());
                assert_eq!(workspace.as_deref(), Some("workspace:2"));
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["workspace", "close", WORKSPACE]) {
            Commands::Workspace(WorkspaceCommands::Close { id, workspace }) => {
                assert_eq!(id.as_deref(), Some(WORKSPACE));
                assert!(workspace.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&[
            "workspace",
            "rename",
            "--workspace",
            "workspace:2",
            "--",
            "New name",
        ]) {
            Commands::Workspace(WorkspaceCommands::Rename {
                id,
                name,
                workspace,
            }) => {
                assert_eq!(workspace.as_deref(), Some("workspace:2"));
                assert_eq!(id.as_deref(), Some("New name"));
                assert!(name.is_none());
            }
            _ => panic!("wrong command variant"),
        }

        // The namespace exists only for its six subcommands.
        assert!(Cli::try_parse_from(["cmux", "workspace"]).is_err());
        assert!(Cli::try_parse_from(["cmux", "workspace", "env"]).is_err());
        match parse(&["workspace", "list"]) {
            Commands::Workspace(WorkspaceCommands::List) => {}
            _ => panic!("wrong command variant"),
        }
        match parse(&["workspace", "current"]) {
            Commands::Workspace(WorkspaceCommands::Current) => {}
            _ => panic!("wrong command variant"),
        }
        // The legacy verbs stay available.
        assert!(matches!(
            parse(&["new-workspace", "--name", "X"]),
            Commands::NewWorkspace { .. }
        ));
        assert!(matches!(
            parse(&["select-workspace", WORKSPACE]),
            Commands::SelectWorkspace { .. }
        ));
    }

    /// `workspace-action` parses upstream's flags, its positional action and trailing value.
    #[test]
    fn workspace_action_forms_parse() {
        match parse(&[
            "workspace-action",
            "--action",
            "pin",
            "--workspace",
            "workspace:2",
        ]) {
            Commands::WorkspaceAction {
                action, workspace, ..
            } => {
                assert_eq!(action.as_deref(), Some("pin"));
                assert_eq!(workspace.as_deref(), Some("workspace:2"));
            }
            _ => panic!("wrong command variant"),
        }
        // The action can be positional, with the value's words following it.
        match parse(&["workspace-action", "rename", "My title"]) {
            Commands::WorkspaceAction {
                action,
                action_positional,
                value_words,
                ..
            } => {
                assert!(action.is_none());
                assert_eq!(action_positional.as_deref(), Some("rename"));
                assert_eq!(value_words, vec!["My title"]);
            }
            _ => panic!("wrong command variant"),
        }
        // `--name` is the alias of upstream's `--title`, and each value flag parses alone.
        match parse(&["workspace-action", "--action", "rename", "--name", "Plan"]) {
            Commands::WorkspaceAction { title, .. } => {
                assert_eq!(title.as_deref(), Some("Plan"))
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&[
            "workspace-action",
            "--action",
            "set-color",
            "--color",
            "#336699",
        ]) {
            Commands::WorkspaceAction { color, .. } => {
                assert_eq!(color.as_deref(), Some("#336699"))
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&[
            "workspace-action",
            "--action",
            "set-description",
            "--description",
            "Notes",
        ]) {
            Commands::WorkspaceAction { description, .. } => {
                assert_eq!(description.as_deref(), Some("Notes"))
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&[
            "workspace-action",
            "--action",
            "close-others",
            "--window",
            "window:1",
        ]) {
            Commands::WorkspaceAction { window, .. } => {
                assert_eq!(window.as_deref(), Some("window:1"))
            }
            _ => panic!("wrong command variant"),
        }
    }

    /// `tab-action` parses upstream's flags, its positional action and trailing title.
    #[test]
    fn tab_action_forms_parse() {
        match parse(&[
            "tab-action",
            "--action",
            "close-right",
            "--surface",
            "surface:2",
        ]) {
            Commands::TabAction {
                action, surface, ..
            } => {
                assert_eq!(action.as_deref(), Some("close-right"));
                assert_eq!(surface.as_deref(), Some("surface:2"));
            }
            _ => panic!("wrong command variant"),
        }
        // `--tab` is upstream's spelling of the same target flag.
        match parse(&["tab-action", "--action", "pin", "--tab", "surface:3"]) {
            Commands::TabAction { surface, .. } => {
                assert_eq!(surface.as_deref(), Some("surface:3"))
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["tab-action", "rename", "build", "logs"]) {
            Commands::TabAction {
                action,
                action_positional,
                value_words,
                ..
            } => {
                assert!(action.is_none());
                assert_eq!(action_positional.as_deref(), Some("rename"));
                assert_eq!(value_words, vec!["build", "logs"]);
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&[
            "tab-action",
            "--action",
            "new-terminal-right",
            "--focus",
            "true",
            "--workspace",
            "workspace:2",
            "--window",
            "window:1",
        ]) {
            Commands::TabAction {
                focus,
                workspace,
                window,
                ..
            } => {
                assert_eq!(focus, Some(true));
                assert_eq!(workspace.as_deref(), Some("workspace:2"));
                assert_eq!(window.as_deref(), Some("window:1"));
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["tab-action", "--action", "rename", "--title", "Logs"]) {
            Commands::TabAction { title, .. } => assert_eq!(title.as_deref(), Some("Logs")),
            _ => panic!("wrong command variant"),
        }
        assert!(Cli::try_parse_from(["cmux", "tab-action", "--focus", "maybe"]).is_err());
    }

    /// `move-tab-to-new-workspace` parses its flags, with `detach-tab` as the alias.
    #[test]
    fn move_tab_to_new_workspace_forms_parse() {
        match parse(&[
            "move-tab-to-new-workspace",
            "--surface",
            "surface:2",
            "--title",
            "build logs",
            "--focus",
            "true",
        ]) {
            Commands::MoveTabToNewWorkspace {
                surface,
                title,
                focus,
                workspace,
                window,
            } => {
                assert_eq!(surface.as_deref(), Some("surface:2"));
                assert_eq!(title.as_deref(), Some("build logs"));
                assert_eq!(focus, Some(true));
                assert!(workspace.is_none() && window.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        // `--tab` is upstream's spelling of the same flag, `detach-tab` of the command.
        match parse(&[
            "detach-tab",
            "--tab",
            "surface:3",
            "--workspace",
            "workspace:2",
        ]) {
            Commands::MoveTabToNewWorkspace {
                surface, workspace, ..
            } => {
                assert_eq!(surface.as_deref(), Some("surface:3"));
                assert_eq!(workspace.as_deref(), Some("workspace:2"));
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["move-tab-to-new-workspace"]) {
            Commands::MoveTabToNewWorkspace { focus, surface, .. } => {
                assert!(focus.is_none() && surface.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        // Upstream's wrapper takes no `--action`.
        assert!(
            Cli::try_parse_from(["cmux", "move-tab-to-new-workspace", "--action", "pin"]).is_err()
        );
    }

    /// `trigger-flash` parses upstream's target flags with the `--panel` alias.
    #[test]
    fn trigger_flash_forms_parse() {
        match parse(&["trigger-flash"]) {
            Commands::TriggerFlash {
                surface,
                workspace,
                window,
            } => {
                assert!(surface.is_none() && workspace.is_none() && window.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&[
            "trigger-flash",
            "--surface",
            "surface:3",
            "--workspace",
            "workspace:2",
            "--window",
            "window:1",
        ]) {
            Commands::TriggerFlash {
                surface,
                workspace,
                window,
            } => {
                assert_eq!(surface.as_deref(), Some("surface:3"));
                assert_eq!(workspace.as_deref(), Some("workspace:2"));
                assert_eq!(window.as_deref(), Some("window:1"));
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&["trigger-flash", "--panel", "surface:4"]) {
            Commands::TriggerFlash { surface, .. } => {
                assert_eq!(surface.as_deref(), Some("surface:4"))
            }
            _ => panic!("wrong command variant"),
        }
    }

    /// `surface-health` parses upstream's workspace and window context.
    #[test]
    fn surface_health_forms_parse() {
        match parse(&["surface-health"]) {
            Commands::SurfaceHealth { workspace, window } => {
                assert!(workspace.is_none() && window.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        match parse(&[
            "surface-health",
            "--workspace",
            "workspace:2",
            "--window",
            "window:1",
        ]) {
            Commands::SurfaceHealth { workspace, window } => {
                assert_eq!(workspace.as_deref(), Some("workspace:2"));
                assert_eq!(window.as_deref(), Some("window:1"));
            }
            _ => panic!("wrong command variant"),
        }
        // The single-surface `health` keeps its own `--id` flag.
        match parse(&["health", "--id", "surface:3"]) {
            Commands::Health { id } => assert_eq!(id.as_deref(), Some("surface:3")),
            _ => panic!("wrong command variant"),
        }
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

#[cfg(test)]
mod config_argument_tests {
    use super::*;

    /// The settings verbs parse with their documented flags, file overrides and aliases.
    #[test]
    fn config_verbs_parse_with_file_override_and_aliases() {
        let cli = Cli::try_parse_from(["cmux", "config", "path"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Config(ConfigCommands::Path)
        ));

        let cli = Cli::try_parse_from([
            "cmux",
            "config",
            "validate",
            "--file",
            "/tmp/cmux.json",
        ])
        .unwrap();
        let Commands::Config(ConfigCommands::Validate { file, scope }) = &cli.command else {
            panic!("wrong command");
        };
        assert_eq!(file.as_deref(), Some(std::path::Path::new("/tmp/cmux.json")));
        assert!(scope.is_none());

        for alias in ["validate", "doctor", "check"] {
            assert!(Cli::try_parse_from(["cmux", "config", alias]).is_ok(), "{alias}");
        }

        let cli = Cli::try_parse_from(["cmux", "config", "get", "sidebar.branchLayout"]).unwrap();
        let Commands::Config(ConfigCommands::Get { path, file }) = &cli.command else {
            panic!("wrong command");
        };
        assert_eq!(path, "sidebar.branchLayout");
        assert!(file.is_none());

        let cli = Cli::try_parse_from(["cmux", "--json", "config", "list-supported"]).unwrap();
        assert!(cli.json);
        assert!(matches!(
            cli.command,
            Commands::Config(ConfigCommands::ListSupported)
        ));

        // `get` needs a path; `--file` is not a substitute for one.
        assert!(Cli::try_parse_from(["cmux", "config", "get"]).is_err());
    }

    /// The editing verbs take a dotted path, a JSON value and both writing flags.
    #[test]
    fn config_set_and_unset_parse_their_arguments() {
        let cli = Cli::try_parse_from([
            "cmux",
            "config",
            "set",
            "app.appearance",
            "dark",
            "--file",
            "/tmp/cmux.json",
            "--scope",
            "project",
        ])
        .unwrap();
        let Commands::Config(ConfigCommands::Set {
            path,
            value,
            file,
            scope,
        }) = &cli.command
        else {
            panic!("wrong command");
        };
        assert_eq!(path, "app.appearance");
        assert_eq!(value, "dark");
        assert_eq!(file.as_deref(), Some(std::path::Path::new("/tmp/cmux.json")));
        assert!(matches!(scope, Some(ScopeArg::Project)));

        let cli = Cli::try_parse_from(["cmux", "config", "unset", "app.appearance"]).unwrap();
        let Commands::Config(ConfigCommands::Unset { path, file, scope }) = &cli.command else {
            panic!("wrong command");
        };
        assert_eq!(path, "app.appearance");
        assert!(file.is_none());
        assert!(scope.is_none());

        // A value starting with a dash still belongs to `set`, not to the flag parser.
        let cli = Cli::try_parse_from(["cmux", "config", "set", "a.b", "-12"]).unwrap();
        let Commands::Config(ConfigCommands::Set { value, .. }) = &cli.command else {
            panic!("wrong command");
        };
        assert_eq!(value, "-12");

        // `set` without a value and an unknown scope name are both rejected up front.
        assert!(Cli::try_parse_from(["cmux", "config", "set", "a.b"]).is_err());
        assert!(Cli::try_parse_from([
            "cmux", "config", "validate", "--scope", "workspace"
        ])
        .is_err());
    }

    /// `docs` answers to the macOS spelling too, and takes no arguments.
    #[test]
    fn config_docs_parses_with_its_alias() {
        for name in ["docs", "documentation"] {
            let cli = Cli::try_parse_from(["cmux", "config", name]).unwrap();
            assert!(
                matches!(cli.command, Commands::Config(ConfigCommands::Docs)),
                "{name}"
            );
        }
        assert!(Cli::try_parse_from(["cmux", "config", "docs", "extra"]).is_err());
    }
}

#[cfg(test)]
mod browser_verb_tests {
    use super::*;

    /// Parse one browser invocation and return its command.
    fn parse_browser(arguments: &[&str]) -> Commands {
        let mut invocation = vec!["cmux"];
        invocation.extend_from_slice(arguments);
        Cli::try_parse_from(invocation)
            .unwrap_or_else(|error| panic!("{arguments:?} should parse: {error}"))
            .command
    }

    /// The viewport, cookies and storage verbs parse with the surface first.
    #[test]
    fn advanced_browser_verbs_parse_with_the_surface_first() {
        match parse_browser(&["browser", "viewport", "surface:3", "800", "600"]) {
            Commands::Browser(BrowserCommand::Viewport {
                surface,
                width,
                height,
            }) => {
                assert_eq!(surface, "surface:3");
                assert_eq!(width, "800");
                assert_eq!(height, Some(600));
            }
            _ => panic!("wrong command variant"),
        }
        match parse_browser(&["browser", "viewport", "surface:3", "reset"]) {
            Commands::Browser(BrowserCommand::Viewport {
                surface,
                width,
                height,
            }) => {
                assert_eq!(surface, "surface:3");
                assert_eq!(width, "reset");
                assert!(height.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        match parse_browser(&["browser", "cookies", "surface:3"]) {
            Commands::Browser(BrowserCommand::Cookies { action, .. }) => {
                assert_eq!(action, "get");
            }
            _ => panic!("wrong command variant"),
        }
        match parse_browser(&[
            "browser",
            "cookies",
            "surface:3",
            "set",
            "id",
            "7",
            "--domain",
            "example.com",
        ]) {
            Commands::Browser(BrowserCommand::Cookies {
                action,
                name,
                value,
                domain,
                ..
            }) => {
                assert_eq!(action, "set");
                assert_eq!(name.as_deref(), Some("id"));
                assert_eq!(value.as_deref(), Some("7"));
                assert_eq!(domain.as_deref(), Some("example.com"));
            }
            _ => panic!("wrong command variant"),
        }
        match parse_browser(&["browser", "storage", "surface:3", "session"]) {
            Commands::Browser(BrowserCommand::Storage { surface, store }) => {
                assert_eq!(surface, "surface:3");
                assert_eq!(store, "session");
            }
            _ => panic!("wrong command variant"),
        }
        // Unknown cookie actions and stores never parse.
        assert!(
            Cli::try_parse_from(["cmux", "browser", "cookies", "surface:3", "freeze"]).is_err()
        );
        assert!(
            Cli::try_parse_from(["cmux", "browser", "storage", "surface:3", "disk"]).is_err()
        );
    }

    /// `--expires` is a Unix timestamp: the daemon rejects it as a string.
    #[test]
    fn browser_cookie_expires_must_be_a_number() {
        assert!(Cli::try_parse_from([
            "cmux", "browser", "cookies", "surface:1", "set", "k", "v", "--expires", "2999999999",
        ])
        .is_ok());
        assert!(Cli::try_parse_from([
            "cmux", "browser", "cookies", "surface:1", "set", "k", "v", "--expires", "demain",
        ])
        .is_err());
    }

    /// The script and download verbs parse with the surface first; missing payloads never parse.
    #[test]
    fn script_and_download_verbs_parse_with_the_surface_first() {
        match parse_browser(&["browser", "addinitscript", "surface:3", "window.x = 1;"]) {
            Commands::Browser(BrowserCommand::AddInitScript { surface, script }) => {
                assert_eq!(surface, "surface:3");
                assert_eq!(script, "window.x = 1;");
            }
            _ => panic!("wrong command variant"),
        }
        match parse_browser(&["browser", "addstyle", "surface:3", "body { color: red; }"]) {
            Commands::Browser(BrowserCommand::AddStyle { surface, css }) => {
                assert_eq!(surface, "surface:3");
                assert_eq!(css, "body { color: red; }");
            }
            _ => panic!("wrong command variant"),
        }
        match parse_browser(&["browser", "addscript", "surface:3", "alert(1);"]) {
            Commands::Browser(BrowserCommand::AddScript { surface, script }) => {
                assert_eq!(surface, "surface:3");
                assert_eq!(script, "alert(1);");
            }
            _ => panic!("wrong command variant"),
        }
        match parse_browser(&["browser", "download", "surface:3", "#dl", "/tmp/file.zip"]) {
            Commands::Browser(BrowserCommand::Download {
                surface,
                selector,
                path,
            }) => {
                assert_eq!(surface, "surface:3");
                assert_eq!(selector, "#dl");
                assert_eq!(path, "/tmp/file.zip");
            }
            _ => panic!("wrong command variant"),
        }
        match parse_browser(&[
            "browser",
            "download-wait",
            "surface:3",
            "--timeout",
            "4000",
            "--path",
            "/tmp/file.zip",
        ]) {
            Commands::Browser(BrowserCommand::DownloadWait {
                surface,
                timeout,
                path,
            }) => {
                assert_eq!(surface, "surface:3");
                assert_eq!(timeout, Some(4000));
                assert_eq!(path.as_deref(), Some("/tmp/file.zip"));
            }
            _ => panic!("wrong command variant"),
        }
        match parse_browser(&["browser", "download-wait", "surface:3"]) {
            Commands::Browser(BrowserCommand::DownloadWait {
                surface,
                timeout,
                path,
            }) => {
                assert_eq!(surface, "surface:3");
                assert!(timeout.is_none());
                assert!(path.is_none());
            }
            _ => panic!("wrong command variant"),
        }
        // A missing script, selector, path or timeout value never parses.
        assert!(Cli::try_parse_from(["cmux", "browser", "addinitscript", "surface:3"]).is_err());
        assert!(Cli::try_parse_from(["cmux", "browser", "addstyle", "surface:3"]).is_err());
        assert!(Cli::try_parse_from(["cmux", "browser", "addscript", "surface:3"]).is_err());
        assert!(
            Cli::try_parse_from(["cmux", "browser", "download", "surface:3", "#dl"]).is_err()
        );
        assert!(
            Cli::try_parse_from(["cmux", "browser", "download-wait", "surface:3", "--timeout"])
                .is_err()
        );
    }
}

#[cfg(test)]
mod markdown_argument_tests {
    use super::*;

    /// Parse one markdown invocation and return its command.
    fn parse_markdown(arguments: &[&str]) -> Commands {
        let mut invocation = vec!["cmux"];
        invocation.extend_from_slice(arguments);
        Cli::try_parse_from(invocation)
            .unwrap_or_else(|error| panic!("{arguments:?} should parse: {error}"))
            .command
    }

    /// A bare file parses with the viewer defaults (no workspace, kept focus).
    #[test]
    fn markdown_file_parses_with_defaults() {
        match parse_markdown(&["markdown", "notes.md"]) {
            Commands::Markdown {
                file,
                workspace,
                surface,
                focus,
                no_focus,
            } => {
                assert_eq!(file, std::path::PathBuf::from("notes.md"));
                assert!(workspace.is_none());
                assert!(surface.is_none());
                assert!(!focus);
                assert!(!no_focus);
            }
            _ => panic!("wrong command variant"),
        }
    }

    /// Workspace, surface and focus flags reach the markdown viewer.
    #[test]
    fn markdown_options_parse() {
        match parse_markdown(&[
            "markdown",
            "notes.md",
            "--workspace",
            "workspace:2",
            "--surface",
            "surface:3",
            "--focus",
        ]) {
            Commands::Markdown {
                file,
                workspace,
                surface,
                focus,
                no_focus,
            } => {
                assert_eq!(file, std::path::PathBuf::from("notes.md"));
                assert_eq!(workspace.as_deref(), Some("workspace:2"));
                assert_eq!(surface.as_deref(), Some("surface:3"));
                assert!(focus);
                assert!(!no_focus);
            }
            _ => panic!("wrong command variant"),
        }
        match parse_markdown(&["markdown", "notes.md", "--no-focus"]) {
            Commands::Markdown {
                focus, no_focus, ..
            } => {
                assert!(!focus);
                assert!(no_focus);
            }
            _ => panic!("wrong command variant"),
        }
    }

    /// The file is required, and focus flags contradict each other.
    #[test]
    fn markdown_rejects_missing_file_and_conflicting_focus() {
        assert!(Cli::try_parse_from(["cmux", "markdown"]).is_err());
        assert!(Cli::try_parse_from(["cmux", "markdown", "a.md", "--focus", "--no-focus"]).is_err());
    }
}
