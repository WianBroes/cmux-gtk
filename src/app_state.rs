use crate::ghostty::ffi;
use crate::split_engine::SplitEngine;
use crate::workspace::{ConnectionState, Workspace};
use gtk4::prelude::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub type AppStateRef = Rc<RefCell<AppState>>;

pub struct AppState {
    pub split_engines: Vec<SplitEngine>,
    pub gtk_app: gtk4::Application,
    /// All open workspaces. Never empty after initialization — create_workspace is called in new().
    pub workspaces: Vec<Workspace>,
    /// Ordered persistent workspace groups rendered independently of model indices.
    pub workspace_groups: Vec<crate::workspace_group::WorkspaceGroup>,
    /// Index into workspaces of the currently visible workspace.
    pub active_index: usize,
    /// GtkStack holding one page per workspace (the workspace's root GTK widget).
    pub stack: gtk4::Stack,
    /// GtkListBox in the sidebar showing workspace names.
    pub sidebar_list: gtk4::ListBox,
    /// Right sidebar panel (Files); None only in widget tests building a bare AppState.
    pub right_sidebar: Option<crate::right_sidebar::RightSidebar>,
    /// Ghostty app handle — used by create_surface() for new panes.
    pub ghostty_app: ffi::ghostty_app_t,
    /// Next workspace ID (monotonically increasing).
    next_id: u64,
    /// Next display number for default names ("Workspace N").
    next_display_number: usize,
    /// Bounded retained messages, separate from transient terminal BEL attention.
    pub inbox: crate::inbox::Inbox,
    /// Coalesced change signal and non-owning panel reference; the panel owns its cancellable listener.
    pub inbox_updates: Option<tokio::sync::watch::Sender<()>>,
    pub inbox_window: glib::WeakRef<gtk4::Dialog>,
    /// Header bell counter of unread notifications.
    pub notifications_badge: Option<gtk4::Label>,
    /// Focus Back / Forward positions, and the titlebar arrows' actions kept in step with them.
    pub focus_history: crate::focus_history::FocusHistory,
    pub focus_history_actions: Option<(gtk4::gio::SimpleAction, gtk4::gio::SimpleAction)>,
    /// Validated application-owned authority for automatic local terminal resume.
    pub resume_policy: crate::resume_policy::ResumePolicy,
    /// Sender for session snapshots to the debounce task.
    /// Each mutation snapshots SessionData on the main thread and sends it here.
    pub session_tx: Option<tokio::sync::watch::Sender<Option<crate::session::Snapshot>>>,
    /// Sender for SSH events (cloned into SSH lifecycle tokio tasks).
    pub ssh_event_tx: Option<crate::ssh::SshEventTx>,
    /// Tokio runtime handle for spawning SSH lifecycle tasks.
    pub runtime_handle: Option<tokio::runtime::Handle>,
    /// Handles to SSH lifecycle tasks, keyed by workspace id. Used for cleanup on close.
    pub ssh_task_handles: std::collections::HashMap<u64, tokio::task::JoinHandle<()>>,
    /// Maps workspace_id -> SshBridge for remote workspaces.
    pub workspace_bridges:
        std::collections::HashMap<u64, std::sync::Arc<crate::ssh::bridge::SshBridge>>,
    /// Provisional session owned by an in-flight UI or RPC startup until a surface is created.
    pub browser_manager: Option<crate::browser::BrowserManager>,
    /// Serialize lazy browser restoration without launching hidden pages.
    pub browser_restore_gate: std::sync::Arc<tokio::sync::Semaphore>,
    /// Independent live or starting daemon sessions, keyed by the owning GTK surface UUID.
    pub browser_sessions: std::collections::HashMap<uuid::Uuid, crate::browser::BrowserManager>,
    /// Retain asynchronous daemon-close tasks for the post-GTK shutdown drain.
    pub browser_shutdown_tasks: crate::browser::ShutdownTasks,
    /// Short `window:N` / `workspace:N` / `surface:N` refs handed to socket clients.
    pub handles: crate::socket::handles::HandleRegistry,
    /// Maps short-ref ID -> surface UUID (lost on restart, per D-06).
    pub browser_surface_refs: std::collections::HashMap<u32, String>,
}

impl AppState {
    /// Read a local workspace's selected terminal CWD on GTK, falling back to its launch directory.
    pub(crate) fn local_workspace_directory(&self, index: usize) -> Option<std::path::PathBuf> {
        let workspace = self.workspaces.get(index)?;
        if workspace.remote_target.is_some() {
            return None;
        }
        let native = self.split_engines.get(index).and_then(|engine| {
            engine
                .active_pane_uuid()
                .and_then(|id| engine.find_surface_by_uuid(&id))
        });
        native
            .map(|pointer| crate::ghostty::registry::working_directory(pointer as usize))
            .filter(|value| !value.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| workspace.working_directory.clone())
    }

    /// Whether the right sidebar is on screen; false when no panel was built (widget tests).
    pub fn right_sidebar_visible(&self) -> bool {
        self.right_sidebar
            .as_ref()
            .is_some_and(|sidebar| sidebar.is_visible())
    }

    /// Show or hide the right sidebar (upstream `right-sidebar` CLI), persisting the choice.
    pub fn set_right_sidebar_visible(&self, visible: bool) {
        if let Some(sidebar) = &self.right_sidebar {
            sidebar.set_visible(visible);
        }
    }

    /// Toggle the right sidebar (upstream ⌘⌥B, here `Ctrl+Alt+B`).
    pub fn toggle_right_sidebar(&self) {
        if let Some(sidebar) = &self.right_sidebar {
            sidebar.toggle();
        }
    }

    /// Point the right sidebar's Files tree at the focused workspace's root
    /// (upstream `FileExplorerWorkspaceRootResolver`: local current directory,
    /// remote workspaces unavailable). Cheap when the root did not change.
    pub fn refresh_right_sidebar(&self) {
        let Some(sidebar) = &self.right_sidebar else {
            return;
        };
        let index = self.active_index;
        let workspace = self.workspaces.get(index);
        let is_remote = workspace.is_some_and(|workspace| workspace.remote_target.is_some());
        let directory = self.local_workspace_directory(index);
        sidebar.show_root(crate::file_explorer::resolve_root(is_remote, directory));
    }

    /// Create a new AppState. Does NOT create the first workspace — caller must call
    /// create_workspace() after constructing the GTK widget tree (Plan 04 wires this).
    pub fn new(
        stack: gtk4::Stack,
        sidebar_list: gtk4::ListBox,
        ghostty_app: ffi::ghostty_app_t,
        gtk_app: gtk4::Application,
    ) -> AppStateRef {
        let state = AppState {
            workspaces: Vec::new(),
            workspace_groups: Vec::new(),
            split_engines: Vec::new(),
            active_index: 0,
            stack,
            sidebar_list,
            right_sidebar: None,
            ghostty_app,
            gtk_app,
            next_id: 1,
            next_display_number: 1,
            session_tx: None,
            resume_policy: Default::default(),
            inbox: Default::default(),
            inbox_updates: None,
            inbox_window: Default::default(),
            notifications_badge: None,
            focus_history: Default::default(),
            focus_history_actions: None,
            ssh_event_tx: None,
            runtime_handle: None,
            ssh_task_handles: std::collections::HashMap::new(),
            workspace_bridges: std::collections::HashMap::new(),
            browser_manager: None,
            browser_sessions: std::collections::HashMap::new(),
            browser_restore_gate: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
            browser_shutdown_tasks: Default::default(),
            handles: Default::default(),
            browser_surface_refs: std::collections::HashMap::new(),
        };
        Rc::new(RefCell::new(state))
    }

