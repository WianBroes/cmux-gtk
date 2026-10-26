use serde_json::Value;

pub type RespTx = tokio::sync::oneshot::Sender<Value>;

/// Commands dispatched from tokio accept loop to GTK main thread.
/// All variants carry `req_id` (echoed in response) and `resp_tx` (result channel).
/// GTK/AppState reads and mutations happen ONLY in handlers.rs on the main thread.
#[allow(dead_code)]
pub enum SocketCommand {
    /// Carry transport correlation and queue timing into the GTK dispatcher.
    Observed {
        command: Box<SocketCommand>,
        trace_id: uuid::Uuid,
        queued_at: std::time::Instant,
    },
    /// Resolve `window:N` / `workspace:N` / `surface:N` refs to ids before a request is decoded.
    ResolveHandles {
        req_id: Value,
        refs: Vec<String>,
        resp_tx: RespTx,
    },
    // -- system.* --
    Ping {
        req_id: Value,
        resp_tx: RespTx,
    },
    Identify {
        req_id: Value,
        /// Optional `{workspace_id, surface_id}` naming the calling terminal.
        caller: Option<Value>,
        resp_tx: RespTx,
    },
    Capabilities {
        req_id: Value,
        resp_tx: RespTx,
    },
    /// Show, hide or toggle the right sidebar (upstream `cmux right-sidebar …`).
    RightSidebarApply {
        req_id: Value,
        /// One of `toggle`, `show`, `hide`, `set`; validated in dispatch.
        action: String,
        /// Mode for `set`; only `files` exists on Linux so far.
        mode: Option<String>,
        resp_tx: RespTx,
    },

    /// Explicitly submit a reviewed project command after checking its fingerprint and live context.
    ProjectActionRun {
        req_id: Value,
        workspace: Option<uuid::Uuid>,
        action_id: String,
        fingerprint: String,
        confirmed: bool,
        resp_tx: RespTx,
    },
    /// Resolve project actions from a captured local workspace directory on a bounded worker.
    ProjectActionsList {
        req_id: Value,
        workspace: Option<uuid::Uuid>,
        resp_tx: RespTx,
    },

    /// Read the latest listener snapshot for a validated workspace/surface scope.
    PortsList {
        req_id: Value,
        workspace: Option<uuid::Uuid>,
        surface: Option<uuid::Uuid>,
        resp_tx: RespTx,
    },
    WorkspaceMetadata {
        req_id: Value,
        workspace: Option<uuid::Uuid>,
        action: crate::workspace_metadata::Action,
        resp_tx: RespTx,
    },
    // -- workspace.* --
    WorkspaceList {
        req_id: Value,
        resp_tx: RespTx,
    },
    /// Mobile workspace snapshot with each entry's active terminal surface and host status.
    MobileWorkspaceList {
        req_id: Value,
        resp_tx: RespTx,
    },
    WorkspaceCurrent {
        req_id: Value,
        resp_tx: RespTx,
    },
    WorkspaceCreate {
        req_id: Value,
        remote_target: Option<String>,
        name: Option<String>,
        working_directory: Option<std::path::PathBuf>,
        remote_directory: Option<String>,
        terminal_transport: crate::remote_transport::TerminalTransport,
        terminal_profile: crate::remote_transport::TerminalProfile,
        terminal_tmux_session: Option<String>,
        /// Typed into the first terminal's shell, followed by Enter (upstream `initial_input`).
        initial_input: Option<String>,
        /// Select the new workspace; upstream defaults to keeping the current selection.
        focus: bool,
        resp_tx: RespTx,
    },
    /// `id` is the workspace UUID string from the client.
    WorkspaceSelect {
        req_id: Value,
        id: String,
        resp_tx: RespTx,
    },
    WorkspaceClose {
        req_id: Value,
        id: String,
        resp_tx: RespTx,
    },
    WorkspaceRename {
        req_id: Value,
        id: String,
        name: String,
        resp_tx: RespTx,
    },
    /// `workspace.set_description` / `workspace.clear_description`; None clears the description.
    WorkspaceDescription {
        req_id: Value,
        id: String,
        description: Option<String>,
        resp_tx: RespTx,
    },
    WorkspaceNext {
        req_id: Value,
        resp_tx: RespTx,
    },
    WorkspacePrev {
        req_id: Value,
        resp_tx: RespTx,
    },
    WorkspaceLast {
        req_id: Value,
        resp_tx: RespTx,
    },
    WorkspaceReorder {
        req_id: Value,
        id: String,
        /// Absolute destination slot; `None` places relative to `before` or `after`.
        position: Option<usize>,
        /// Anchor workspace: the moved one lands immediately before this one.
        before: Option<String>,
        /// Anchor workspace: the moved one lands immediately after this one.
        after: Option<String>,
        /// Report the resolved final index without touching the order.
        dry_run: bool,
        resp_tx: RespTx,
    },
    WorkspaceReorderMany {
        req_id: Value,
        order: Vec<uuid::Uuid>,
        dry_run: bool,
        resp_tx: RespTx,
    },
    /// One `workspace.action` call: an action name plus the values its arm reads.
    WorkspaceAction {
        req_id: Value,
        /// Action as written by the caller; the handler normalizes and validates it.
        action: String,
        /// Target workspace UUID; None targets the active workspace, as upstream.
        workspace: Option<String>,
        /// `rename` title.
        title: Option<String>,
        /// `set-color` value.
        color: Option<String>,
        /// `set-description` text.
        description: Option<String>,
        resp_tx: RespTx,
    },
    WorkspaceGroupList {
        req_id: Value,
        resp_tx: RespTx,
    },
    WorkspaceGroupCreate {
        req_id: Value,
        name: String,
        color: Option<String>,
        resp_tx: RespTx,
    },
    WorkspaceGroupUpdate {
        req_id: Value,
        id: uuid::Uuid,
        name: Option<String>,
        color: Option<Option<String>>,
        collapsed: Option<bool>,
        position: Option<usize>,
        resp_tx: RespTx,
    },
    WorkspaceGroupAssign {
        req_id: Value,
        id: Option<uuid::Uuid>,
        workspaces: Vec<uuid::Uuid>,
        resp_tx: RespTx,
    },
    WorkspaceGroupDelete {
        req_id: Value,
        id: uuid::Uuid,
        resp_tx: RespTx,
    },

