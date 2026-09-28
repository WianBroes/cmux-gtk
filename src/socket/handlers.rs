// src/socket/handlers.rs — GTK main thread command dispatch

use crate::socket::commands::SocketCommand;
use gtk4::prelude::*;
use serde_json::{json, Value};

use super::response::{err, ok};

/// Snapshot one workspace's identity and layout counts without changing focus or retaining widgets.
/// Return None for an absent index; missing engines produce unknown counts rather than fabricated zeroes.
fn workspace_record(state: &crate::app_state::AppState, index: usize) -> Option<Value> {
    let workspace = state.workspaces.get(index)?;
    let counts = state.split_engines.get(index).map(|engine| {
        let panes = engine.pane_info();
        let surfaces: usize = panes.iter().map(|pane| pane.surface_ids.len()).sum();
        (panes.len(), surfaces)
    });
    Some(json!({
        "index": index,
        "id": workspace.uuid,
        "ref": state.handles.ensure_ref(super::handles::HandleKind::Workspace, &workspace.uuid.to_string()),
        "uuid": workspace.uuid,
        "git": workspace.git,
        "ports": workspace.ports,
        "remote": workspace.remote_target.as_ref().map(|_| {
            let bridge = state.workspace_bridges.get(&workspace.id);
            let port = bridge.map(|bridge| bridge.browser_proxy_port.load(std::sync::atomic::Ordering::Acquire)).filter(|port| *port != 0);
            json!({
                "connection_state": workspace.connection_state.css_class(),
                "reconnect_attempt": match workspace.connection_state { crate::workspace::ConnectionState::Reconnecting(attempt) => Some(attempt), _ => None },
                "browser_proxy_port": port,
                "browser_proxy_ready": bridge.and_then(|bridge| bridge.browser_proxy_ready()).is_some(),
                "terminal_transport": workspace.terminal_transport,
                "terminal_profile": workspace.terminal_profile,
                "terminal_tmux_session": workspace.terminal_tmux_session,
            })
        }),
        "title": workspace.name,
        "name": workspace.name,
        "group_id": workspace.group_id,
        "working_directory": workspace.working_directory.as_ref().map(|path| path.to_string_lossy()),
        "selected": index == state.active_index,
        "pane_count": counts.map(|(panes, _)| panes),
        "surface_count": counts.map(|(_, surfaces)| surfaces),
    }))
}

/// The single main window, with its ref and index like upstream `window.list` items.
fn window_record(state: &crate::app_state::AppState) -> Value {
    use super::handles::{HandleKind, MAIN_WINDOW_ID};
    json!({
        "id": MAIN_WINDOW_ID,
        "ref": state.handles.ensure_ref(HandleKind::Window, MAIN_WINDOW_ID),
        "index": 0,
        "key": true,
        "workspaces": state.workspaces.len(),
        "workspace_count": state.workspaces.len(),
    })
}

/// Window, workspace, pane and surface of one location, ids beside refs (upstream identify payload).
/// None when `surface` is given but not in that workspace.
fn identify_location(
    state: &crate::app_state::AppState,
    index: usize,
    surface: Option<uuid::Uuid>,
) -> Option<Value> {
    use super::handles::{HandleKind, MAIN_WINDOW_ID};
    let workspace = state.workspaces.get(index)?.uuid.to_string();
    let engine = state.split_engines.get(index)?;
    let mut payload = json!({
        "window_id": MAIN_WINDOW_ID,
        "window_ref": state.handles.ensure_ref(HandleKind::Window, MAIN_WINDOW_ID),
        "workspace_id": workspace,
        "workspace_ref": state.handles.ensure_ref(HandleKind::Workspace, &workspace),
        "surface_id": null, "surface_ref": null, "surface_type": null,
        "is_browser_surface": null, "pane_id": null, "pane_ref": null,
    });
    if let Some(surface) = surface {
        let pane = engine
            .pane_info()
            .into_iter()
            .find(|pane| pane.surface_ids.contains(&surface))?;
        let browser = engine
            .browser_tabs()
            .iter()
            .any(|widgets| widgets.uuid == surface);
        let id = surface.to_string();
        let fields = payload.as_object_mut()?;
        fields.insert(
            "surface_ref".into(),
            json!(state.handles.ensure_ref(HandleKind::Surface, &id)),
        );
        fields.insert("surface_id".into(), json!(id));
        fields.insert(
            "surface_type".into(),
            json!(if browser { "browser" } else { "terminal" }),
        );
        fields.insert("is_browser_surface".into(), json!(browser));
        fields.insert("pane_id".into(), json!(format!("pane:{}", pane.id)));
        fields.insert("pane_ref".into(), json!(format!("pane:{}", pane.id)));
    }
    Some(payload)
}

/// Resolve a live terminal in the current workspace without focus changes; GTK-thread callers only.
fn terminal_target(
    state: &crate::app_state::AppStateRef,
    id: Option<&str>,
) -> Result<crate::ghostty::ffi::ghostty_surface_t, (&'static str, &'static str)> {
    let surface = {
        let state = state.borrow();
        match id {
            Some(id) => state
                .split_engines
                .iter()
                .find_map(|engine| engine.find_surface_by_uuid(id)),
            None => state
                .split_engines
                .get(state.active_index)
                .and_then(|engine| engine.root.find_surface_for_pane(engine.active_pane_id)),
        }
    }
    .filter(|surface| !surface.is_null())
    .ok_or(("not_found", "live terminal surface not found"))?;
    Ok(surface)
}

/// Send literal UTF-8 text to a live terminal in the active workspace without changing focus.
/// Resolve the optional UUID before native calls; reject missing targets and embedded NUL bytes.
fn send_terminal_text(
    state: &crate::app_state::AppStateRef,
    id: Option<&str>,
    text: &str,
) -> Result<(), (&'static str, &'static str)> {
    let surface = terminal_target(state, id)?;
    // SAFETY: target resolution found a live native terminal on GTK and released
    // the model borrow. No event-loop iteration or teardown occurs before delivery.
    unsafe { crate::ghostty::text::send_literal(surface, text) }
        .map_err(|message| ("invalid_params", message))
}

/// Resolve a surface_ref string ("surface:N" or UUID) to a UUID string.
/// Returns Ok(uuid_string) or Err((error_message, available_refs)).
fn resolve_surface_ref(
    surface_ref: &str,
    refs: &std::collections::HashMap<u32, String>,
) -> Result<String, (String, Vec<String>)> {
    if let Some(n_str) = surface_ref.strip_prefix("surface:") {
        if let Ok(n) = n_str.parse::<u32>() {
            if let Some(uuid) = refs.get(&n) {
                return Ok(uuid.clone());
            }
            let available: Vec<String> = refs.keys().map(|k| format!("surface:{}", k)).collect();
            return Err((format!("surface:{} not found", n), available));
        }
    }
    if let Ok(id) = uuid::Uuid::parse_str(surface_ref) {
        if refs
            .values()
            .any(|value| uuid::Uuid::parse_str(value).ok() == Some(id))
        {
            return Ok(id.to_string());
        }
    }
    Err((
        "browser surface not found".into(),
        refs.keys().map(|key| format!("surface:{key}")).collect(),
    ))
}

/// Resolve explicit browser identity or the selected browser; a sole session is an unambiguous fallback.
fn browser_target(state: &crate::app_state::AppState, target: Option<&str>) -> Option<uuid::Uuid> {
    let surfaces: Vec<_> = state
        .split_engines
        .iter()
        .flat_map(|engine| engine.browser_tabs())
        .map(|widgets| widgets.uuid)
        .collect();
    if let Some(target) = target {
        let value = resolve_surface_ref(target, &state.browser_surface_refs).ok()?;
        let id = uuid::Uuid::parse_str(&value).ok()?;
        return surfaces.contains(&id).then_some(id);
    }
    let selected = state
        .split_engines
        .get(state.active_index)
        .and_then(|engine| engine.active_pane_uuid())
        .and_then(|id| uuid::Uuid::parse_str(&id).ok());
    selected
        .filter(|id| surfaces.contains(id))
        .or_else(|| (surfaces.len() == 1).then(|| surfaces[0]))
}

/// Dispatch a SocketCommand on the GTK main thread.
/// SOCK-05: Only focus-intent commands (workspace.select, workspace.next/previous/last,
/// pane.focus, pane.last, surface.focus) may call grab_active_focus() or focus_active_surface().
#[allow(unused_variables)]
pub fn handle_socket_command(cmd: SocketCommand, state: &crate::app_state::AppStateRef) {
    handle_socket_command_traced(cmd, state, None);
}