    /// Create a new workspace. Allocates an ID, creates a sidebar row, and adds a placeholder
    /// page to the GtkStack. The actual GLArea/split root is added by the caller (Plan 04).
    /// Returns the new workspace id.
    pub fn create_workspace(&mut self) -> u64 {
        self.create_local_workspace(None, None, None, Default::default(), None)
    }

    /// Create a local workspace bound to an existing directory.
    pub fn create_workspace_in(
        &mut self,
        name: String,
        working_directory: &Path,
    ) -> Result<u64, String> {
        let (name, working_directory) =
            crate::workspace::prepare_local_workspace(&name, working_directory)?;
        Ok(self.create_workspace_bound(name, working_directory))
    }

    /// Create a workspace from inputs already validated off the GTK main thread.
    pub fn create_workspace_bound(&mut self, name: String, working_directory: PathBuf) -> u64 {
        self.create_local_workspace(
            Some(name),
            Some(working_directory),
            None,
            Default::default(),
            None,
        )
    }

    /// Create a local workspace whose first terminal gets `initial_input` typed into its shell.
    pub(crate) fn create_workspace_with_input(
        &mut self,
        name: Option<String>,
        working_directory: Option<PathBuf>,
        initial_input: Option<String>,
    ) -> u64 {
        self.create_local_workspace(
            name,
            working_directory,
            None,
            Default::default(),
            initial_input,
        )
    }

    /// Create from worker-validated project inputs; overrides reach the first surface before realization.
    pub(crate) fn create_workspace_configured(
        &mut self,
        name: String,
        directory: PathBuf,
        environment: std::collections::BTreeMap<String, String>,
        initial_input: Option<String>,
    ) -> u64 {
        self.create_local_workspace(
            Some(name),
            Some(directory),
            None,
            environment,
            initial_input,
        )
    }

    /// Install a worker-prepared project pane tree without constructing a placeholder terminal.
    #[allow(clippy::too_many_arguments)] // The explicit inputs form the complete launch contract.
    pub(crate) fn create_workspace_layout(
        &mut self,
        name: String,
        directory: PathBuf,
        environment: std::collections::BTreeMap<String, String>,
        color: Option<String>,
        layout: crate::split_engine::SplitNodeData,
        active_surface: &str,
    ) -> Option<u64> {
        let id = self.next_id;
        let display_number = self.next_display_number;
        let engine = crate::split_engine::SplitEngine::from_data_with_command(
            self.ghostty_app,
            &layout,
            Some(active_surface),
            Some(directory.clone()),
            None,
            None,
            None,
            &self.resume_policy,
            environment,
        )?;
        self.next_id += 1;
        self.next_display_number += 1;
        let mut workspace = Workspace::new_bound(id, display_number, name, directory);
        workspace.color = color.filter(|value| crate::workspace::valid_workspace_color(value));
        let row = self.build_sidebar_row(&workspace);
        self.sidebar_list.append(&row);
        let page_name = format!("workspace-{id}");
        self.stack
            .add_named(&engine.root_widget(), Some(&page_name));
        workspace.stack_page_name = page_name;
        self.workspaces.push(workspace);
        self.publish_workspace("workspace.created", self.workspaces.len() - 1, serde_json::json!({}));
        self.split_engines.push(engine);
        self.switch_to_index(self.workspaces.len() - 1);
        self.trigger_session_save();
        Some(id)
    }

    /// Validate the launch directory and readable script before creating a local GTK workspace.
    pub fn create_script_workspace(
        &mut self,
        name: String,
        directory: &Path,
        script: &Path,
    ) -> Result<u64, String> {
        let (name, directory) = crate::workspace::prepare_local_workspace(&name, directory)?;
        let script = crate::workspace::prepare_startup_script(script)?;
        Ok(self.create_local_workspace(
            Some(name),
            Some(directory),
            Some(script),
            Default::default(),
            None,
        ))
    }

    /// Allocate identity, construct the pane tree and sidebar row, select and schedule persistence.
    fn create_local_workspace(
        &mut self,
        name: Option<String>,
        working_directory: Option<PathBuf>,
        startup_script: Option<PathBuf>,
        environment: std::collections::BTreeMap<String, String>,
        initial_input: Option<String>,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let display_number = self.next_display_number;
        self.next_display_number += 1;

        let mut workspace = match (name, working_directory) {
            (Some(name), Some(directory)) => {
                Workspace::new_bound(id, display_number, name, directory)
            }
            _ => Workspace::new(id, display_number),
        };

        workspace.startup_script = startup_script;
        let launch_command = workspace
            .startup_script
            .as_deref()
            .map(crate::workspace::startup_command);
        let row = self.build_sidebar_row(&workspace);
        self.sidebar_list.append(&row);

        // Create surface and split engine
        let pane_id = id * 1000;
        eprintln!(
            "cmux: create_workspace calling create_surface for workspace_id={}, pane_id={}",
            id, pane_id
        );
        let (gl_area, _) = crate::ghostty::surface::create_surface(
            self.ghostty_app,
            None,
            workspace.working_directory.clone(),
            pane_id,
            crate::ghostty::surface::SurfaceIoMode::Configured {
                initial_input,
                command: launch_command.clone(),
                environment: environment.clone(),
            },
        );
        let mut engine = SplitEngine::new(
            self.ghostty_app,
            gl_area,
            pane_id,
            workspace.working_directory.clone(),
        );

        engine.launch_command = launch_command;
        engine.launch_environment = environment;

        // Add to stack
        let page_name = format!("workspace-{}", id);
        self.stack
            .add_named(&engine.root_widget(), Some(&page_name));
        workspace.stack_page_name = page_name;

        self.workspaces.push(workspace);
        self.publish_workspace("workspace.created", self.workspaces.len() - 1, serde_json::json!({}));
        self.split_engines.push(engine);

        let new_index = self.workspaces.len() - 1;
        self.switch_to_index(new_index);

        self.trigger_session_save();
        id
    }

    /// Restore a workspace from a session snapshot (SESS-02).
    /// Creates sidebar row, reconstructs the saved split tree with its launch context.
    /// Returns the workspace id, or None if tree is invalid (D-14 depth limit).
    pub fn restore_workspace(&mut self, ws: &crate::session::WorkspaceSession) -> Option<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let display_number = self.next_display_number;
        self.next_display_number += 1;

        let mut workspace = Workspace::new(id, display_number);
        workspace.name = ws.name.clone();
        workspace.custom_name = ws
            .custom_name
            .unwrap_or_else(|| !Workspace::is_default_name(&ws.name));
        workspace.metadata = ws.metadata.clone().validated();
        workspace.describe(ws.custom_description.clone());
        workspace.uuid = uuid::Uuid::parse_str(&ws.uuid).unwrap_or_else(|_| uuid::Uuid::new_v4());
        workspace.color = ws
            .color
            .clone()
            .filter(|c| crate::workspace::valid_workspace_color(c));
        workspace.group_id = ws.group_id.filter(|group_id| {
            self.workspace_groups
                .iter()
                .any(|group| group.id == *group_id)
        });
        workspace.startup_script = ws.startup_script.clone();
        workspace.remote_directory = ws
            .remote_directory
            .clone()
            .filter(|value| value.starts_with('/') && !value.contains('\0'));
        workspace.working_directory = ws.working_directory.clone();
        workspace.terminal_transport = ws.terminal_transport;
        workspace.terminal_profile = ws.terminal_profile;
        workspace.terminal_tmux_session = ws
            .terminal_tmux_session
            .clone()
            .filter(|value| crate::remote_transport::validate_tmux_session(value).is_ok());
        if matches!(
            workspace.terminal_profile,
            crate::remote_transport::TerminalProfile::Tmux
        ) && workspace.terminal_tmux_session.is_none()
        {
            workspace.terminal_tmux_session = Some("main".into());
        }