    // -- surface.* (implemented in Plan 04) --
    SurfaceList {
        req_id: Value,
        /// Restrict to one workspace; `None` uses the selected workspace.
        workspace: Option<uuid::Uuid>,
        /// List every workspace when no workspace is named.
        all_workspaces: bool,
        resp_tx: RespTx,
    },
    SurfaceCurrent {
        req_id: Value,
        /// Workspace to inspect; `None` uses the selected workspace.
        workspace: Option<uuid::Uuid>,
        resp_tx: RespTx,
    },
    SurfaceResume {
        req_id: Value,
        id: Option<String>,
        action: crate::resume::ResumeAction,
        resp_tx: RespTx,
    },
    /// Split a pane with a new terminal (`surface.split`, `pane.create`). The target is the
    /// surface `id`, else the pane `pane`, else the active pane of `workspace`, of the `caller`
    /// surface's workspace, or of the window.
    SurfaceSplit {
        req_id: Value,
        id: Option<String>,
        workspace: Option<String>,
        caller: Option<String>,
        pane: Option<String>,
        direction: crate::split_engine::FocusDirection,
        launch: crate::split_engine::TerminalLaunch,
        focus: bool,
        resp_tx: RespTx,
    },
    /// Add a terminal tab to a pane (`surface.create`), with the same target rules.
    SurfaceCreate {
        req_id: Value,
        workspace: Option<String>,
        caller: Option<String>,
        pane: Option<String>,
        launch: crate::split_engine::TerminalLaunch,
        focus: bool,
        resp_tx: RespTx,
    },
    SurfaceFocus {
        req_id: Value,
        id: String,
        resp_tx: RespTx,
    },
    SurfaceClose {
        req_id: Value,
        id: String,
        resp_tx: RespTx,
    },
    SurfaceMove {
        req_id: Value,
        id: String,
        workspace: Option<String>,
        pane: Option<String>,
        position: Option<usize>,
        /// Anchor surface: the moved surface lands immediately before this one.
        before: Option<String>,
        /// Anchor surface: the moved surface lands immediately after this one.
        after: Option<String>,
        focus: bool,
        resp_tx: RespTx,
    },
    SurfaceReorder {
        req_id: Value,
        id: String,
        /// Absolute destination slot; `None` places relative to `before` or `after`.
        position: Option<usize>,
        /// Anchor surface of the same pane: the moved one lands immediately before it.
        before: Option<String>,
        /// Anchor surface of the same pane: the moved one lands immediately after it.
        after: Option<String>,
        /// Select the reordered surface after the move; false keeps the current selection.
        focus: bool,
        resp_tx: RespTx,
    },
    /// One `tab.action` call: an action name plus the target tab it resolves.
    TabAction {
        req_id: Value,
        /// Action as written by the caller; the handler normalizes, aliases and validates it.
        action: String,
        /// Target surface UUID; None targets the focused tab, as upstream.
        surface: Option<String>,
        /// Workspace UUID when the caller names one instead of the surface's own.
        workspace: Option<String>,
        /// `move-to-new-workspace` names the workspace it creates.
        title: Option<String>,
        /// Move focus to the destination where the action supports it (creation only).
        focus: bool,
        resp_tx: RespTx,
    },
    /// Light the visual attention markers of one tab (upstream `surface.trigger_flash`).
    SurfaceTriggerFlash {
        req_id: Value,
        /// Target surface UUID; None targets the focused tab, as upstream.
        surface: Option<String>,
        /// Workspace UUID when the caller names one instead of the surface's own.
        workspace: Option<String>,
        resp_tx: RespTx,
    },
    SurfaceDragToSplit {
        req_id: Value,
        id: String,
        target_pane: String,
        direction: crate::split_engine::FocusDirection,
        resp_tx: RespTx,
    },
    /// Split the surface's own pane and move the tab into the new split (`surface.split_off`).
    SurfaceSplitOff {
        req_id: Value,
        id: String,
        direction: crate::split_engine::FocusDirection,
        /// Move focus with the split; upstream's CLI keeps focus when absent.
        focus: bool,
        resp_tx: RespTx,
    },
    SurfaceSendText {
        req_id: Value,
        id: Option<String>,
        /// Without `id`: the focused terminal of this workspace instead of the selected one.
        workspace: Option<String>,
        text: String,
        resp_tx: RespTx,
    },
    SurfaceSendKey {
        req_id: Value,
        id: Option<String>,
        workspace: Option<String>,
        key: String,
        resp_tx: RespTx,
    },
    /// Mouse-wheel scroll of `delta_lines` lines on a terminal (mobile.terminal.scroll).
    SurfaceMouseScroll {
        req_id: Value,
        id: Option<String>,
        delta_lines: f64,
        resp_tx: RespTx,
    },
    SurfaceReadText {
        req_id: Value,
        scrollback: bool,
        /// Also return the styled render-grid frame (mobile only).
        render_grid: bool,
        id: Option<String>,
        workspace: Option<String>,
        resp_tx: RespTx,
    },
    /// Shared terminal sizing for mobile viewers (mobile only), see `mobile_sizing_host`.
    MobileSizing {
        op: MobileSizingOp,
        resp_tx: RespTx,
    },
    /// Rows just above the viewport, for the mobile history (mobile only).
    SurfaceReadHistory {
        req_id: Value,
        id: Option<String>,
        max_lines: u64,
        resp_tx: RespTx,
    },
    SurfaceHealth {
        req_id: Value,
        id: Option<String>,
        /// Workspace whose surfaces are listed instead of the single focused/targeted one.
        workspace: Option<String>,
        resp_tx: RespTx,
    },
    SurfaceRefresh {
        req_id: Value,
        id: Option<String>,
        resp_tx: RespTx,
    },