/// Carry an observed request identity through dispatch into asynchronous service completion.
#[allow(unused_variables)]
fn handle_socket_command_traced(
    cmd: SocketCommand,
    state: &crate::app_state::AppStateRef,
    trace_id: Option<String>,
) {
    match cmd {
        SocketCommand::Observed {
            command,
            trace_id,
            queued_at,
        } => {
            let started = std::time::Instant::now();
            crate::diagnostics::record(
                "rpc.gtk.start",
                json!({
                    "trace_id": trace_id, "queue_wait_us": queued_at.elapsed().as_micros(),
                }),
            );
            handle_socket_command_traced(*command, state, Some(trace_id.to_string()));
            crate::diagnostics::record(
                "rpc.gtk.dispatched",
                json!({
                    "trace_id": trace_id, "duration_us": started.elapsed().as_micros(),
                }),
            );
        }

        SocketCommand::ResolveHandles {
            req_id,
            refs,
            resp_tx,
        } => {
            let s = state.borrow();
            let response = match super::handles::resolve_all(&s.handles, &s, &refs) {
                Ok(resolved) => ok(req_id, json!(resolved)),
                Err(message) => err(req_id, "not_found", &message),
            };
            let _ = resp_tx.send(response);
        }

        // -- system.* --
        SocketCommand::Ping { req_id, resp_tx } => {
            let _ = resp_tx.send(ok(req_id, json!({"pong": true})));
        }

        SocketCommand::Identify {
            req_id,
            caller,
            resp_tx,
        } => {
            let socket_path = crate::socket::socket_path().to_string_lossy().to_string();
            let s = state.borrow();
            let focused = s.split_engines.get(s.active_index).and_then(|engine| {
                let surface = engine
                    .pane_info()
                    .into_iter()
                    .find(|pane| pane.id == engine.active_pane_id)
                    .and_then(|pane| pane.selected_surface);
                identify_location(&s, s.active_index, surface)
            });
            let caller = caller.and_then(|caller| {
                let text = |key: &str| caller.get(key).and_then(Value::as_str);
                let surface = text("surface_id").and_then(|id| uuid::Uuid::parse_str(id).ok());
                let index = match (surface, text("workspace_id")) {
                    (Some(surface), _) => s.split_engines.iter().position(|engine| {
                        engine
                            .pane_info()
                            .iter()
                            .any(|pane| pane.surface_ids.contains(&surface))
                    }),
                    (None, Some(workspace)) => s
                        .workspaces
                        .iter()
                        .position(|candidate| candidate.uuid.to_string() == workspace),
                    (None, None) => None,
                }?;
                identify_location(&s, index, surface)
            });
            let _ = resp_tx.send(ok(
                req_id,
                json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "platform": "linux",
                    "socket_path": socket_path,
                    "focused": focused,
                    "caller": caller,
                }),
            ));
        }

        SocketCommand::WorkspaceMetadata {
            req_id,
            workspace,
            action,
            resp_tx,
        } => {
            let mut state = state.borrow_mut();
            let index = match workspace {
                Some(id) => state
                    .workspaces
                    .iter()
                    .position(|workspace| workspace.uuid == id),
                None => Some(state.active_index).filter(|index| *index < state.workspaces.len()),
            };
            let Some(index) = index else {
                let _ = resp_tx.send(err(req_id, "not_found", "workspace not found"));
                return;
            };
            match crate::workspace_metadata::apply(&mut state.workspaces[index].metadata, action) {
                Ok(changed) => {
                    if changed {
                        if let Some(row) = crate::sidebar::row_for_workspace(
                            &state.sidebar_list,
                            state.workspaces[index].id,
                        ) {
                            if let Some(vbox) = crate::sidebar::row_text(&row) {
                                let mut child = vbox.first_child();
                                while let Some(widget) = child {
                                    child = widget.next_sibling();
                                    if widget.has_css_class("workspace-metadata") {
                                        if let Ok(container) = widget.downcast::<gtk4::Box>() {
                                            crate::workspace_metadata::render(
                                                &container,
                                                &state.workspaces[index].metadata,
                                            );
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                        state.trigger_session_save();
                    }
                    let workspace = &state.workspaces[index];
                    let metadata = &workspace.metadata;
                    let focused_surface = state
                        .split_engines
                        .get(index)
                        .and_then(|engine| engine.active_pane_uuid());
                    let _ = resp_tx.send(ok(
                        req_id,
                        json!({"workspace_id":workspace.uuid,
                        "statuses":metadata.statuses,"blocks":metadata.blocks,"progress":metadata.progress,
                        "logs":metadata.logs,"color":workspace.color,
                        "cwd":state.local_workspace_directory(index),
                        "focused_surface_id":focused_surface,"git":workspace.git,"ports":workspace.ports}),
                    ));
                }
                Err(message) => {
                    let _ = resp_tx.send(err(req_id, "capacity", message));
                }
            }
        }

        SocketCommand::ProjectActionRun {
            req_id,
            workspace,
            action_id,
            fingerprint,
            confirmed,
            resp_tx,
        } => {
            super::project::run(
                state,
                super::project::RunRequest {
                    workspace,
                    action_id,
                    fingerprint,
                    confirmed,
                    req_id,
                    trace_id,
                },
                resp_tx,
            );
        }
        SocketCommand::ProjectActionsList {
            req_id,
            workspace,
            resp_tx,
        } => {
            super::project::list(state, workspace, req_id, resp_tx, trace_id);
        }
        SocketCommand::PortsList {
            req_id,
            workspace,
            surface,
            resp_tx,
        } => {
            let s = state.borrow();
            let index = if let Some(id) = workspace {
                s.workspaces
                    .iter()
                    .position(|workspace| workspace.uuid == id)
            } else if let Some(id) = surface {
                s.split_engines.iter().position(|engine| {
                    engine
                        .all_panes()
                        .iter()
                        .any(|(candidate, _, _)| *candidate == id)
                })
            } else {
                (s.active_index < s.workspaces.len()).then_some(s.active_index)
            };
            let Some(index) = index else {
                let _ = resp_tx.send(err(req_id, "not_found", "workspace or surface not found"));
                return;
            };
            if surface.is_some_and(|id| {
                !s.split_engines[index]
                    .all_panes()
                    .iter()
                    .any(|(candidate, _, _)| *candidate == id)
            }) {
                let _ = resp_tx.send(err(
                    req_id,
                    "invalid_params",
                    "surface does not belong to workspace",
                ));
                return;
            }
            let ports = s.workspaces[index].ports.as_ref().map(|ports| {
                ports
                    .iter()
                    .filter(|port| surface.is_none_or(|id| port.surface_uuid == id))
                    .collect::<Vec<_>>()
            });
            let _ = resp_tx.send(ok(
                req_id,
                json!({"workspace_id":s.workspaces[index].uuid,"surface_id":surface,"ports":ports}),
            ));
        }

        SocketCommand::Capabilities { req_id, resp_tx } => {
            let methods: Vec<&str> = vec![
                "system.ping",
                "system.identify",
                "system.capabilities",
                "ports.list",
                "project.actions.list",
                "project.actions.run",
                "sidebar.metadata",
                "sidebar.set_status",
                "sidebar.report_meta_block",
                "sidebar.clear_meta_block",
                "sidebar.clear_status",
                "sidebar.set_progress",
                "sidebar.clear_progress",
                "sidebar.log",
                "sidebar.clear_log",
                "sidebar.state",
                "events.agent_hook",
                "events.stream",
                "system.diagnostics",
                "workspace.list",
                "workspace.current",
                "workspace.create",
                "workspace.select",
                "workspace.close",
                "workspace.rename",
                "workspace.set_description",
                "workspace.clear_description",
                "workspace.next",
                "workspace.previous",
                "workspace.last",
                "workspace.reorder",
                "workspace.reorder_many",
                "workspace.group.list",
                "workspace.group.create",
                "workspace.group.update",
                "workspace.group.assign",
                "workspace.group.delete",
                "surface.list",
                "surface.split",
                "surface.create",
                "surface.focus",
                "surface.close",
                "surface.move",
                "surface.reorder",
                "surface.drag_to_split",
                "surface.send_text",
                "surface.send_key",
                "surface.read_text",
                "surface.read_scrollback",
                "surface.resume.set",
                "surface.resume.show",
                "surface.resume.clear",
                "surface.health",
                "surface.refresh",
                "pane.list",
                "pane.create",
                "pane.focus",
                "pane.last",
                "window.list",
                "window.current",
                "notification.list",
                "notification.clear",
                "notification.create",
                "notification.create_for_surface",
                "notification.create_for_caller",
                "notification.create_for_target",
                "notification.mark_read",
                "notification.dismiss",
                "notification.open",
                "notification.jump_to_unread",
                // Browser lifecycle + streaming
                "browser.open",
                "browser.close",
                "browser.list",
                "browser.stream.enable",
                "browser.stream.disable",
                "browser.snapshot",
                "browser.screenshot",
                // P0: navigation
                "browser.navigate",
                "browser.goto",
                "browser.back",
                "browser.forward",
                "browser.reload",
                // P0: interaction
                "browser.click",
                "browser.dblclick",
                "browser.type",
                "browser.fill",
                "browser.press",
                "browser.keydown",
                "browser.keyup",
                "browser.hover",
                "browser.focus",
                "browser.check",
                "browser.uncheck",
                "browser.select",
                "browser.scroll",
                "browser.scroll_into_view",
                "browser.drag",
                "browser.upload",
                "browser.download",
                "browser.pdf",
                // P0: evaluation + waiting
                "browser.eval",
                "browser.wait",
                // P0: getters
                "browser.get.url",
                "browser.url.get",
                "browser.get.title",
                "browser.get.text",
                "browser.get.html",
                "browser.get.value",
                "browser.get.attr",
                "browser.get.count",
                "browser.get.box",
                "browser.get.styles",
                // P0: state checks
                "browser.is.visible",
                "browser.is.enabled",
                "browser.is.checked",
                // P1: frames, dialogs, console, errors
                "browser.frame.select",
                "browser.frame.main",
                "browser.dialog.accept",
                "browser.dialog.dismiss",
                "browser.console.list",
                "browser.console.clear",
                "browser.errors.list",
                "browser.highlight",
                "browser.state.save",
                "browser.state.load",
                // Debug
                "debug.layout",
                "debug.type",
            ];
            let _ = resp_tx.send(ok(req_id, json!({"methods": methods})));
        }

        // -- workspace.* --
        SocketCommand::WorkspaceList { req_id, resp_tx } => {
            // SOCK-05: No focus side effects.
            let s = state.borrow();
            let list: Vec<Value> = (0..s.workspaces.len())
                .filter_map(|index| workspace_record(&s, index))
                .collect();
            let _ = resp_tx.send(ok(req_id, json!({"workspaces": list})));
        }

        SocketCommand::WorkspaceCurrent { req_id, resp_tx } => {
            // SOCK-05: No focus side effects.
            let s = state.borrow();
            let response = match workspace_record(&s, s.active_index) {
                Some(workspace) => ok(req_id, workspace),
                None => err(req_id, "no_workspace", "no active workspace"),
            };
            let _ = resp_tx.send(response);
        }

        SocketCommand::WorkspaceCreate {
            req_id,
            remote_target,
            name,
            working_directory,
            remote_directory,
            terminal_transport,
            terminal_profile,
            terminal_tmux_session,
            initial_input,
            resp_tx,
        } => {
            if let Some(target) = remote_target {
                // SSH workspace creation per D-13, D-15
                // Create per-workspace bridge for SSH I/O routing
                let bridge = std::sync::Arc::new(crate::ssh::bridge::SshBridge::new());
                *bridge.directory.lock().unwrap() = remote_directory.clone();
                let id = state.borrow_mut().create_remote_workspace_with_transport(
                    target.clone(),
                    &bridge,
                    remote_directory,
                    terminal_transport,
                    terminal_profile,
                    terminal_tmux_session,
                );
                let uuid_str = {
                    let mut s = state.borrow_mut();
                    if let Some(name) = name.filter(|value| !value.trim().is_empty()) {
                        if let Some(index) =
                            s.workspaces.iter().position(|workspace| workspace.id == id)
                        {
                            s.rename_workspace_at(index, name);
                        }
                    }
                    s.workspaces
                        .iter()
                        .find(|ws| ws.id == id)
                        .map(|ws| ws.uuid.to_string())
                        .unwrap_or_default()
                };
                state.borrow_mut().start_ssh(
                    id,
                    target,
                    bridge,
                    trace_id
                        .as_deref()
                        .and_then(|value| uuid::Uuid::parse_str(value).ok()),
                    "rpc",
                );
                let _ = resp_tx.send(ok(
                    req_id,
                    json!({"uuid": uuid_str, "remote": true,
                    "terminal_transport":terminal_transport,"terminal_profile":terminal_profile}),
                ));
            } else {
                let id = if let Some(path) = working_directory {
                    state.borrow_mut().create_workspace_with_input(
                        Some(name.unwrap_or_default()),
                        Some(path),
                        initial_input,
                    )
                } else {
                    let id =
                        state
                            .borrow_mut()
                            .create_workspace_with_input(None, None, initial_input);
                    if let Some(name) = name.filter(|value| !value.trim().is_empty()) {
                        state.borrow_mut().rename_active(name);
                    }
                    id
                };
                let s = state.borrow();
                let workspace = s.workspaces.iter().find(|ws| ws.id == id);
                let uuid_str = workspace.map(|ws| ws.uuid.to_string()).unwrap_or_default();
                let directory = workspace
                    .and_then(|ws| ws.working_directory.as_ref())
                    .map(|path| path.to_string_lossy());
                let _ = resp_tx.send(ok(
                    req_id,
                    json!({
                        "uuid": uuid_str,
                        "working_directory": directory,
                    }),
                ));
            }
            let (list, app) = {
                let s = state.borrow();
                (s.sidebar_list.clone(), s.gtk_app.clone())
            };
            crate::sidebar::wire_latest_row(&list, state.clone(), &app);
        }

        SocketCommand::WorkspaceSelect {
            req_id,
            id,
            resp_tx,
        } => {
            // SOCK-05: workspace.select IS a focus-intent command.
            let idx = {
                let s = state.borrow();
                s.workspaces.iter().position(|ws| ws.uuid.to_string() == id)
            };
            match idx {
                Some(i) => {
                    state.borrow_mut().switch_to_index(i);
                    let _ = resp_tx.send(ok(req_id, json!({})));
                }
                None => {
                    let _ = resp_tx.send(err(req_id, "not_found", "workspace not found"));
                }
            }
        }

        SocketCommand::WorkspaceClose {
            req_id,
            id,
            resp_tx,
        } => {
            // SOCK-05: No focus side effects (close_workspace adjusts index internally).
            let idx = {
                let s = state.borrow();
                s.workspaces.iter().position(|ws| ws.uuid.to_string() == id)
            };
            match idx {
                Some(i) => {
                    let closed = state.borrow_mut().close_workspace(i);
                    if closed {
                        let _ = resp_tx.send(ok(req_id, json!({})));
                    } else {
                        let _ = resp_tx.send(err(
                            req_id,
                            "last_workspace",
                            "cannot close the last workspace",
                        ));
                    }
                }
                None => {
                    let _ = resp_tx.send(err(req_id, "not_found", "workspace not found"));
                }
            }
        }

        SocketCommand::WorkspaceRename {
            req_id,
            id,
            name,
            resp_tx,
        } => {
            // SOCK-05: No focus side effects. Find workspace by uuid, switch to it
            // (rename_active requires the target to be active), then rename.
            let idx = {
                let s = state.borrow();
                s.workspaces.iter().position(|ws| ws.uuid.to_string() == id)
            };
            match idx {
                Some(i) => {
                    let mut s = state.borrow_mut();
                    s.rename_workspace_at(i, name);
                    drop(s);
                    let _ = resp_tx.send(ok(req_id, json!({})));
                }
                None => {
                    let _ = resp_tx.send(err(req_id, "not_found", "workspace not found"));
                }
            }
        }

        SocketCommand::WorkspaceDescription {
            req_id,
            id,
            description,
            resp_tx,
        } => {
            // SOCK-05: no focus side effects either; describe the workspace found by uuid.
            let idx = {
                let s = state.borrow();
                s.workspaces.iter().position(|ws| ws.uuid.to_string() == id)
            };
            match idx {
                Some(i) => {
                    state.borrow_mut().set_workspace_description(i, description);
                    let _ = resp_tx.send(ok(req_id, json!({})));
                }
                None => {
                    let _ = resp_tx.send(err(req_id, "not_found", "workspace not found"));
                }
            }
        }

        SocketCommand::WorkspaceNext { req_id, resp_tx } => {
            // SOCK-05: focus-intent command.
            state.borrow_mut().switch_next();
            let _ = resp_tx.send(ok(req_id, json!({})));
        }

        SocketCommand::WorkspacePrev { req_id, resp_tx } => {
            // SOCK-05: focus-intent command.
            state.borrow_mut().switch_prev();
            let _ = resp_tx.send(ok(req_id, json!({})));
        }

        SocketCommand::WorkspaceLast { req_id, resp_tx } => {
            // SOCK-05: focus-intent command.
            // "Last" = most recently visited; for now same as prev (Phase 4 can track history).
            state.borrow_mut().switch_prev();
            let _ = resp_tx.send(ok(req_id, json!({})));
        }

        SocketCommand::WorkspaceReorderMany {
            req_id,
            order,
            dry_run,
            resp_tx,
        } => {
            let result = state.borrow_mut().reorder_workspaces(&order, dry_run);
            if result.is_ok() && !dry_run {
                crate::sidebar::rebuild_grouped_sidebar(state);
            }
            let response = match result {
                Ok(result) => ok(req_id, result),
                Err(message) => err(
                    req_id,
                    if message == "workspace not found" {
                        "not_found"
                    } else {
                        "invalid_params"
                    },
                    message,
                ),
            };
            let _ = resp_tx.send(response);
        }
        SocketCommand::WorkspaceReorder {
            req_id,
            id,
            position,
            resp_tx,
        } => {
            // SOCK-05: No focus side effects.
            let mut s = state.borrow_mut();
            let idx = s.workspaces.iter().position(|ws| ws.uuid.to_string() == id);
            match idx {
                Some(from) => {
                    let to = position.min(s.workspaces.len().saturating_sub(1));
                    s.reorder_workspace(from, to);
                    drop(s);
                    crate::sidebar::rebuild_grouped_sidebar(state);
                    let _ = resp_tx.send(ok(req_id, json!({})));
                }
                None => {
                    drop(s);
                    let _ = resp_tx.send(err(req_id, "not_found", "workspace not found"));
                }
            }
        }
        SocketCommand::WorkspaceGroupList { req_id, resp_tx } => {
            let state = state.borrow();
            let groups: Vec<_> = state
                .workspace_groups
                .iter()
                .map(|group| {
                    let members: Vec<_> = state
                        .workspaces
                        .iter()
                        .filter(|workspace| workspace.group_id == Some(group.id))
                        .map(|workspace| workspace.uuid)
                        .collect();
                    let unread = state
                        .workspaces
                        .iter()
                        .filter(|workspace| {
                            workspace.group_id == Some(group.id) && workspace.has_attention
                        })
                        .count();
                    json!({"id":group.id,"name":group.name,"color":group.color,
                    "collapsed":group.collapsed,"workspace_ids":members,"unread":unread})
                })
                .collect();
            let _ = resp_tx.send(ok(req_id, json!({"groups":groups})));
        }
        SocketCommand::WorkspaceGroupCreate {
            req_id,
            name,
            color,
            resp_tx,
        } => {
            let response = match state.borrow_mut().create_workspace_group(name, color) {
                Ok(id) => ok(req_id, json!({"id":id})),
                Err(message) => err(req_id, "invalid_params", message),
            };
            crate::sidebar::rebuild_grouped_sidebar(state);
            let _ = resp_tx.send(response);
        }
        SocketCommand::WorkspaceGroupUpdate {
            req_id,
            id,
            name,
            color,
            collapsed,
            position,
            resp_tx,
        } => {
            let response = match state
                .borrow_mut()
                .update_workspace_group(id, name, color, collapsed, position)
            {
                Ok(()) => ok(req_id, json!({"id":id})),
                Err(message) => err(
                    req_id,
                    if message.contains("not found") {
                        "not_found"
                    } else {
                        "invalid_params"
                    },
                    message,
                ),
            };
            crate::sidebar::rebuild_grouped_sidebar(state);
            let _ = resp_tx.send(response);
        }
        SocketCommand::WorkspaceGroupAssign {
            req_id,
            id,
            workspaces,
            resp_tx,
        } => {
            let response = match state.borrow_mut().assign_workspace_group(id, &workspaces) {
                Ok(changed) => ok(req_id, json!({"group_id":id,"changed":changed})),
                Err(message) => err(
                    req_id,
                    if message.contains("not found") {
                        "not_found"
                    } else {
                        "invalid_params"
                    },
                    message,
                ),
            };
            crate::sidebar::rebuild_grouped_sidebar(state);
            let _ = resp_tx.send(response);
        }
        SocketCommand::WorkspaceGroupDelete {
            req_id,
            id,
            resp_tx,
        } => {
            let response = match state.borrow_mut().delete_workspace_group(id) {
                Ok(changed) => ok(req_id, json!({"id":id,"ungrouped":changed})),
                Err(message) => err(req_id, "not_found", message),
            };
            crate::sidebar::rebuild_grouped_sidebar(state);
            let _ = resp_tx.send(response);
        }

        // -- window.* --
        SocketCommand::WindowList { req_id, resp_tx } => {
            // SOCK-05: No focus side effects.
            let s = state.borrow();
            let _ = resp_tx.send(ok(
                req_id,
                json!({
                    "windows": [window_record(&s)]
                }),
            ));
        }

        SocketCommand::WindowCurrent { req_id, resp_tx } => {
            // SOCK-05: No focus side effects.
            let s = state.borrow();
            let window = window_record(&s);
            let _ = resp_tx.send(ok(
                req_id,
                json!({"id": window["id"], "ref": window["ref"], "window_id": window["id"], "window_ref": window["ref"]}),
            ));
        }

        // -- debug.* --
        SocketCommand::DebugLayout { req_id, resp_tx } => {
            // SOCK-05: No focus side effects.
            let s = state.borrow();
            match s.split_engines.get(s.active_index) {
                Some(engine) => {
                    let data = engine.root.to_data();
                    let json_tree = serde_json::to_value(&data).unwrap_or(Value::Null);
                    let _ = resp_tx.send(ok(req_id, json!({"layout": json_tree})));
                }
                None => {
                    let _ = resp_tx.send(err(req_id, "no_workspace", "no active workspace"));
                }
            }
        }

        SocketCommand::DebugType {
            req_id,
            text,
            resp_tx,
        } => {
            // SOCK-05: No focus side effects (sends text to active surface without changing focus).
            let s = state.borrow();
            if let Some(engine) = s.split_engines.get(s.active_index) {
                if let Some(pane_id) = engine.root.find_active_pane_id() {
                    if let Some(surface) = engine.root.find_surface_for_pane(pane_id) {
                        if !surface.is_null() {
                            let c_text = std::ffi::CString::new(text.clone()).unwrap_or_default();
                            unsafe {
                                crate::ghostty::ffi::ghostty_surface_text(
                                    surface,
                                    c_text.as_ptr(),
                                    c_text.to_bytes().len(),
                                );
                            }
                        }
                    }
                }
            }
            let _ = resp_tx.send(ok(req_id, json!({})));
        }

        // ── surface.* ────────────────────────────────────────────────────
        SocketCommand::SurfaceResume {
            req_id,
            id,
            action,
            resp_tx,
        } => {
            let s = state.borrow();
            let id = id.or_else(|| {
                s.split_engines
                    .get(s.active_index)
                    .and_then(|engine| engine.active_pane_uuid())
            });
            let target = id.as_ref().and_then(|id| {
                s.split_engines
                    .iter()
                    .enumerate()
                    .find(|(_, engine)| engine.find_pane_id_by_uuid(id).is_some())
            });
            let response = match (id.as_ref(), target) {
                (Some(id), Some((index, engine))) => match engine.resume_action(id, &action) {
                    Ok(binding) => {
                        if !matches!(action, crate::resume::ResumeAction::Show) {
                            s.trigger_session_save();
                        }
                        ok(
                            req_id,
                            json!({"surface_id": id, "workspace_id": s.workspaces[index].uuid,
                            "resume_binding": binding,
                            "auto_resume": binding.as_ref().is_some_and(|binding| s.resume_policy.allows_automatic(binding)),
                            "execution_location": if s.workspaces[index].remote_target.is_none() {
                                "local"
                            } else if s.workspaces[index].terminal_transport == crate::remote_transport::TerminalTransport::Ssh {
                                "remote_ssh"
                            } else {
                                "remote_mosh"
                            }}),
                        )
                    }
                    Err(message) => err(
                        req_id,
                        if message == "checkpoint mismatch" {
                            "conflict"
                        } else {
                            "invalid_params"
                        },
                        message,
                    ),
                },
                _ => err(req_id, "not_found", "terminal surface not found"),
            };
            let _ = resp_tx.send(response);
        }
        SocketCommand::SurfaceList {
            req_id,
            workspace,
            resp_tx,
        } => {
            // SOCK-05: No focus side effects. `index` counts within each workspace, as on macOS.
            let s = state.borrow();
            let mut surfaces: Vec<Value> = Vec::new();
            for (ws_idx, (ws, engine)) in
                s.workspaces.iter().zip(s.split_engines.iter()).enumerate()
            {
                if workspace.is_some_and(|wanted| wanted != ws.uuid) {
                    continue;
                }
                let workspace = ws.uuid.to_string();
                let mut index = 0;
                for pane in engine.pane_info() {
                    let pane_ref = format!("pane:{}", pane.id);
                    for (index_in_pane, (surface, url)) in
                        pane.surface_ids.iter().zip(&pane.browser_urls).enumerate()
                    {
                        let id = surface.to_string();
                        let selected = pane.selected_surface == Some(*surface);
                        surfaces.push(json!({
                            "uuid": id,
                            "id": id,
                            "ref": s.handles.ensure_ref(super::handles::HandleKind::Surface, &id),
                            "index": index,
                            "type": if url.is_some() { "browser" } else { "terminal" },
                            "title": if url.is_some() { None } else { engine.surface_title(&id) },
                            "url": url,
                            "pane_id": pane_ref,
                            "pane_ref": pane_ref,
                            "index_in_pane": index_in_pane,
                            "selected_in_pane": selected,
                            "workspace_uuid": workspace,
                            "workspace_id": workspace,
                            "workspace_ref": s.handles.ensure_ref(super::handles::HandleKind::Workspace, &workspace),
                            "active": selected && pane.id == engine.active_pane_id && ws_idx == s.active_index,
                        }));
                        index += 1;
                    }
                }
            }
            let _ = resp_tx.send(ok(req_id, json!({"surfaces": surfaces})));
        }

        SocketCommand::SurfaceSplit {
            req_id,
            id,
            workspace,
            caller,
            pane,
            direction,
            launch,
            focus,
            resp_tx,
        } => {
            // Split the targeted pane; the window and keyboard focus change only with `focus`.
            let result = {
                let mut s = state.borrow_mut();
                creation_target(
                    &s,
                    id.as_deref(),
                    pane.as_deref(),
                    workspace.as_deref(),
                    caller.as_deref(),
                )
                .and_then(|(index, pane_id)| {
                    if focus && index != s.active_index {
                        s.switch_to_index(index);
                    }
                    let engine = s
                        .split_engines
                        .get_mut(index)
                        .ok_or("workspace not found")?;
                    let (new_pane, surface) =
                        engine.split_pane_terminal(pane_id, direction, launch, focus)?;
                    Ok(created_json(&s, index, new_pane, surface))
                })
            };
            let _ = resp_tx.send(match result {
                Ok(created) => ok(req_id, created),
                Err(message) => err(req_id, "split_failed", message),
            });
        }

        SocketCommand::SurfaceCreate {
            req_id,
            workspace,
            caller,
            pane,
            launch,
            focus,
            resp_tx,
        } => {
            let result = {
                let mut s = state.borrow_mut();
                creation_target(
                    &s,
                    None,
                    pane.as_deref(),
                    workspace.as_deref(),
                    caller.as_deref(),
                )
                .and_then(|(index, pane_id)| {
                    if focus && index != s.active_index {
                        s.switch_to_index(index);
                    }
                    let engine = s
                        .split_engines
                        .get_mut(index)
                        .ok_or("workspace not found")?;
                    let surface = engine.new_terminal_surface(pane_id, launch, focus)?;
                    Ok(created_json(&s, index, pane_id, surface))
                })
            };
            let _ = resp_tx.send(match result {
                Ok(created) => ok(req_id, created),
                Err(message) => err(req_id, "create_failed", message),
            });
        }

        SocketCommand::SurfaceFocus {
            req_id,
            id,
            resp_tx,
        } => {
            // SOCK-05: surface.focus IS a focus-intent command — allowed to change focus.
            let focused = {
                let mut s = state.borrow_mut();
                let index = s
                    .split_engines
                    .iter()
                    .position(|engine| engine.find_pane_id_by_uuid(&id).is_some());
                if let Some(index) = index {
                    s.switch_to_index(index);
                    s.split_engines[index].focus_surface(&id)
                } else {
                    false
                }
            };
            let response = if focused {
                ok(req_id, json!({}))
            } else {
                err(req_id, "not_found", "surface not found")
            };
            let _ = resp_tx.send(response);
        }

        SocketCommand::SurfaceClose {
            req_id,
            id,
            resp_tx,
        } => {
            let Some(uuid) = uuid::Uuid::parse_str(&id).ok() else {
                let _ = resp_tx.send(err(req_id, "invalid_request", "invalid surface UUID"));
                return;
            };
            let closed = {
                let mut s = state.borrow_mut();
                let index = s
                    .split_engines
                    .iter()
                    .position(|engine| engine.find_pane_id_by_uuid(&id).is_some());
                index.map(|index| s.split_engines[index].close_surface_and_empty_pane(uuid))
            };
            match closed {
                Some(crate::split_engine::CloseSurfaceResult::Closed) => {
                    state.borrow().trigger_session_save();
                    let _ = resp_tx.send(ok(req_id, json!({})));
                }
                Some(crate::split_engine::CloseSurfaceResult::LastSurfaceInPane) => {
                    let _ = resp_tx.send(err(req_id, "close_failed", "cannot close last surface"));
                }
                Some(crate::split_engine::CloseSurfaceResult::NotFound) | None => {
                    let _ = resp_tx.send(err(req_id, "not_found", "surface not found"));
                }
            }
        }

        SocketCommand::SurfaceMove {
            req_id,
            id,
            workspace,
            pane,
            position,
            before,
            after,
            focus,
            resp_tx,
        } => {
            let Some(uuid) = uuid::Uuid::parse_str(&id).ok() else {
                let _ = resp_tx.send(err(req_id, "invalid_params", "invalid surface UUID"));
                return;
            };
            let pane_id = match pane.as_deref() {
                None => None,
                Some(reference) => {
                    let Some(id) = reference
                        .strip_prefix("pane:")
                        .and_then(|value| value.parse::<u64>().ok())
                    else {
                        let _ =
                            resp_tx.send(err(req_id, "invalid_params", "invalid pane reference"));
                        return;
                    };
                    Some(id)
                }
            };
            let response = {
                let mut s = state.borrow_mut();
                let Some(source_index) = s
                    .split_engines
                    .iter()
                    .position(|engine| engine.find_pane_id_by_uuid(&id).is_some())
                else {
                    let _ = resp_tx.send(err(req_id, "not_found", "surface not found"));
                    return;
                };
                // `before` / `after` name an anchor surface whose pane and slot decide both the
                // destination and the position; an explicit pane or workspace must agree with it.
                let anchor = match (before.as_deref(), after.as_deref()) {
                    (None, None) => None,
                    (Some(_), Some(_)) => {
                        let _ = resp_tx.send(err(
                            req_id,
                            "invalid_params",
                            "only one of before or after may be given",
                        ));
                        return;
                    }
                    (before, after) => {
                        let reference = before.or(after).expect("one anchor is present");
                        let Ok(anchor) = uuid::Uuid::parse_str(reference) else {
                            let _ =
                                resp_tx.send(err(req_id, "invalid_params", "invalid surface UUID"));
                            return;
                        };
                        if anchor == uuid {
                            let _ = resp_tx.send(err(
                                req_id,
                                "invalid_params",
                                "before/after cannot name the surface being moved",
                            ));
                            return;
                        }
                        let text = anchor.to_string();
                        let found =
                            s.split_engines
                                .iter()
                                .enumerate()
                                .find_map(|(index, engine)| {
                                    engine
                                        .surface_location(&text)
                                        .map(|(pane, slot)| (index, pane, slot))
                                });
                        let Some((engine_index, pane, slot)) = found else {
                            let _ = resp_tx.send(err(
                                req_id,
                                "not_found",
                                "before/after surface not found",
                            ));
                            return;
                        };
                        Some((engine_index, pane, slot, before.is_some()))
                    }
                };
                let destination_workspace = match workspace.as_deref() {
                    Some(value) => match uuid::Uuid::parse_str(value) {
                        Ok(value) => value,
                        Err(_) => {
                            let _ = resp_tx.send(err(
                                req_id,
                                "invalid_params",
                                "invalid workspace UUID",
                            ));
                            return;
                        }
                    },
                    None => match anchor {
                        Some((engine_index, _, _, _)) => s.workspaces[engine_index].uuid,
                        None => pane_id
                            .and_then(|pane_id| {
                                s.split_engines
                                    .iter()
                                    .position(|engine| engine.contains_pane(pane_id))
                            })
                            .and_then(|index| {
                                s.workspaces.get(index).map(|workspace| workspace.uuid)
                            })
                            .unwrap_or(s.workspaces[source_index].uuid),
                    },
                };
                let (destination_pane, placement) = match anchor {
                    Some((engine_index, anchor_pane, anchor_slot, place_before)) => {
                        if s.workspaces[engine_index].uuid != destination_workspace {
                            let _ = resp_tx.send(err(
                                req_id,
                                "invalid_params",
                                "before/after and workspace name different workspaces",
                            ));
                            return;
                        }
                        if pane_id.is_some_and(|pane| pane != anchor_pane) {
                            let _ = resp_tx.send(err(
                                req_id,
                                "invalid_params",
                                "before/after and pane name different panes",
                            ));
                            return;
                        }
                        // In the anchor's own pane the removal of the moved tab shifts the slot.
                        let source_slot = if source_index == engine_index {
                            s.split_engines[source_index]
                                .surface_location(&id)
                                .filter(|(source_pane, _)| *source_pane == anchor_pane)
                                .map(|(_, slot)| slot)
                        } else {
                            None
                        };
                        let shifted = match source_slot {
                            Some(source_slot) => {
                                anchor_slot - usize::from(source_slot < anchor_slot)
                            }
                            None => anchor_slot,
                        };
                        let slot = if place_before { shifted } else { shifted + 1 };
                        (Some(anchor_pane), Some(slot))
                    }
                    None => (pane_id, position),
                };
                match s.move_surface_between_workspaces(
                    uuid,
                    destination_workspace,
                    destination_pane,
                    placement,
                    focus,
                ) {
                    Ok((result, route_restarted)) => ok(
                        req_id,
                        json!({
                            "id": id,
                            "workspace_id": destination_workspace,
                            "pane": format!("pane:{}", result.pane_id),
                            "position": result.position,
                            "browser_route_restarted": route_restarted,
                        }),
                    ),
                    Err(message) => err(
                        req_id,
                        if message.contains("not found") {
                            "not_found"
                        } else {
                            "invalid_params"
                        },
                        message,
                    ),
                }
            };
            let _ = resp_tx.send(response);
        }

        SocketCommand::SurfaceReorder {
            req_id,
            id,
            position,
            resp_tx,
        } => {
            let Some(uuid) = uuid::Uuid::parse_str(&id).ok() else {
                let _ = resp_tx.send(err(req_id, "invalid_params", "invalid surface UUID"));
                return;
            };
            let response = {
                let mut s = state.borrow_mut();
                let Some(index) = s
                    .split_engines
                    .iter()
                    .position(|engine| engine.find_pane_id_by_uuid(&id).is_some())
                else {
                    let _ = resp_tx.send(err(req_id, "not_found", "surface not found"));
                    return;
                };
                match s.split_engines[index].reorder_surface(uuid, position) {
                    Ok(result) => {
                        s.trigger_session_save();
                        ok(
                            req_id,
                            json!({
                                "id": id,
                                "workspace_id": s.workspaces[index].uuid,
                                "pane": format!("pane:{}", result.pane_id),
                                "position": result.position,
                            }),
                        )
                    }
                    Err(message) => err(
                        req_id,
                        if message.contains("not found") {
                            "not_found"
                        } else {
                            "invalid_params"
                        },
                        message,
                    ),
                }
            };
            let _ = resp_tx.send(response);
        }

        SocketCommand::SurfaceDragToSplit {
            req_id,
            id,
            target_pane,
            direction,
            resp_tx,
        } => {
            let Some(uuid) = uuid::Uuid::parse_str(&id).ok() else {
                let _ = resp_tx.send(err(req_id, "invalid_params", "invalid surface UUID"));
                return;
            };
            let Some(target_pane_id) = target_pane
                .strip_prefix("pane:")
                .and_then(|value| value.parse::<u64>().ok())
            else {
                let _ = resp_tx.send(err(req_id, "invalid_params", "invalid pane reference"));
                return;
            };
            let response = {
                let mut s = state.borrow_mut();
                let Some(index) = s
                    .split_engines
                    .iter()
                    .position(|engine| engine.find_pane_id_by_uuid(&id).is_some())
                else {
                    let _ = resp_tx.send(err(req_id, "not_found", "surface not found"));
                    return;
                };
                match s.split_engines[index].drag_surface_to_split(uuid, target_pane_id, direction)
                {
                    Ok(pane_id) => {
                        s.switch_to_index(index);
                        s.trigger_session_save();
                        ok(
                            req_id,
                            json!({
                                "id": id,
                                "workspace_id": s.workspaces[index].uuid,
                                "pane": format!("pane:{pane_id}"),
                            }),
                        )
                    }
                    Err(message) => err(
                        req_id,
                        if message.contains("not found") {
                            "not_found"
                        } else {
                            "invalid_params"
                        },
                        message,
                    ),
                }
            };
            let _ = resp_tx.send(response);
        }

        SocketCommand::SurfaceSendText {
            req_id,
            id,
            text,
            resp_tx,
        } => {
            let response = match send_terminal_text(state, id.as_deref(), &text) {
                Ok(()) => ok(req_id, json!({})),
                Err((code, message)) => err(req_id, code, message),
            };
            let _ = resp_tx.send(response);
        }

        SocketCommand::SurfaceSendKey {
            req_id,
            id,
            key,
            resp_tx,
        } => {
            // Named keys are synthesized as a press/release pair, literal
            // characters use typed input; an unknown key is never reported as delivered.
            let mut characters = key.chars();
            let result = if let Some(named) = crate::ghostty::named_key::parse(&key) {
                terminal_target(state, id.as_deref()).map(|surface| {
                    // SAFETY: resolution releases the model borrow and returns a
                    // live GTK-owned terminal; no teardown occurs before input.
                    unsafe { crate::ghostty::named_key::send(surface, named) }
                })
            } else if let (Some(character), None) = (characters.next(), characters.next()) {
                terminal_target(state, id.as_deref()).and_then(|surface| {
                    // SAFETY: resolution releases the model borrow and returns a
                    // live GTK-owned terminal; no teardown occurs before input.
                    unsafe { crate::ghostty::text::send_character(surface, character) }
                        .map_err(|message| ("invalid_params", message))
                })
            } else {
                Err((
                    "not_supported",
                    "send-key accepts a key name (enter, tab, escape, up, ctrl-c…) or one literal character",
                ))
            };
            let response = match result {
                Ok(()) => ok(req_id, json!({})),
                Err((code, message)) => err(req_id, code, message),
            };
            let _ = resp_tx.send(response);
        }

        SocketCommand::SurfaceReadText {
            req_id,
            scrollback,
            id,
            resp_tx,
        } => {
            let result = terminal_target(state, id.as_deref()).and_then(|surface| {
                // SAFETY: resolution found a live GTK-owned terminal. No model
                // borrow or event-loop iteration spans this bounded native read.
                unsafe {
                    if scrollback {
                        crate::ghostty::text::read_scrollback(surface)
                    } else {
                        crate::ghostty::text::read_visible(surface)
                    }
                }
                .map_err(|message| ("read_failed", message))
            });
            let response = match result {
                Ok(text) => ok(req_id, json!({"text": text})),
                Err((code, message)) => err(req_id, code, message),
            };
            let _ = resp_tx.send(response);
        }

        SocketCommand::SurfaceHealth {
            req_id,
            id,
            resp_tx,
        } => {
            // SOCK-05: health is NOT focus-intent — NO focus change.
            let (found, has_attention) = {
                let s = state.borrow();
                if let Some(engine) = s.split_engines.get(s.active_index) {
                    if let Some(ref uuid_str) = id {
                        let alive = engine.find_surface_by_uuid(uuid_str).is_some();
                        let attn = engine
                            .find_pane_id_by_uuid(uuid_str)
                            .map(|pid| engine.root.pane_has_attention(pid))
                            .unwrap_or(false);
                        (alive, attn)
                    } else {
                        let alive = engine
                            .root
                            .find_surface_for_pane(engine.active_pane_id)
                            .is_some();
                        let attn = engine.root.pane_has_attention(engine.active_pane_id);
                        (alive, attn)
                    }
                } else {
                    (false, false)
                }
            };
            let _ = resp_tx.send(ok(
                req_id,
                json!({"alive": found, "has_attention": has_attention}),
            ));
        }

        SocketCommand::SurfaceRefresh {
            req_id,
            id,
            resp_tx,
        } => {
            // SOCK-05: refresh is NOT focus-intent — NO focus change.
            // Queue a render on the target surface's GLArea.
            let gl_area = {
                let s = state.borrow();
                if let Some(engine) = s.split_engines.get(s.active_index) {
                    match id.as_deref() {
                        Some(uuid) => engine.gl_area_for_surface(uuid),
                        None => engine.gl_area_for_pane(engine.active_pane_id),
                    }
                } else {
                    None
                }
            };
            let response = if let Some(area) = gl_area {
                area.queue_render();
                ok(req_id, json!({}))
            } else {
                err(req_id, "not_found", "terminal surface not found")
            };
            let _ = resp_tx.send(response);
        }

        // ── pane.* ───────────────────────────────────────────────────────────
        SocketCommand::PaneList {
            req_id,
            workspace,
            resp_tx,
        } => {
            let s = state.borrow();
            let mut panes = Vec::new();
            let ancestor = s
                .stack
                .root()
                .and_then(|root| root.downcast::<gtk4::ApplicationWindow>().ok())
                .map(|window| window.upcast::<gtk4::Widget>());
            for (ws_idx, (ws, engine)) in s.workspaces.iter().zip(&s.split_engines).enumerate() {
                if workspace.is_some_and(|wanted| wanted != ws.uuid) {
                    continue;
                }
                let geometry = ancestor
                    .as_ref()
                    .map(|ancestor| engine.pane_geometry(ancestor))
                    .unwrap_or_default();
                for (index, pane) in engine.pane_info().into_iter().enumerate() {
                    let realized = geometry.iter().find(|item| item.id == pane.id);
                    let workspace = ws.uuid.to_string();
                    let surface_refs: Vec<String> = pane
                        .surface_ids
                        .iter()
                        .map(|id| {
                            s.handles
                                .ensure_ref(super::handles::HandleKind::Surface, &id.to_string())
                        })
                        .collect();
                    panes.push(json!({
                        "id": format!("pane:{}", pane.id),
                        "ref": format!("pane:{}", pane.id),
                        "index": index,
                        "uuid": pane.selected_surface,
                        "workspace_uuid": ws.uuid,
                        "workspace_id": workspace,
                        "workspace_ref": s.handles.ensure_ref(super::handles::HandleKind::Workspace, &workspace),
                        "surface_refs": surface_refs,
                        "selected_surface_ref": pane.selected_surface.map(|id| s.handles.ensure_ref(super::handles::HandleKind::Surface, &id.to_string())),
                        "surface_ids": pane.surface_ids,
                        "active_surface_uuid": pane.selected_surface,
                        "focused": ws_idx == s.active_index && pane.id == engine.active_pane_id,
                        "active": ws_idx == s.active_index && pane.id == engine.active_pane_id,
                        "bounds": realized.and_then(|item| item.bounds),
                        "surface_bounds": realized.map(|item| item.surface_bounds.iter().map(|(id, bounds)| json!({
                            "surface_id": id,
                            "bounds": bounds,
                        })).collect::<Vec<_>>()).unwrap_or_default(),
                    }));
                }
            }
            let _ = resp_tx.send(ok(req_id, json!({"panes": panes})));
        }

        SocketCommand::PaneFocus {
            req_id,
            id,
            resp_tx,
        } => {
            let focused = {
                let mut s = state.borrow_mut();
                // A pane in another workspace selects that workspace first, as on macOS.
                let owner = id.as_deref().and_then(|reference| {
                    let number = reference
                        .strip_prefix("pane:")
                        .and_then(|value| value.parse::<u64>().ok());
                    s.split_engines.iter().position(|engine| match number {
                        Some(number) => engine.pane_info().iter().any(|pane| pane.id == number),
                        None => engine.find_pane_id_by_uuid(reference).is_some(),
                    })
                });
                if let Some(owner) = owner.filter(|owner| *owner != s.active_index) {
                    s.switch_to_index(owner);
                }
                let idx = s.active_index;
                s.split_engines.get_mut(idx).is_some_and(|engine| {
                    id.as_deref()
                        .is_some_and(|reference| engine.focus_pane_ref(reference))
                })
            };
            let response = if focused {
                ok(req_id, json!({}))
            } else {
                err(req_id, "not_found", "pane not found")
            };
            let _ = resp_tx.send(response);
        }

        SocketCommand::PaneLast { req_id, resp_tx } => {
            // SOCK-05: pane.last IS focus-intent — allowed to change focus.
            // Phase 3 stub: re-grab focus on current active pane. Phase 4 tracks focus history.
            {
                let s = state.borrow();
                if let Some(engine) = s.split_engines.get(s.active_index) {
                    engine.grab_active_focus();
                }
            }
            let _ = resp_tx.send(ok(req_id, json!({})));
        }

        // -- notification.* (Phase 4) --
        SocketCommand::NotificationList { req_id, resp_tx } => {
            // SOCK-05: No focus side effects. Read-only attention state query.
            let s = state.borrow();
            let notifications: Vec<Value> = s
                .workspaces
                .iter()
                .map(|ws| {
                    json!({
                        "workspace_uuid": ws.uuid.to_string(),
                        "workspace_name": ws.name,
                        "has_attention": ws.has_attention,
                    })
                })
                .collect();
            let _ = resp_tx.send(ok(
                req_id,
                json!({"notifications": s.inbox.records, "workspace_attention": notifications}),
            ));
        }

        SocketCommand::Inbox {
            req_id,
            action,
            resp_tx,
        } => {
            let response = match crate::inbox_actions::handle(&mut state.borrow_mut(), action) {
                Ok(value) => ok(req_id, value),
                Err((code, message)) => err(req_id, code, message),
            };
            let _ = resp_tx.send(response);
        }

        // -- browser.* (Phase 8: D-04 lifecycle + streaming) --
        // SOCK-05: None of these commands steal focus.
        SocketCommand::BrowserOpen {
            req_id,
            url,
            workspace,
            profile,
            resp_tx,
        } => {
            let mut params = json!({"url": crate::browser_address::normalize(&url)});
            if let Some(workspace) = workspace {
                params["workspace"] = json!(workspace);
            }
            if let Some(profile) = profile {
                params["profile"] = json!(profile);
            }
            start_browser_lifecycle(
                state,
                crate::browser::StartupRequest::Open(params),
                req_id,
                resp_tx,
                trace_id,
                None,
            );
        }

        SocketCommand::BrowserStreamEnable { req_id, resp_tx } => {
            let s = state.borrow();
            if !s.browser_sessions.is_empty() {
                let Some(browser) =
                    browser_target(&s, None).and_then(|id| s.browser_sessions.get(&id))
                else {
                    let _ = resp_tx.send(err(
                        req_id,
                        "surface_not_found",
                        "select a browser surface for streaming",
                    ));
                    return;
                };
                let Some(runtime) = s.runtime_handle.clone() else {
                    let _ = resp_tx.send(err(req_id, "not_running", "Async runtime unavailable"));
                    return;
                };
                let exchange = browser.send_command_async(
                    "stream_enable",
                    json!({}),
                    trace_id
                        .as_deref()
                        .and_then(|id| uuid::Uuid::parse_str(id).ok()),
                );
                drop(s);
                spawn_browser_exchange(
                    &runtime,
                    exchange,
                    req_id,
                    resp_tx,
                    "stream_error",
                    trace_id,
                );
                return;
            }
            drop(s);
            start_browser_lifecycle(
                state,
                crate::browser::StartupRequest::Stream,
                req_id,
                resp_tx,
                trace_id,
                None,
            );
        }

        SocketCommand::BrowserStreamDisable { req_id, resp_tx } => {
            let s = state.borrow();
            let Some(browser) = browser_target(&s, None).and_then(|id| s.browser_sessions.get(&id))
            else {
                let _ = resp_tx.send(err(req_id, "not_running", "No browser session active"));
                return;
            };
            let Some(runtime) = s.runtime_handle.clone() else {
                let _ = resp_tx.send(err(req_id, "not_running", "Async runtime unavailable"));
                return;
            };
            let exchange = browser.send_command_async(
                "stream_disable",
                json!({}),
                trace_id
                    .as_deref()
                    .and_then(|id| uuid::Uuid::parse_str(id).ok()),
            );
            drop(s);
            spawn_browser_exchange(
                &runtime,
                exchange,
                req_id,
                resp_tx,
                "stream_error",
                trace_id,
            );
        }

        SocketCommand::BrowserList { req_id, resp_tx } => {
            let s = state.borrow();
            let surfaces: Vec<_> = s.split_engines.iter().enumerate().flat_map(|(index, engine)| {
                engine.browser_tabs().into_iter().map(move |widgets| (index, widgets))
            }).map(|(index, widgets)| {
                let id = widgets.uuid;
                let reference = s.browser_surface_refs.iter().find(|(_, value)| *value == &id.to_string()).map(|(reference, _)| format!("surface:{reference}"));
                let status = match s.browser_sessions.get(&id) {
                    Some(browser) if matches!(browser.preview_state, crate::browser::PreviewState::Connected | crate::browser::PreviewState::Streaming) => "connected",
                    Some(_) => "starting",
                    None => "suspended",
                };
                json!({"ref":reference,"uuid":id,"workspace_uuid":s.workspaces[index].uuid,"status":status,"url":widgets.url_entry.text().to_string(),"profile":widgets.profile})
            }).collect();
            let _ = resp_tx.send(ok(req_id, serde_json::json!({"surfaces": surfaces})));
        }

        // -- browser.* generic proxy (P0/P1 parity) --
        SocketCommand::BrowserAction {
            req_id,
            action,
            mut params,
            surface_ref,
            mut resp_tx,
        } => {
            if action == "close" {
                let mut s = state.borrow_mut();
                let targets: Vec<_> = if surface_ref.is_none() {
                    s.split_engines
                        .iter()
                        .flat_map(|engine| engine.browser_tabs())
                        .map(|widgets| widgets.uuid)
                        .collect()
                } else if let Some(id) = surface_ref
                    .as_deref()
                    .and_then(|target| resolve_surface_ref(target, &s.browser_surface_refs).ok())
                    .and_then(|value| uuid::Uuid::parse_str(&value).ok())
                {
                    vec![id]
                } else {
                    let _ = resp_tx.send(err(
                        req_id,
                        "surface_not_found",
                        "browser surface not found",
                    ));
                    return;
                };
                let mut rejected = false;
                for id in targets {
                    rejected |= !matches!(
                        s.close_browser_surface(id),
                        crate::split_engine::CloseSurfaceResult::Closed
                    );
                }
                let response = if rejected {
                    err(req_id, "close_failed", "cannot close the final surface of a workspace; that browser remains running")
                } else {
                    ok(req_id, json!({"success":true,"data":{}}))
                };
                let _ = resp_tx.send(response);
                return;
            }
            let s = state.borrow();
            let Some(id) = browser_target(&s, surface_ref.as_deref()) else {
                let _ = resp_tx.send(err(
                    req_id,
                    "surface_not_found",
                    "select or specify a live browser surface",
                ));
                return;
            };
            if !s.browser_sessions.contains_key(&id) {
                drop(s);
                let weak = std::rc::Rc::downgrade(state);
                glib::MainContext::default().spawn_local(async move {
                    let trace = trace_id.as_deref().and_then(|value| uuid::Uuid::parse_str(value).ok());
                    let result = tokio::select! {
                        biased;
                        _ = resp_tx.closed() => return,
                        result = crate::browser::ui::initialize_browser_surface(weak.clone(), id, trace) => result,
                    };
                    match (result, weak.upgrade()) {
                        (Ok(()), Some(state)) => handle_socket_command_traced(
                            SocketCommand::BrowserAction { req_id, action, params, surface_ref: Some(id.to_string()), resp_tx },
                            &state, trace_id,
                        ),
                        (Err(error), _) => { let _ = resp_tx.send(err(req_id, "browser_error", &error)); }
                        (_, None) => { let _ = resp_tx.send(err(req_id, "not_running", "application closed")); }
                    }
                });
                return;
            }
            if let Some(bm) = s.browser_sessions.get(&id) {
                if !params.is_object() {
                    params = json!({});
                }
                let fields = params.as_object_mut().unwrap();
                fields.remove("surface_ref");
                fields.remove("surface_id");
                // Translate cmux CLI and upstream action names to agent-browser action names
                let daemon_action = match crate::browser::daemon_action(&action, fields) {
                    Ok(daemon_action) => daemon_action,
                    Err(message) => {
                        let _ = resp_tx.send(err(req_id, "not_supported", &message));
                        return;
                    }
                };
                let style_property = (daemon_action == "styles")
                    .then(|| {
                        fields
                            .get("property")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .flatten();
                let Some(runtime) = s.runtime_handle.clone() else {
                    let _ = resp_tx.send(err(req_id, "not_running", "Async runtime unavailable"));
                    return;
                };
                let session = bm.session_identity();
                let exchange = bm.send_command_async(
                    &daemon_action,
                    params,
                    trace_id
                        .as_deref()
                        .and_then(|id| uuid::Uuid::parse_str(id).ok()),
                );
                let (url_tx, url_rx) = tokio::sync::oneshot::channel();
                drop(s);
                spawn_browser_exchange(
                    &runtime,
                    async move {
                        let mut result = exchange.await?;
                        if let Some(property) = style_property {
                            result = crate::browser::pick_style(result, &property);
                        }
                        if result.get("success").and_then(Value::as_bool) != Some(false) {
                            if let Some(url) = result
                                .get("data")
                                .and_then(|data| data.get("url"))
                                .and_then(Value::as_str)
                                .filter(|url| url.len() <= 8192)
                            {
                                let _ = url_tx.send(url.to_owned());
                            }
                        }
                        Ok(result)
                    },
                    req_id,
                    resp_tx,
                    "browser_error",
                    trace_id,
                );
                let weak = std::rc::Rc::downgrade(state);
                glib::MainContext::default().spawn_local(async move {
                    let Ok(url) = url_rx.await else {
                        return;
                    };
                    let Some(state) = weak.upgrade() else {
                        return;
                    };
                    let s = state.borrow();
                    if s.browser_sessions
                        .get(&id)
                        .is_none_or(|browser| browser.session_identity() != session)
                    {
                        return;
                    }
                    if let Some(widgets) = s
                        .split_engines
                        .iter()
                        .flat_map(|engine| engine.browser_tabs())
                        .find(|widgets| widgets.uuid == id)
                    {
                        if !crate::browser::location::is_editing(&widgets.url_entry) {
                            widgets.url_entry.set_text(&url);
                            s.trigger_session_save();
                        }
                    }
                });
            } else {
                let _ = resp_tx.send(err(req_id, "not_running", "No browser session active"));
            }
        }
    }
}

/// Initialize and command the daemon on Tokio, then apply surviving results on GTK without stealing focus.
pub(super) fn start_browser_lifecycle(
    state: &std::rc::Rc<std::cell::RefCell<crate::app_state::AppState>>,
    mut request: crate::browser::StartupRequest,
    req_id: Value,
    mut resp_tx: super::commands::RespTx,
    trace_id: Option<String>,
    project_surface: Option<String>,
) {
    let started = std::time::Instant::now();
    let initial_url = match &request {
        crate::browser::StartupRequest::Open(params) => {
            params.get("url").and_then(Value::as_str).map(str::to_owned)
        }
        _ => None,
    };
    let initial_profile = match &mut request {
        crate::browser::StartupRequest::Open(params) => params
            .as_object_mut()
            .and_then(|params| params.remove("profile"))
            .and_then(|value| value.as_str().and_then(crate::browser::profile_selector)),
        _ => None,
    };
    let trace = trace_id
        .as_deref()
        .and_then(|id| uuid::Uuid::parse_str(id).ok())
        .unwrap_or_else(uuid::Uuid::new_v4);
    let (session, workspace, mut task) = {
        let mut s = state.borrow_mut();
        let Some(runtime) = s.runtime_handle.clone() else {
            let _ = resp_tx.send(err(req_id, "not_running", "Async runtime unavailable"));
            return;
        };
        let explicit_workspace = match &mut request {
            crate::browser::StartupRequest::Open(params) => params
                .as_object_mut()
                .and_then(|params| params.remove("workspace")),
            _ => None,
        };
        let workspace = match explicit_workspace.filter(|value| !value.is_null()) {
            Some(value) => {
                let Some(id) = value
                    .as_str()
                    .and_then(|value| uuid::Uuid::parse_str(value).ok())
                else {
                    let _ = resp_tx.send(err(req_id, "invalid_params", "invalid workspace UUID"));
                    return;
                };
                if !s.workspaces.iter().any(|workspace| workspace.uuid == id) {
                    let _ = resp_tx.send(err(req_id, "not_found", "workspace not found"));
                    return;
                }
                Some(id)
            }
            None => s
                .workspaces
                .get(s.active_index)
                .map(|workspace| workspace.uuid),
        };
        if s.browser_manager.is_some() {
            let _ = resp_tx.send(err(req_id, "busy", "another browser is starting"));
            return;
        }
        let manager = match workspace
            .ok_or_else(|| "browser workspace missing".to_string())
            .and_then(|id| crate::browser::BrowserManager::for_workspace(&s, id))
        {
            Ok(manager) => manager,
            Err(message) => {
                let _ = resp_tx.send(err(req_id, "browser_error", &message));
                return;
            }
        };
        let mut manager = manager;
        manager.set_profile(initial_profile.clone());
        let browser = s.browser_manager.insert(manager);
        (
            browser.session_identity(),
            workspace,
            runtime.spawn(browser.startup_async(request, trace)),
        )
    };
    let guard = crate::task::AbortOnDrop(task.abort_handle());
    let owner = crate::browser::ui::StartupOwner::new(state, session.clone());
    let state = std::rc::Rc::downgrade(state);
    glib::MainContext::default().spawn_local(async move {
        let _guard = guard;
        let _owner = owner;
        let mut activity = crate::browser::metrics::Activity::begin("rpc_startup", Some(trace));
        let completed = tokio::select! {
            biased;
            _ = resp_tx.closed() => { task.abort(); let _ = task.await; return; }
            result = &mut task => result,
        };
        let (binary, mut result) = match completed {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                activity.finish("error");
                let _ = resp_tx.send(err(req_id, error.code, &error.message));
                return;
            }
            Err(_) => {
                activity.finish("task_error");
                let _ = resp_tx.send(err(req_id, "daemon_error", "Browser startup worker failed"));
                return;
            }
        };
        let Some(state) = state.upgrade() else {
            return;
        };
        let new_widgets = {
            let mut s = state.borrow_mut();
            if !s
                .browser_manager
                .as_mut()
                .is_some_and(|browser| browser.install_startup(&session, binary))
            {
                activity.finish("stale_manager");
                let _ = resp_tx.send(err(req_id, "not_running", "Browser session was replaced"));
                return;
            }
            let index = workspace.and_then(|id| {
                s.workspaces
                    .iter()
                    .position(|workspace| workspace.uuid == id)
            });
            let Some(index) = index else {
                activity.finish("missing_workspace");
                let _ = resp_tx.send(err(
                    req_id,
                    "not_found",
                    "Target workspace closed during browser startup",
                ));
                return;
            };
            if project_surface.as_ref().is_some_and(|surface| s.split_engines[index].active_pane_uuid().as_ref() != Some(surface)) {
                activity.finish("changed_context");
                let _ = resp_tx.send(err(req_id, "changed", "project browser target changed during startup"));
                return;
            }
            let widgets = s
                .split_engines
                .get_mut(index)
                .and_then(|engine| engine.add_preview_with_profile(false, initial_profile.clone()));
            if let Some(widgets) = &widgets {
                if let Some(url) = &initial_url {
                    widgets.url_entry.set_text(url);
                }
                let id = widgets.uuid;
                let ref_id = s
                    .handles
                    .ordinal(super::handles::HandleKind::Surface, &id.to_string());
                s.browser_surface_refs.insert(ref_id, id.to_string());
                if let Some(fields) = result.as_object_mut() {
                    fields.insert("surface_ref".into(), json!(format!("surface:{ref_id}")));
                    fields.insert("uuid".into(), json!(id));
                }
            }
            widgets
        };
        if let Some(widgets) = new_widgets {
            let surface = widgets.uuid;
            crate::browser::ui::wire_browser_tab(&state, widgets, activity.id);
            if project_surface.is_some() {
                let mut s = state.borrow_mut();
                if let Some(index) = s.workspaces.iter().position(|row| Some(row.uuid) == workspace) {
                    s.switch_to_index(index);
                    s.split_engines[index].focus_surface(&surface.to_string());
                }
                result = json!({"workspace_id":workspace,"source_workspace_id":workspace,"surface_id":surface,"status":"submitted"});
                crate::diagnostics::record("project.actions.run", json!({"trace_id":trace,"workspace_id":workspace,"source_workspace_id":workspace,"surface_id":surface,"outcome":"submitted","duration_us":started.elapsed().as_micros() as u64}));
            }
        } else {
            activity.finish("missing_surface");
            let _ = resp_tx.send(err(req_id, "not_found", "Could not create browser surface"));
            return;
        }
        state.borrow().trigger_session_save();
        activity.finish("success");
        let _ = resp_tx.send(ok(req_id, result));
    });
}

/// Deliver a browser exchange off GTK, preserving endpoint errors and cancelling when its caller leaves.
fn spawn_browser_exchange(
    runtime: &tokio::runtime::Handle,
    exchange: impl std::future::Future<Output = Result<Value, String>> + Send + 'static,
    req_id: Value,
    mut resp_tx: super::commands::RespTx,
    error_code: &'static str,
    trace_id: Option<String>,
) {
    runtime.spawn(async move {
        let started = std::time::Instant::now();
        let outcome = tokio::select! {
            biased;
            _ = resp_tx.closed() => "cancelled",
            result = exchange => {
                match result {
                    Ok(result) => { let _ = resp_tx.send(ok(req_id, result)); "success" }
                    Err(error) => { let _ = resp_tx.send(err(req_id, error_code, &error)); "error" }
                }
            }
        };
        crate::diagnostics::record(
            "browser.rpc.complete",
            json!({
                "trace_id": trace_id, "outcome": outcome,
                "duration_us": started.elapsed().as_micros(),
            }),
        );
    });
}

/// Workspace index and pane a creation request targets: the pane holding `surface`, else the
/// pane `pane` (`pane:N` or a surface UUID), else the active pane of `workspace`, of the
/// workspace holding the `caller` surface, or of the window's current workspace.
fn creation_target(
    s: &crate::app_state::AppState,
    surface: Option<&str>,
    pane: Option<&str>,
    workspace: Option<&str>,
    caller: Option<&str>,
) -> Result<(usize, u64), &'static str> {
    if let Some(reference) = surface.or(pane) {
        return s
            .split_engines
            .iter()
            .enumerate()
            .find_map(|(index, engine)| {
                let pane_id = if surface.is_some() {
                    engine.find_pane_id_by_uuid(reference)
                } else {
                    engine.pane_for_ref(reference)
                };
                pane_id.map(|pane_id| (index, pane_id))
            })
            .ok_or(if surface.is_some() {
                "surface not found"
            } else {
                "pane not found"
            });
    }
    let index = match workspace {
        Some(id) => s
            .workspaces
            .iter()
            .position(|ws| ws.uuid.to_string() == id)
            .ok_or("workspace not found")?,
        None => caller
            .and_then(|caller| {
                s.split_engines
                    .iter()
                    .position(|engine| engine.find_pane_id_by_uuid(caller).is_some())
            })
            .unwrap_or(s.active_index),
    };
    let engine = s.split_engines.get(index).ok_or("workspace not found")?;
    Ok((index, engine.active_pane()))
}

/// Ids of a created surface, with upstream's field names; `uuid` keeps this fork's original reply.
fn created_json(
    s: &crate::app_state::AppState,
    index: usize,
    pane_id: u64,
    surface: uuid::Uuid,
) -> serde_json::Value {
    use super::handles::HandleKind;
    let workspace = s.workspaces.get(index).map(|ws| ws.uuid.to_string());
    json!({
        "uuid": surface.to_string(),
        "surface_id": surface.to_string(),
        "surface_ref": s.handles.ensure_ref(HandleKind::Surface, &surface.to_string()),
        "pane_id": format!("pane:{pane_id}"),
        "pane_ref": format!("pane:{pane_id}"),
        "workspace_ref": workspace
            .as_deref()
            .map(|id| s.handles.ensure_ref(HandleKind::Workspace, id)),
        "workspace_id": workspace,
    })
}

#[cfg(test)]
mod browser_exchange_tests {
    use super::*;

    /// Shared browser delivery preserves identities, successful data and endpoint-specific failures.
    #[tokio::test]
    async fn responses_preserve_endpoint_contract() {
        for result in [
            Ok(json!({"streaming":false})),
            Err("unavailable".to_string()),
        ] {
            let success = result.is_ok();
            let (tx, rx) = tokio::sync::oneshot::channel();
            spawn_browser_exchange(
                &tokio::runtime::Handle::current(),
                async move { result },
                json!(7),
                tx,
                "stream_error",
                None,
            );
            let response = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(response["id"], 7);
            assert_eq!(response["ok"], success);
            if success {
                assert_eq!(response["result"]["streaming"], false);
            } else {
                assert_eq!(response["error"]["code"], "stream_error");
            }
        }
    }

    /// A caller that stops awaiting its response releases the exchange's owned resources.
    #[tokio::test]
    async fn abandoned_response_cancels_exchange() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (resource, dropped) = tokio::sync::oneshot::channel::<()>();
        let exchange = async move {
            let _resource = resource;
            std::future::pending::<Result<Value, String>>().await
        };
        spawn_browser_exchange(
            &tokio::runtime::Handle::current(),
            exchange,
            json!(8),
            tx,
            "browser_error",
            None,
        );
        drop(rx);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), dropped)
                .await
                .unwrap()
                .is_err()
        );
    }
}