        workspace.remote_target = ws
            .remote_target
            .clone()
            .filter(|target| crate::workspace::validate_ssh_target(target).is_ok());
        if workspace.remote_target.is_none() {
            workspace.terminal_transport = Default::default();
            workspace.terminal_profile = Default::default();
            workspace.terminal_tmux_session = None;
        }
        let remote_bridge = ws.remote_target.as_ref().map(|_| {
            let bridge = std::sync::Arc::new(crate::ssh::bridge::SshBridge::new());
            *bridge.directory.lock().unwrap() = ws.remote_directory.clone();
            workspace.connection_state = ConnectionState::Reconnecting(0);
            bridge
        });
        let remote_launch = remote_bridge.as_ref().and_then(|bridge| {
            (workspace.terminal_transport == crate::remote_transport::TerminalTransport::Ssh).then(
                || crate::ghostty::surface::SurfaceIoMode::Remote {
                    bridge: bridge.clone(),
                    ssh_tx: self.ssh_event_tx.clone().unwrap(),
                    initial_input: None,
                },
            )
        });
        let remote_mosh = (workspace.terminal_transport
            == crate::remote_transport::TerminalTransport::Mosh)
            .then(|| crate::remote_transport::MoshLaunch {
                target: workspace
                    .remote_target
                    .clone()
                    .expect("validated Mosh target"),
                directory: workspace.remote_directory.clone(),
                profile: workspace.terminal_profile,
                tmux_session: workspace.terminal_tmux_session.clone(),
            });
        let launch_command = remote_mosh
            .is_none()
            .then(|| {
                ws.startup_script
                    .as_deref()
                    .map(crate::workspace::startup_command)
            })
            .flatten();

        let row = self.build_sidebar_row(&workspace);

        // Build split tree from session data (D-05)
        let engine = crate::split_engine::SplitEngine::from_data_with_command(
            self.ghostty_app,
            &ws.layout,
            ws.active_pane_uuid.as_deref(),
            ws.working_directory.clone(),
            launch_command,
            remote_launch,
            remote_mosh,
            &self.resume_policy,
            ws.launch_environment.clone(),
        )?;

        self.sidebar_list.append(&row);
        // Add to stack
        let page_name = format!("workspace-{}", id);
        self.stack
            .add_named(&engine.root_widget(), Some(&page_name));
        workspace.stack_page_name = page_name;

        self.workspaces.push(workspace);
        self.publish_workspace("workspace.created", self.workspaces.len() - 1, serde_json::json!({}));
        self.split_engines.push(engine);