    // -- pane.* (implemented in Plan 04) --
    PaneList {
        req_id: Value,
        /// Restrict to one workspace; `None` lists every workspace.
        workspace: Option<uuid::Uuid>,
        resp_tx: RespTx,
    },
    PaneSurfaces {
        req_id: Value,
        /// Workspace holding the pane; `None` finds its owner or uses the selected workspace.
        workspace: Option<uuid::Uuid>,
        /// Pane reference (`pane:N`) or one of its surface UUIDs; `None` uses the focused pane.
        pane: Option<String>,
        resp_tx: RespTx,
    },
    PaneFocus {
        req_id: Value,
        id: Option<String>,
        resp_tx: RespTx,
    },
    /// Exchange the selected surfaces of two panes in the source pane's workspace.
    /// Upstream `pane.join`: `surface.move` of a surface (or a pane's selected one) into a pane.
    PaneJoin {
        req_id: Value,
        target_pane: String,
        surface: Option<String>,
        source_pane: Option<String>,
        /// None keeps `surface.move`'s own default, as upstream forwards focus only when given.
        focus: Option<bool>,
        resp_tx: RespTx,
    },
    /// Upstream `pane.break`: move a surface out into a new workspace.
    PaneBreak {
        req_id: Value,
        /// Source pane reference (`pane:N`); None takes the workspace's focused pane.
        pane: Option<String>,
        /// Source surface UUID; None takes the source pane's selected surface.
        surface: Option<String>,
        /// Source workspace UUID; None takes the surface's own, else the selected one.
        workspace: Option<String>,
        focus: bool,
        resp_tx: RespTx,
    },
    PaneSwap {
        req_id: Value,
        /// Source pane reference (`pane:N`) or one of its surface UUIDs.
        pane_id: String,
        /// Target pane reference in the source pane's workspace.
        target_pane_id: String,
        /// Focus the target pane after the swap; false preserves the focused pane.
        focus: bool,
        resp_tx: RespTx,
    },
    PaneLast {
        req_id: Value,
        resp_tx: RespTx,
    },