        if let (Some(bridge), Some(target)) = (remote_bridge, ws.remote_target.clone()) {
            self.start_ssh(id, target, bridge, None, "restore");
        }
        Some(id)
    }

    /// Retain the workspace bridge and own its SSH task, linking retries to the initiating operation.
    /// Call on GTK after workspace creation; absent runtime/channel records unavailable without spawning.
    pub(crate) fn start_ssh(
        &mut self,
        id: u64,
        target: String,
        bridge: std::sync::Arc<crate::ssh::bridge::SshBridge>,
        parent: Option<uuid::Uuid>,
        origin: &'static str,
    ) {
        let trace_id = parent.unwrap_or_else(uuid::Uuid::new_v4);
        self.workspace_bridges.insert(id, bridge.clone());
        let ready = self.runtime_handle.is_some() && self.ssh_event_tx.is_some();
        crate::diagnostics::record(
            "workspace.ssh.launch",
            serde_json::json!({"trace_id": trace_id, "workspace_id": id, "origin": origin,
                "outcome": if ready { "scheduled" } else { "unavailable" }}),
        );
        if let (Some(rt), Some(tx)) = (self.runtime_handle.as_ref(), self.ssh_event_tx.clone()) {
            let handle = rt.spawn(crate::ssh::tunnel::run_ssh_lifecycle(
                id, target, tx, bridge, trace_id,
            ));
            if let Some(previous) = self.ssh_task_handles.insert(id, handle) {
                previous.abort();
            }
        }
    }

    /// Build an unattached GTK sidebar row with workspace identity, styling and controls.
    /// Local, remote and restored workspaces share this construction path.
    pub(crate) fn build_sidebar_row(&self, workspace: &Workspace) -> gtk4::ListBoxRow {
        let row = gtk4::ListBoxRow::new();
        row.set_child(Some(&crate::sidebar::workspace_row_content(workspace)));
        self.apply_row_status(&row, workspace);
        crate::sidebar::style_workspace_row(&row, workspace);
        crate::sidebar::bind_workspace_row(&row, workspace.id);
        let workspace_uuid = workspace.uuid;
        let drop_target = gtk4::DropTarget::new(String::static_type(), gtk4::gdk::DragAction::MOVE);
        drop_target.connect_drop(move |target, value, _, _| {
            let Ok(surface) = value.get::<String>() else {
                return false;
            };
            let Some(row) = target.widget().and_downcast::<gtk4::ListBoxRow>() else {
                return false;
            };
            row.activate_action(
                "win.surface-workspace-drop",
                Some(&format!("{surface}|{workspace_uuid}").to_variant()),
            )
            .is_ok()
        });
        row.add_controller(drop_target);
        row
    }

    /// Create a remote workspace whose interactive PTY may use Mosh while SSH owns management.
    pub fn create_remote_workspace_with_transport(
        &mut self,
        target: String,
        bridge: &std::sync::Arc<crate::ssh::bridge::SshBridge>,
        remote_directory: Option<String>,
        transport: crate::remote_transport::TerminalTransport,
        profile: crate::remote_transport::TerminalProfile,
        tmux_session: Option<String>,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let display_number = self.next_display_number;
        self.next_display_number += 1;

        let mut workspace = Workspace::new_remote(id, display_number, target);
        workspace.remote_directory = remote_directory;
        workspace.terminal_transport = transport;
        workspace.terminal_profile = profile;
        workspace.terminal_tmux_session = tmux_session;
        let row = self.build_sidebar_row(&workspace);
        self.sidebar_list.append(&row);

        // Create remote surface with manual I/O mode
        let pane_id = id * 1000;
        let mosh_command =
            (transport == crate::remote_transport::TerminalTransport::Mosh).then(|| {
                crate::remote_transport::mosh_command(
                    workspace.remote_target.as_deref().unwrap_or_default(),
                    workspace.remote_directory.as_deref(),
                    &workspace.terminal_profile,
                    workspace.terminal_tmux_session.as_deref(),
                )
                .expect("validated remote transport")
            });
        let io_mode = if let Some(command) = mosh_command.clone() {
            crate::ghostty::surface::SurfaceIoMode::Configured {
                command: Some(command),
                initial_input: None,
                environment: Default::default(),
            }
        } else {
            crate::ghostty::surface::SurfaceIoMode::Remote {
                bridge: bridge.clone(),
                ssh_tx: self
                    .ssh_event_tx
                    .clone()
                    .expect("SSH event channel initialized"),
                initial_input: None,
            }
        };
        let (gl_area, _) = crate::ghostty::surface::create_surface(
            self.ghostty_app,
            None,
            None,
            pane_id,
            io_mode.clone(),
        );
        let mut engine = SplitEngine::new(self.ghostty_app, gl_area, pane_id, None);
        if transport == crate::remote_transport::TerminalTransport::Ssh {
            engine.remote_launch = Some(io_mode);
        } else {
            engine.launch_command = mosh_command;
        }
        let page_name = workspace.stack_page_name.clone();
        self.stack
            .add_named(&engine.root_widget(), Some(&page_name));

        self.workspaces.push(workspace);
        self.publish_workspace("workspace.created", self.workspaces.len() - 1, serde_json::json!({}));
        self.split_engines.push(engine);

        crate::diagnostics::record(
            "workspace.remote.terminal",
            serde_json::json!({"workspace_id":id,"transport":transport,
                "profile":profile,"outcome":"created"}),
        );

        let new_index = self.workspaces.len() - 1;
        self.switch_to_index(new_index);
        self.trigger_session_save();
        id
    }

    /// Update the connection state of a workspace and refresh its sidebar row.
    pub fn update_connection_state(&mut self, workspace_id: u64, state: ConnectionState) {
        if let Some(idx) = self.workspaces.iter().position(|ws| ws.id == workspace_id) {
            self.workspaces[idx].connection_state = state.clone();
            if state != ConnectionState::Connected {
                crate::ports::publish(self, idx, None);
            }
            // Update sidebar subtitle
            if let Some(row) = crate::sidebar::row_for_workspace(&self.sidebar_list, workspace_id) {
                if let Some(vbox) = crate::sidebar::row_text(&row) {
                    // Last child in vbox is the status label (if it has connection-state class)
                    if let Some(status) = vbox.last_child().and_downcast::<gtk4::Label>() {
                        if status.has_css_class("connection-state") {
                            status.set_text(state.display_text());
                            status.remove_css_class("connected");
                            status.remove_css_class("disconnected");
                            status.remove_css_class("reconnecting");
                            status.add_css_class(state.css_class());
                        }
                    }
                }
            }
        }
    }

    /// Close the workspace at `index`. Removes the sidebar row and GtkStack page.
    /// Returns false if there is only one workspace (cannot close the last one).
    pub fn close_workspace(&mut self, index: usize) -> bool {
        if self.workspaces.len() <= 1 || index >= self.workspaces.len() {
            return false; // Cannot close the last workspace or an unknown index
        }

        // Abort SSH lifecycle task if this is a remote workspace.
        if let Some(ws) = self.workspaces.get(index) {
            if let Some(handle) = self.ssh_task_handles.remove(&ws.id) {
                handle.abort();
            }
        }

        // Retire browser owners before widget removal; GTK destruction can be delayed by retained widgets.
        let browsers: Vec<_> = self.split_engines[index]
            .browser_tabs()
            .into_iter()
            .map(|widgets| widgets.uuid)
            .collect();
        for id in browsers {
            self.shutdown_browser_surface(id);
        }

        // Stop PTYs and unregister their callbacks before removing GTK widgets.
        if let Some(engine) = self.split_engines.get(index) {
            let mut terminal_areas = Vec::new();
            engine.root.collect_terminal_areas(&mut terminal_areas);
            for area in terminal_areas {
                crate::split_engine::destroy_terminal_area(&area);
            }
        }
        self.split_engines.remove(index);

        let workspace = self.workspaces.remove(index);
        self.workspace_bridges.remove(&workspace.id);
        crate::events::publish(
            "workspace.closed",
            "workspace.lifecycle",
            crate::events::Scope {
                workspace: Some(workspace.uuid),
                ..Default::default()
            },
            serde_json::json!({"index": index}),
        );

        // Remove sidebar row.
        if let Some(row) = crate::sidebar::row_for_workspace(&self.sidebar_list, workspace.id) {
            self.sidebar_list.remove(&row);
        }

        // Remove GtkStack page.
        if let Some(child) = self.stack.child_by_name(&workspace.stack_page_name) {
            self.stack.remove(&child);
        }

        // At least one workspace survives the guard above.
        let was_active = index == self.active_index;
        self.active_index =
            crate::selection::after_removal(self.active_index, index, self.workspaces.len())
                .expect("workspace close preserves a survivor");

        self.switch_to_index(self.active_index);
        // switch_to_index cannot see the removed workspace as the previous selection.
        if was_active {
            self.publish_workspace(
                "workspace.selected",
                self.active_index,
                serde_json::json!({"previous_workspace_id": workspace.uuid}),
            );
        }
        self.trigger_session_save();
        true
    }

    /// Switch to the workspace at `index` (0-based). Updates GtkStack visible child and
    /// sidebar selection. Does nothing if index is out of bounds.
    pub fn switch_to_index(&mut self, index: usize) {
        if index >= self.workspaces.len() {
            return;
        }
        // Phase 4: clear attention when user switches to a workspace (D-05).
        self.clear_workspace_attention(index);
        let previous = self.workspaces.get(self.active_index).map(|workspace| workspace.uuid);
        // Upstream: switching to a workspace marks its own (terminal-less) notifications read.
        let workspace = self.workspaces[index].uuid;
        crate::inbox_actions::mark_read_where(self, workspace, None);
        self.active_index = index;
        if previous != Some(self.workspaces[index].uuid) {
            let tabs = self.split_engines.get(index).map_or(0, |engine| engine.all_panes().len());
            self.publish_workspace(
                "workspace.selected",
                index,
                serde_json::json!({"previous_workspace_id": previous, "tab_count": tabs}),
            );
        }
        let page_name = self.workspaces[index].stack_page_name.clone();
        self.stack.set_visible_child_name(&page_name);
        if let Some(row) =
            crate::sidebar::row_for_workspace(&self.sidebar_list, self.workspaces[index].id)
        {
            self.sidebar_list.select_row(Some(&row));
            // Update CSS classes: active row gets "active-workspace" for styling.
            // All rows: remove first, then add to active.
            for r in crate::sidebar::workspace_rows(&self.sidebar_list) {
                r.remove_css_class("active-workspace");
                if let Some(label) = crate::sidebar::row_title(&r) {
                    label.set_css_classes(&[]);
                }
            }
            row.add_css_class("active-workspace");
            if let Some(label) = crate::sidebar::row_title(&row) {
                label.add_css_class("active-workspace-label");
            }
        }
        // Restore focus through the selected surface for every workspace-switch caller.
        if let Some(engine) = self.split_engines.get(index) {
            engine.focus_active_surface();
        }
        // The Files tree follows the focused workspace (upstream root sync).
        self.refresh_right_sidebar();
    }

    /// Move a workspace and its engine together while retaining active identity and focus.
    pub fn reorder_workspace(&mut self, from: usize, to: usize) -> bool {
        let changed = self.move_workspace_row(from, to);
        if changed {
            self.trigger_session_save();
        }
        changed
    }

    /// Find the previous or next model index within the workspace's visible group scope.
    pub fn adjacent_workspace_in_group(&self, index: usize, offset: isize) -> Option<usize> {
        let group_id = self.workspaces.get(index)?.group_id;
        let peers: Vec<_> = self
            .workspaces
            .iter()
            .enumerate()
            .filter(|(_, workspace)| workspace.group_id == group_id)
            .map(|(index, _)| index)
            .collect();
        let peer = peers.iter().position(|candidate| *candidate == index)?;
        peer.checked_add_signed(offset)
            .and_then(|destination| peers.get(destination).copied())
    }

    /// Move the workspace, engine and existing GTK row together without publishing an intermediate snapshot.
    fn move_workspace_row(&mut self, from: usize, to: usize) -> bool {
        if from >= self.workspaces.len() || to >= self.workspaces.len() || from == to {
            return false;
        }
        let active_id = self.workspaces[self.active_index].id;
        let moved_id = self.workspaces[from].id;
        let workspace = self.workspaces.remove(from);
        self.workspaces.insert(to, workspace);
        let engine = self.split_engines.remove(from);
        self.split_engines.insert(to, engine);
        if self.workspace_groups.is_empty() {
            if let Some(row) = crate::sidebar::row_for_workspace(&self.sidebar_list, moved_id) {
                self.sidebar_list.remove(&row);
                self.sidebar_list.insert(&row, to as i32);
            }
        }
        self.active_index = self
            .workspaces
            .iter()
            .position(|w| w.id == active_id)
            .unwrap();
        if self.workspace_groups.is_empty() {
            let active_row = crate::sidebar::row_for_workspace(&self.sidebar_list, active_id);
            self.sidebar_list.select_row(active_row.as_ref());
        }
        true
    }

    /// Validate a batch before mutation, preserve unspecified order and active identity, and save only once.
    pub fn reorder_workspaces(
        &mut self,
        order: &[uuid::Uuid],
        dry_run: bool,
    ) -> Result<serde_json::Value, &'static str> {
        let mut seen = std::collections::HashSet::new();
        for id in order {
            if !seen.insert(*id) {
                return Err("duplicate workspace");
            }
            if !self
                .workspaces
                .iter()
                .any(|workspace| workspace.uuid == *id)
            {
                return Err("workspace not found");
            }
        }
        let final_order: Vec<_> = order
            .iter()
            .copied()
            .chain(
                self.workspaces
                    .iter()
                    .map(|workspace| workspace.uuid)
                    .filter(|id| !seen.contains(id)),
            )
            .collect();
        let plan: Vec<_> = final_order
            .iter()
            .enumerate()
            .map(|(to, id)| {
                let from = self
                    .workspaces
                    .iter()
                    .position(|workspace| workspace.uuid == *id)
                    .unwrap();
                serde_json::json!({"workspace_id":id,"from_index":from,"to_index":to})
            })
            .collect();
        let mut changed = false;
        if !dry_run {
            for (to, id) in final_order.iter().enumerate() {
                let from = self
                    .workspaces
                    .iter()
                    .position(|workspace| workspace.uuid == *id)
                    .unwrap();
                changed |= self.move_workspace_row(from, to);
            }
            if changed {
                self.trigger_session_save();
            }
        }
        let events: Vec<_> = plan
            .iter()
            .filter(|item| !dry_run && item["from_index"] != item["to_index"])
            .cloned()
            .collect();
        Ok(serde_json::json!({"dry_run":dry_run,"plan":plan,"events":events}))
    }

    /// Apply a validated RGB color to model and sidebar, then schedule a session save.
    /// Invalid colors and unknown workspace IDs leave state unchanged.
    pub fn set_workspace_color(&mut self, id: u64, color: Option<String>) {
        if color
            .as_deref()
            .is_some_and(|c| !crate::workspace::valid_workspace_color(c))
        {
            return;
        }
        if let Some(index) = self.workspaces.iter().position(|w| w.id == id) {
            self.workspaces[index].color = color;
            if let Some(row) = crate::sidebar::row_for_workspace(&self.sidebar_list, id) {
                crate::sidebar::style_workspace_row(&row, &self.workspaces[index]);
            }
            self.trigger_session_save();
        }
    }

    /// Create an ordered workspace group with bounded, CSS-safe presentation data.
    pub fn create_workspace_group(
        &mut self,
        name: String,
        color: Option<String>,
    ) -> Result<uuid::Uuid, &'static str> {
        if self.workspace_groups.len() >= crate::workspace_group::MAX_GROUPS {
            return Err("workspace group capacity reached");
        }
        let group = crate::workspace_group::WorkspaceGroup::new(name, color)?;
        let id = group.id;
        self.workspace_groups.push(group);
        self.trigger_session_save();
        crate::diagnostics::record(
            "workspace.group.change",
            serde_json::json!({"operation":"create","group_id":id,"outcome":"success"}),
        );
        Ok(id)
    }

    /// Update supplied group fields; omitted values retain their current state.
    pub fn update_workspace_group(
        &mut self,
        id: uuid::Uuid,
        name: Option<String>,
        color: Option<Option<String>>,
        collapsed: Option<bool>,
        position: Option<usize>,
    ) -> Result<(), &'static str> {
        if let Some(name) = name.as_deref() {
            crate::workspace_group::validate_name(name)?;
        }
        if let Some(color) = color.as_ref() {
            crate::workspace_group::validate_color(color.as_deref())?;
        }
        let Some(index) = self
            .workspace_groups
            .iter()
            .position(|group| group.id == id)
        else {
            return Err("workspace group not found");
        };
        {
            let group = &mut self.workspace_groups[index];
            if let Some(name) = name {
                group.name = name;
            }
            if let Some(color) = color {
                group.color = color;
            }
            if let Some(collapsed) = collapsed {
                group.collapsed = collapsed;
            }
        }
        if let Some(position) = position {
            let destination = position.min(self.workspace_groups.len().saturating_sub(1));
            if destination != index {
                let group = self.workspace_groups.remove(index);
                self.workspace_groups.insert(destination, group);
            }
        }
        self.trigger_session_save();
        crate::diagnostics::record(
            "workspace.group.change",
            serde_json::json!({"operation":"update","group_id":id,"outcome":"success"}),
        );
        Ok(())
    }

    /// Assign an explicit bounded set of workspaces atomically, or remove their membership.
    pub fn assign_workspace_group(
        &mut self,
        group_id: Option<uuid::Uuid>,
        workspace_ids: &[uuid::Uuid],
    ) -> Result<usize, &'static str> {
        if workspace_ids.is_empty() || workspace_ids.len() > 4096 {
            return Err("workspace_ids must contain 1..4096 UUIDs");
        }
        if group_id.is_some_and(|id| !self.workspace_groups.iter().any(|group| group.id == id)) {
            return Err("workspace group not found");
        }
        if workspace_ids.iter().any(|id| {
            !self
                .workspaces
                .iter()
                .any(|workspace| workspace.uuid == *id)
        }) {
            return Err("workspace not found");
        }
        let requested: std::collections::HashSet<_> = workspace_ids.iter().copied().collect();
        let mut changed = 0;
        for workspace in &mut self.workspaces {
            if requested.contains(&workspace.uuid) && workspace.group_id != group_id {
                workspace.group_id = group_id;
                changed += 1;
            }
        }
        if changed > 0 {
            self.trigger_session_save();
        }
        crate::diagnostics::record(
            "workspace.group.change",
            serde_json::json!({"operation":"assign","group_id":group_id,
                "workspace_count":workspace_ids.len(),"changed":changed,"outcome":"success"}),
        );
        Ok(changed)
    }

    /// Delete a group while retaining its workspaces and their stable identities.
    pub fn delete_workspace_group(&mut self, id: uuid::Uuid) -> Result<usize, &'static str> {
        let Some(index) = self
            .workspace_groups
            .iter()
            .position(|group| group.id == id)
        else {
            return Err("workspace group not found");
        };
        self.workspace_groups.remove(index);
        let mut changed = 0;
        for workspace in &mut self.workspaces {
            if workspace.group_id == Some(id) {
                workspace.group_id = None;
                changed += 1;
            }
        }
        self.trigger_session_save();
        crate::diagnostics::record(
            "workspace.group.change",
            serde_json::json!({"operation":"delete","group_id":id,"workspace_count":changed,
                "outcome":"success"}),
        );
        Ok(changed)
    }

    /// Switch to next workspace (wrap-around). Per D-10: Ctrl+].
    pub fn switch_next(&mut self) {
        if self.workspaces.is_empty() {
            return;
        }
        let next = (self.active_index + 1) % self.workspaces.len();
        self.switch_to_index(next);
    }

    /// Switch to previous workspace (wrap-around). Per D-10: Ctrl+[.
    pub fn switch_prev(&mut self) {
        if self.workspaces.is_empty() {
            return;
        }
        let prev = if self.active_index == 0 {
            self.workspaces.len() - 1
        } else {
            self.active_index - 1
        };
        self.switch_to_index(prev);
    }

    /// Borrow the selected workspace's pane tree, or None when no workspace exists.
    pub fn active_split_engine(&self) -> Option<&SplitEngine> {
        self.split_engines.get(self.active_index)
    }

    /// Mutably borrow the selected pane tree for GTK-thread workspace operations.
    pub fn active_split_engine_mut(&mut self) -> Option<&mut SplitEngine> {
        self.split_engines.get_mut(self.active_index)
    }

    /// Rename the active workspace. Per D-03/D-10: Ctrl+Shift+R (UI wired in Plan 04/05).
    pub fn rename_active(&mut self, new_name: String) {
        self.rename_workspace_at(self.active_index, new_name);
    }

    /// Update the workspace name and sidebar label, then schedule persistence; ignore invalid indices.
    pub fn rename_workspace_at(&mut self, index: usize, new_name: String) {
        if let Some(ws) = self.workspaces.get_mut(index) {
            ws.rename(new_name);
            self.update_sidebar_name(index);
            self.trigger_session_save();
            self.publish_workspace("workspace.renamed", index, serde_json::json!({}));
        }
    }

    /// Set or clear a workspace description and show it in its sidebar row (upstream
    /// `set-description` / `clear-description`: an empty value clears it).
    pub fn set_workspace_description(&mut self, index: usize, description: Option<String>) {
        if let Some(workspace) = self.workspaces.get_mut(index) {
            workspace.describe(description);
        } else {
            return;
        }
        self.trigger_session_save();
        self.publish_workspace("workspace.described", index, serde_json::json!({}));
        self.update_sidebar_description(index);
    }

    /// Show the workspace's description in its sidebar row.
    fn update_sidebar_description(&self, index: usize) {
        let Some(workspace) = self.workspaces.get(index) else {
            return;
        };
        if let Some(row) = crate::sidebar::row_for_workspace(&self.sidebar_list, workspace.id) {
            crate::sidebar::set_row_description(&row, workspace.custom_description.as_deref());
        }
    }

    /// Show the workspace's current name in its sidebar row.
    fn update_sidebar_name(&self, index: usize) {
        let Some(workspace) = self.workspaces.get(index) else {
            return;
        };
        if let Some(row) = crate::sidebar::row_for_workspace(&self.sidebar_list, workspace.id) {
            if let Some(label) = crate::sidebar::row_title(&row) {
                label.set_text(&workspace.name);
            }
        }
    }

    /// Publish a workspace lifecycle event carrying the workspace's identity, title and index.
    fn publish_workspace(&self, name: &str, index: usize, mut payload: serde_json::Value) {
        let Some(workspace) = self.workspaces.get(index) else {
            return;
        };
        payload["index"] = serde_json::json!(index);
        payload["title"] = serde_json::json!(workspace.name);
        crate::events::publish(
            name,
            "workspace.lifecycle",
            crate::events::Scope {
                workspace: Some(workspace.uuid),
                ..Default::default()
            },
            payload,
        );
    }

    /// A terminal's program set its title: show it on the tab and, as upstream, let it
    /// name the workspace when it is the focused tab.
    pub fn surface_title_changed(&mut self, surface: uuid::Uuid, title: &str) {
        let id = surface.to_string();
        let Some(index) = self
            .split_engines
            .iter()
            .position(|engine| engine.set_surface_title(&id, title))
        else {
            return;
        };
        if self.split_engines[index].active_pane_uuid().as_deref() == Some(id.as_str()) {
            self.apply_focused_title(index);
        }
    }

    /// Upstream `applyFocusedPanelTitle`: a workspace without a user-chosen name shows the
    /// title of its focused tab (ignoring the placeholder of a tab that never set one).
    pub fn apply_focused_title(&mut self, index: usize) {
        let Some(title) = self.split_engines.get(index).and_then(|engine| {
            engine
                .active_pane_uuid()
                .and_then(|id| engine.surface_title(&id))
        }) else {
            return;
        };
        let Some(workspace) = self.workspaces.get_mut(index) else {
            return;
        };
        if workspace.custom_name || title == "Terminal" || workspace.name == title {
            return;
        }
        workspace.name = title;
        self.update_sidebar_name(index);
        self.trigger_session_save();
        self.publish_workspace("workspace.renamed", index, serde_json::json!({"automatic": true}));
    }

    /// Returns the active workspace, if any.
    pub fn active_workspace(&self) -> Option<&Workspace> {
        self.workspaces.get(self.active_index)
    }

    /// Set attention on a specific pane. Called from bell handler.
    /// Updates workspace has_attention and sidebar dot.
    pub fn set_pane_attention(&mut self, pane_id: u64) {
        for (idx, engine) in self.split_engines.iter_mut().enumerate() {
            if engine.root.set_attention(pane_id, true) {
                self.workspaces[idx].has_attention = engine.root.any_attention();
                self.update_sidebar_attention(idx);

                // Desktop notification when window is unfocused (NOTF-03)
                let window_focused = self
                    .gtk_app
                    .active_window()
                    .map(|w| w.is_active())
                    .unwrap_or(false);
                if !window_focused && self.workspaces[idx].has_attention {
                    let should_notify = self.workspaces[idx]
                        .last_notification
                        .map(|t| t.elapsed() >= std::time::Duration::from_secs(5))
                        .unwrap_or(true);
                    if should_notify {
                        self.workspaces[idx].last_notification = Some(std::time::Instant::now());
                        if let Some(runtime) = &self.runtime_handle {
                            crate::notification::send(
                                runtime,
                                cmux_platform::notification::terminal_bell(
                                    &self.workspaces[idx].name,
                                ),
                                self.workspaces[idx].uuid,
                            );
                        }
                    }
                }
                break;
            }
        }
    }

    /// Clear all attention in the workspace at `index`.
    pub fn clear_workspace_attention(&mut self, index: usize) {
        if let Some(engine) = self.split_engines.get_mut(index) {
            engine.root.clear_all_attention();
        }
        if let Some(ws) = self.workspaces.get_mut(index) {
            ws.has_attention = false;
        }
        self.update_sidebar_attention(index);
    }

    /// Unread count and latest message for one workspace, from the inbox (upstream row model).
    fn inbox_row_summary(
        records: &[crate::inbox::Record],
        workspace_uuid: uuid::Uuid,
    ) -> (usize, Option<String>) {
        let mut records = records
            .iter()
            .filter(|record| record.workspace_id == workspace_uuid);
        let unread = records.clone().filter(|record| !record.is_read).count();
        // Upstream: the latest message stays, read or not; body first, else title.
        let latest = records.next_back().map(|record| {
            let content = &record.content;
            if content.body.trim().is_empty() {
                content.title.clone()
            } else {
                content.body.clone()
            }
        });
        (unread, latest)
    }

    /// Fill the status slot and message line of a row just built. A built row has no engine yet,
    /// so a running agent is read from the workspace itself; hook events prune it afterwards.
    pub(crate) fn apply_row_status(&self, row: &gtk4::ListBoxRow, workspace: &Workspace) {
        let (unread, latest) = Self::inbox_row_summary(&self.inbox.records, workspace.uuid);
        crate::sidebar::set_row_status(
            row,
            unread,
            workspace.has_attention || unread > 0,
            !workspace.agents_running.is_empty(),
            latest.as_deref(),
        );
    }

    /// Update the sidebar dot visibility for workspace at `index`.
    pub(crate) fn update_sidebar_attention(&self, index: usize) {
        if let Some(row) =
            crate::sidebar::row_for_workspace(&self.sidebar_list, self.workspaces[index].id)
        {
            let workspace = &self.workspaces[index];
            let (unread, latest) = Self::inbox_row_summary(&self.inbox.records, workspace.uuid);
            // A turn counts while its terminal is still open; closed ones were pruned on hook.
            let running = workspace.agents_running.iter().any(|(surface, _)| {
                self.split_engines[index]
                    .find_pane_id_by_uuid(surface)
                    .is_some()
            });
            crate::sidebar::set_row_status(
                &row,
                unread,
                workspace.has_attention || unread > 0,
                running,
                latest.as_deref(),
            );
        }
        crate::sidebar::update_group_attention(self);
    }

    /// Where focus is now: the active workspace and its selected terminal.
    fn current_focus_entry(&self) -> Option<crate::focus_history::Entry> {
        Some(crate::focus_history::Entry {
            workspace: self.workspaces.get(self.active_index)?.uuid,
            surface: self
                .split_engines
                .get(self.active_index)?
                .active_pane_uuid()
                .and_then(|uuid| uuid::Uuid::parse_str(&uuid).ok()),
        })
    }

    /// Where a history position lands today: gone with its workspace; a closed terminal falls
    /// back to the workspace's selected one (upstream `resolvedFocusHistoryEntry`).
    fn resolve_focus_entry(
        &self,
        entry: crate::focus_history::Entry,
    ) -> Option<crate::focus_history::Entry> {
        let index = self
            .workspaces
            .iter()
            .position(|workspace| workspace.uuid == entry.workspace)?;
        let engine = self.split_engines.get(index)?;
        let surface = entry
            .surface
            .filter(|surface| engine.find_pane_id_by_uuid(&surface.to_string()).is_some())
            .or_else(|| {
                engine
                    .active_pane_uuid()
                    .and_then(|uuid| uuid::Uuid::parse_str(&uuid).ok())
            });
        Some(crate::focus_history::Entry {
            workspace: entry.workspace,
            surface,
        })
    }

    /// Record the terminal that just took focus in the workspace at `index`.
    pub fn record_focus(&mut self, index: usize, surface: uuid::Uuid) {
        let Some(workspace) = self.workspaces.get(index) else {
            return;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs() as i64);
        self.focus_history.record(
            crate::focus_history::Entry {
                workspace: workspace.uuid,
                surface: Some(surface),
            },
            now,
        );
        self.refresh_focus_history_actions();
    }

    /// Reachable history positions in `direction`, nearest first.
    pub fn focus_history_items(
        &self,
        direction: crate::focus_history::Direction,
    ) -> Vec<crate::focus_history::Item> {
        self.focus_history.items(direction, self.current_focus_entry(), |entry| {
            self.resolve_focus_entry(entry)
        })
    }

    /// Menu label of a history position (upstream `FocusHistoryMenuFormatter`): workspace and
    /// tab title, then direction and time.
    pub fn focus_history_label(
        &self,
        item: &crate::focus_history::Item,
        direction: crate::focus_history::Direction,
    ) -> String {
        let index = self
            .workspaces
            .iter()
            .position(|workspace| workspace.uuid == item.entry.workspace);
        let workspace = index
            .map(|index| self.workspaces[index].name.trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Untitled Workspace".into());
        let tab = index
            .zip(item.entry.surface)
            .and_then(|(index, surface)| {
                self.split_engines[index].surface_title(&surface.to_string())
            })
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty() && *title != workspace);
        let title = match tab {
            Some(tab) => format!("{workspace} - {tab}"),
            None => workspace,
        };
        let focused = gtk4::glib::DateTime::from_unix_local(item.focused_at)
            .and_then(|time| time.format("%H:%M"))
            .map(|time| time.to_string())
            .unwrap_or_default();
        let direction = match direction {
            crate::focus_history::Direction::Back => "Focus Back",
            crate::focus_history::Direction::Forward => "Focus Forward",
        };
        format!("{title}\n{direction}, Focused {focused}")
    }

    /// Go to a history position without recording the moves it causes; the caller lifts the
    /// suppression once GTK has delivered the resulting focus events.
    pub fn navigate_focus_history(&mut self, item: &crate::focus_history::Item) -> bool {
        let Some(index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.uuid == item.entry.workspace)
        else {
            return false;
        };
        self.focus_history.suppressed = true;
        self.switch_to_index(index);
        if let Some(surface) = item.entry.surface {
            self.split_engines[index].focus_surface(&surface.to_string());
        }
        self.focus_history.go(item.index);
        self.refresh_focus_history_actions();
        true
    }

    /// Enable each titlebar arrow only when it leads somewhere (upstream `canNavigateBack`).
    pub fn refresh_focus_history_actions(&self) {
        if let Some((back, forward)) = &self.focus_history_actions {
            back.set_enabled(!self.focus_history_items(crate::focus_history::Direction::Back).is_empty());
            forward.set_enabled(
                !self.focus_history_items(crate::focus_history::Direction::Forward).is_empty(),
            );
        }
    }

    /// Agent hook turn boundaries drive the sidebar spinner (upstream `showAgentActivity`).
    pub fn set_agent_running(&mut self, surface: &str, source: &str, running: bool) {
        let Some(index) = self
            .split_engines
            .iter()
            .position(|engine| engine.find_pane_id_by_uuid(surface).is_some())
        else {
            return;
        };
        let engine = &self.split_engines[index];
        let agents = &mut self.workspaces[index].agents_running;
        // Forget terminals that closed without reporting the end of their turn.
        agents.retain(|(surface, _)| engine.find_pane_id_by_uuid(surface).is_some());
        let key = (surface.to_owned(), source.to_owned());
        let changed = if running {
            agents.insert(key)
        } else {
            agents.remove(&key)
        };
        if changed {
            self.update_sidebar_attention(index);
        }
    }

    /// Remove the manager and cancel its local work now; close its daemon on Tokio without GTK I/O.
    pub fn shutdown_browser(&mut self) {
        let sessions = std::mem::take(&mut self.browser_sessions);
        for (_, browser) in sessions {
            self.retire_browser_session(browser);
        }
        if let Some(browser) = self.browser_manager.take() {
            self.retire_browser_session(browser);
        }
        self.browser_surface_refs.clear();
    }

    /// Transfer one session's shutdown future to the shared bounded post-GTK drain.
    /// Local input, navigation and frame workers stop synchronously before the asynchronous close.
    fn retire_browser_session(&mut self, browser: crate::browser::BrowserManager) {
        let close = browser.shutdown();
        if let Some(runtime) = self.runtime_handle.as_ref() {
            let mut tasks = self.browser_shutdown_tasks.borrow_mut();
            // Reap completed closes during normal use so retained handles do not accumulate.
            while tasks.try_join_next().is_some() {}
            tasks.spawn_on(close, runtime);
        } else {
            crate::diagnostics::record(
                "browser.shutdown.runtime_unavailable",
                serde_json::json!({}),
            );
        }
    }

    /// Close an existing browser tab before retiring its daemon; a rejected final-pane close keeps it usable.
    /// GTK-only, with the same final-workspace policy as ordinary surface closure.
    pub fn close_browser_surface(
        &mut self,
        id: uuid::Uuid,
    ) -> crate::split_engine::CloseSurfaceResult {
        use crate::split_engine::CloseSurfaceResult;
        let Some(index) = self.split_engines.iter().position(|engine| {
            engine
                .browser_tabs()
                .iter()
                .any(|widgets| widgets.uuid == id)
        }) else {
            return CloseSurfaceResult::NotFound;
        };
        let result = self.split_engines[index].close_surface_and_empty_pane(id);
        if matches!(result, CloseSurfaceResult::Closed) {
            self.shutdown_browser_surface(id);
            self.trigger_session_save();
        }
        result
    }

    /// Retire exactly one browser surface without disturbing sibling sessions or focus.
    pub fn shutdown_browser_surface(&mut self, id: uuid::Uuid) {
        self.browser_surface_refs
            .retain(|_, value| value != &id.to_string());
        if let Some(browser) = self.browser_sessions.remove(&id) {
            self.retire_browser_session(browser);
        }
    }

    /// Retire a live browser daemon when a surface crosses into a different network route.
    /// Keep its stable surface/short-ref identity; the next command lazily starts the daemon
    /// against the destination workspace using the retained URL and profile.
    fn rebind_browser_route(&mut self, id: uuid::Uuid, workspace: uuid::Uuid) -> bool {
        let route_changed = self
            .browser_sessions
            .get(&id)
            .is_some_and(|browser| !browser.route_matches_workspace(self, workspace));
        if route_changed {
            if let Some(browser) = self.browser_sessions.remove(&id) {
                self.retire_browser_session(browser);
            }
            crate::diagnostics::record(
                "browser.route_rebound",
                serde_json::json!({"surface_id": id, "workspace_id": workspace}),
            );
        }
        route_changed
    }

    /// Move a stable surface between workspace engines on GTK. Local emptied workspaces are
    /// removed; an emptied remote workspace is rejected because its SSH bridge currently owns
    /// the live terminal transport and cannot be retired underneath the moved widget.
    pub(crate) fn move_surface_between_workspaces(
        &mut self,
        id: uuid::Uuid,
        destination_workspace: uuid::Uuid,
        destination_pane: Option<u64>,
        position: Option<usize>,
        focus: bool,
    ) -> Result<(crate::split_engine::SurfaceMoveResult, bool), &'static str> {
        let id_text = id.to_string();
        let source_index = self
            .split_engines
            .iter()
            .position(|engine| engine.find_pane_id_by_uuid(&id_text).is_some())
            .ok_or("surface not found")?;
        let destination_index = self
            .workspaces
            .iter()
            .position(|workspace| workspace.uuid == destination_workspace)
            .ok_or("destination workspace not found")?;
        let destination_pane =
            destination_pane.unwrap_or(self.split_engines[destination_index].active_pane_id);
        if source_index == destination_index {
            let result = self.split_engines[source_index].move_surface(
                id,
                destination_pane,
                position,
                focus,
            )?;
            if focus {
                self.switch_to_index(source_index);
            }
            self.trigger_session_save();
            return Ok((result, false));
        }
        if !self.split_engines[destination_index].can_insert_surface(destination_pane, position) {
            return Err("destination pane or position invalid");
        }
        let source_surface_count = self.split_engines[source_index]
            .pane_info()
            .iter()
            .map(|pane| pane.surface_ids.len())
            .sum::<usize>();
        let source_will_empty = source_surface_count == 1;
        let source_is_browser = self.split_engines[source_index]
            .browser_tabs()
            .iter()
            .any(|widgets| widgets.uuid == id);
        if source_will_empty
            && self.workspaces[source_index].remote_target.is_some()
            && !source_is_browser
        {
            return Err("cannot empty a remote workspace while its transport owns the surface");
        }
        let source_workspace = self.workspaces[source_index].uuid;
        let detached = self.split_engines[source_index].detach_surface(id)?;
        let source_pane = detached.source_pane;
        let source_position = detached.position;

        if self.split_engines[source_index].is_empty() && !self.close_workspace(source_index) {
            return Err("could not remove empty source workspace");
        }
        let destination_index = self
            .workspaces
            .iter()
            .position(|workspace| workspace.uuid == destination_workspace)
            .expect("validated destination workspace survives source removal");
        let result = self.split_engines[destination_index].insert_detached_surface(
            detached,
            destination_pane,
            position,
            focus,
        );
        for record in &mut self.inbox.records {
            if record.surface_id == Some(id) {
                record.workspace_id = destination_workspace;
            }
        }
        let route_restarted = self.rebind_browser_route(id, destination_workspace);
        if focus {
            self.switch_to_index(destination_index);
        }
        self.trigger_session_save();
        crate::diagnostics::record(
            "surface.workspace_moved",
            serde_json::json!({
                "surface_id": id,
                "source_workspace": source_workspace,
                "destination_workspace": destination_workspace,
                "source_pane": source_pane,
                "source_position": source_position,
                "destination_pane": result.pane_id,
                "destination_position": result.position,
                "focused": focus,
                "browser_route_restarted": route_restarted,
            }),
        );
        Ok((result, route_restarted))
    }

    /// Cancel only an unfinished restored manager, retaining the existing surface reference for retries.
    pub fn cancel_browser_restore(&mut self, id: uuid::Uuid, session: &str) {
        if self.browser_sessions.get(&id).is_some_and(|browser| {
            browser.session_identity() == session
                && !matches!(
                    browser.preview_state,
                    crate::browser::PreviewState::Connected
                        | crate::browser::PreviewState::Streaming
                )
        }) {
            if let Some(browser) = self.browser_sessions.remove(&id) {
                self.retire_browser_session(browser);
            }
        }
    }

    /// Retire a failed provisional startup only if its session still owns the admission slot.
    pub fn cancel_browser_startup(&mut self, session: &str) {
        if self
            .browser_manager
            .as_ref()
            .is_some_and(|browser| browser.session_identity() == session)
        {
            if let Some(browser) = self.browser_manager.take() {
                self.retire_browser_session(browser);
            }
        }
    }

    /// Publish the final live layout before native teardown and prevent later callbacks overwriting it.
    /// Idempotent on GTK; the composition root separately waits for durable worker completion.
    pub fn finish_session(&mut self) {
        self.trigger_session_save();
        self.session_tx.take();
    }

    /// Trigger a debounced session save. Call after any workspace/pane mutation.
    /// Snapshots SessionData on the main thread (safe for Rc) and sends to the
    /// tokio debounce task which handles the file I/O. Records GTK construction
    /// and publication cost separately from the worker's serialization/write timing.
    pub fn trigger_session_save(&self) {
        // Snapshot on GTK; the worker shares ownership rather than cloning the tree.
        if let Some(ref tx) = self.session_tx {
            let started = std::time::Instant::now();
            let mut history_budget = crate::scrollback::SESSION_MAX_BYTES;
            let session = crate::session::SessionData {
                version: 3, // Per-pane terminal/browser tabs and persisted URLs
                active_index: self.active_index,
                resume_policy: self.resume_policy.clone(),
                inbox: self.inbox.clone(),
                workspace_groups: self.workspace_groups.clone(),
                workspaces: self
                    .workspaces
                    .iter()
                    .enumerate()
                    .map(|(i, ws)| {
                        // D-02: save full split tree for ALL workspaces
                        let layout = if i < self.split_engines.len() {
                            self.split_engines[i]
                                .root
                                .to_data_with_history(&mut history_budget)
                        } else {
                            // Fallback: shouldn't happen, but be safe
                            crate::split_engine::SplitNodeData::Leaf {
                                pane_id: 0,
                                surface_uuid: uuid::Uuid::nil(),
                                shell: String::new(),
                                cwd: String::new(),
                            }
                        };
                        // D-04: save active_pane_uuid per workspace
                        let active_pane_uuid = if i < self.split_engines.len() {
                            self.split_engines[i].active_pane_uuid()
                        } else {
                            None
                        };
                        crate::session::WorkspaceSession {
                            launch_environment: self
                                .split_engines
                                .get(i)
                                .map(|engine| engine.launch_environment.clone())
                                .unwrap_or_default(),
                            metadata: ws.metadata.clone(),
                            uuid: ws.uuid.to_string(),
                            name: ws.name.clone(),
                            custom_name: Some(ws.custom_name),
                            custom_description: ws.custom_description.clone(),
                            color: ws.color.clone(),
                            group_id: ws.group_id,
                            startup_script: ws.startup_script.clone(),
                            remote_target: ws.remote_target.clone(),
                            remote_directory: ws.remote_directory.clone(),
                            terminal_transport: ws.terminal_transport,
                            terminal_profile: ws.terminal_profile,
                            terminal_tmux_session: ws.terminal_tmux_session.clone(),
                            working_directory: ws.working_directory.clone(),
                            active_pane_uuid,
                            layout,
                        }
                    })
                    .collect(),
            };
            let construction_us = started.elapsed().as_micros() as u64;
            let published = tx.send(Some(std::sync::Arc::new(session))).is_ok();
            crate::diagnostics::record(
                "session.snapshot",
                serde_json::json!({
                    "outcome": if published { "published" } else { "worker_closed" },
                    "workspaces": self.workspaces.len(),
                    "workspace_groups": self.workspace_groups.len(),
                    "construction_us": construction_us,
                    "duration_us": started.elapsed().as_micros() as u64,
                }),
            );
        }
    }
}