    SettingsOpen {
        req_id: Value,
        resp_tx: RespTx,
    },
    // -- config.* --
    ConfigReload {
        req_id: Value,
        resp_tx: RespTx,
    },

    // -- window.* --
    WindowList {
        req_id: Value,
        resp_tx: RespTx,
    },
    WindowCurrent {
        req_id: Value,
        resp_tx: RespTx,
    },

    // -- debug.* --
    DebugLayout {
        req_id: Value,
        resp_tx: RespTx,
    },
    DebugTerminalIsFocused {
        req_id: Value,
        surface_id: String,
        resp_tx: RespTx,
    },
    DebugRenderStats {
        req_id: Value,
        /// Surface UUID; None takes the selected workspace's focused terminal.
        surface_id: Option<String>,
        resp_tx: RespTx,
    },
    DebugPanelSnapshot {
        req_id: Value,
        surface_id: String,
        label: String,
        resp_tx: RespTx,
    },
    DebugPanelSnapshotReset {
        req_id: Value,
        surface_id: String,
        resp_tx: RespTx,
    },
    DebugShortcutSet {
        req_id: Value,
        name: String,
        combo: String,
        resp_tx: RespTx,
    },
    DebugShortcutSimulate {
        req_id: Value,
        combo: String,
        resp_tx: RespTx,
    },
    DebugAppActivate {
        req_id: Value,
        resp_tx: RespTx,
    },
    WindowFocus {
        req_id: Value,
        window: String,
        resp_tx: RespTx,
    },
    DebugType {
        req_id: Value,
        text: String,
        resp_tx: RespTx,
    },

    // -- notification.* (Phase 4: NOTF-01, NOTF-02) --
    NotificationList {
        req_id: Value,
        resp_tx: RespTx,
    },
    Inbox {
        req_id: Value,
        action: crate::inbox::Action,
        resp_tx: RespTx,
    },

    // -- browser.* (Phase 8: D-04 lifecycle + streaming) --
    BrowserOpen {
        req_id: Value,
        url: String,
        workspace: Option<String>,
        profile: Option<String>,
        resp_tx: RespTx,
    },
    /// Open a browser to the right of a source surface or pane, reusing its right neighbor.
    BrowserOpenSplit {
        req_id: Value,
        url: String,
        source: Option<String>,
        pane: Option<String>,
        workspace: Option<String>,
        profile: Option<String>,
        focus: bool,
        resp_tx: RespTx,
    },
    BrowserStreamEnable {
        req_id: Value,
        resp_tx: RespTx,
    },
    BrowserStreamDisable {
        req_id: Value,
        resp_tx: RespTx,
    },
    BrowserList {
        req_id: Value,
        resp_tx: RespTx,
    },

    // -- browser.* generic proxy (P0/P1 parity) --
    BrowserAction {
        req_id: Value,
        action: String,
        params: Value,
        surface_ref: Option<String>,
        resp_tx: RespTx,
    },
}

/// One shared-sizing event for a terminal (upstream `mobile.terminal.viewport`,
/// activity, detach). Answers `{"size_state": …}`.
pub enum MobileSizingOp {
    Viewport {
        surface_id: String,
        participant: crate::mobile_sizing::TerminalSizingParticipant,
    },
    Detach {
        surface_id: String,
        participant_id: String,
    },
    Activity {
        surface_id: String,
        participant_id: String,
    },
    /// Re-measure the PC pane (window resized) and return the current state.
    Refresh {
        surface_id: String,
    },
}
