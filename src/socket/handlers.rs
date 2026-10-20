// src/socket/handlers.rs — GTK main thread command dispatch

use crate::socket::commands::SocketCommand;
use gtk4::prelude::*;
use serde_json::{json, Value};

use super::response::{err, err_data, ok};

/// Actions `workspace.action` accepts, in upstream's snake_case spelling (the handler
/// also takes the dashed CLI spelling). `mobile_connect` is upstream-only.
const WORKSPACE_ACTIONS: &[&str] = &[
    "pin",
    "unpin",
    "rename",
    "clear_name",
    "set_description",
    "clear_description",
    "move_up",
    "move_down",
    "move_top",
    "close_others",
    "close_above",
    "close_below",
    "mark_read",
    "mark_unread",
    "set_color",
    "clear_color",
];

/// Tab actions `tab.action` implements on Linux, after alias normalization.
const TAB_ACTIONS: &[&str] = &[
    "close_left",
    "close_right",
    "close_others",
    "mark_read",
    "mark_unread",
    "new_terminal_right",
    "move_to_new_workspace",
    "rename",
    "clear_name",
];

/// Upstream tab actions this build has no equivalent for; they fail with the support
/// note, not as unknown actions.
const TAB_ACTIONS_UNSUPPORTED: &[&str] = &[
    "pin",
    "unpin",
    "toggle_full_width_tab",
    "reload",
    "duplicate",
    "new_browser_right",
];

/// Lowercase, dash-free and reduced to the canonical action name upstream dispatches on.
fn canonical_tab_action(action: &str) -> String {
    let normalized = action.to_lowercase().replace('-', "_");
    match normalized.as_str() {
        "close_to_left" => "close_left",
        "close_to_right" => "close_right",
        "close_other_tabs" => "close_others",
        "mark_as_unread" => "mark_unread",
        "reload_tab" => "reload",
        "duplicate_tab" => "duplicate",
        "toggle_full_width" | "toggle_full_width_tab_mode" => "toggle_full_width_tab",
        "new_browser_to_right" | "new_browser_tab_to_right" => "new_browser_right",
        "new_terminal_to_right" | "new_terminal_tab_to_right" => "new_terminal_right",
        "detach_to_workspace" | "detach_to_new_workspace" => "move_to_new_workspace",
        other => other,
    }
    .to_owned()
}

/// What this build can do with a tab; both rejections quote it.
fn tab_support_note() -> String {
    format!("supported: {}", TAB_ACTIONS.join(", "))
}

/// The tab a surface-scoped call means: the named surface (and its own workspace), else
/// the selected tab of the named workspace, else the active workspace's. Errors carry the
/// code and message the socket replies with; `tab.action` and `surface.trigger_flash`
/// share this resolution.
fn resolve_tab_target(
    state: &crate::app_state::AppState,
    surface: Option<&str>,
    workspace: Option<&str>,
) -> Result<(usize, u64, String), (&'static str, String)> {
    let requested = match workspace {
        None => None,
        Some(text) => Some(
            uuid::Uuid::parse_str(text)
                .map_err(|_| ("invalid_params", "invalid workspace UUID".to_owned()))?,
        ),
    };
    let located = surface.and_then(|id| {
        state
            .split_engines
            .iter()
            .enumerate()
            .find_map(|(index, engine)| {
                engine
                    .find_pane_id_by_uuid(id)
                    .map(|pane_id| (index, pane_id))
            })
    });
    match located {
        Some((index, pane_id)) => {
            if let Some(requested) = requested {
                if state.workspaces[index].uuid != requested {
                    return Err((
                        "invalid_params",
                        "surface is not in the named workspace".to_owned(),
                    ));
                }
            }
            let id = surface.expect("a located surface was named").to_owned();
            Ok((index, pane_id, id))
        }
        None => {
            if surface.is_some() {
                return Err(("not_found", "surface not found".to_owned()));
            }
            let index = match requested {
                Some(uuid) => state.workspaces.iter().position(|row| row.uuid == uuid),
                None => Some(state.active_index),
            }
            .filter(|index| *index < state.workspaces.len());
            let Some(index) = index else {
                return Err(("not_found", "workspace not found".to_owned()));
            };
            // No tab named: the selected tab of that workspace's active pane.
            let Some(focused) = state.split_engines[index].active_pane_uuid() else {
                return Err(("not_found", "no focused tab".to_owned()));
            };
            let Some(pane_id) = state.split_engines[index].find_pane_id_by_uuid(&focused) else {
                return Err(("not_found", "no focused tab".to_owned()));
            };
            Ok((index, pane_id, focused))
        }
    }
}

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
        "pinned": workspace.pinned,
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

/// Add upstream's `workspace.create` identity (window, workspace, group and focused surface,
/// ids beside refs) to a creation reply.
fn created_identity(state: &crate::app_state::AppState, id: u64, payload: &mut Value) {
    let Some(index) = state.workspaces.iter().position(|workspace| workspace.id == id) else {
        return;
    };
    let surface = state
        .split_engines
        .get(index)
        .and_then(|engine| engine.active_pane_uuid())
        .and_then(|surface| uuid::Uuid::parse_str(&surface).ok());
    let identity = identify_location(state, index, surface)
        .or_else(|| identify_location(state, index, None));
    let (Some(identity), Some(fields)) = (identity, payload.as_object_mut())
    else {
        return;
    };
    for key in [
        "window_id",
        "window_ref",
        "workspace_id",
        "workspace_ref",
        "surface_id",
        "surface_ref",
    ] {
        fields.insert(key.into(), identity[key].clone());
    }
    fields.insert("group_id".into(), json!(state.workspaces[index].group_id));
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
    workspace: Option<&str>,
) -> Result<crate::ghostty::ffi::ghostty_surface_t, (&'static str, &'static str)> {
    let surface = {
        let state = state.borrow();
        match id {
            Some(id) => state
                .split_engines
                .iter()
                .find_map(|engine| engine.find_surface_by_uuid(id)),
            // Upstream: without a surface, the focused terminal of `workspace_id`, else of the
            // selected workspace.
            None => match workspace {
                Some(workspace) => state
                    .workspaces
                    .iter()
                    .position(|ws| ws.uuid.to_string() == workspace),
                None => Some(state.active_index),
            }
            .and_then(|index| state.split_engines.get(index))
            .and_then(|engine| engine.root.find_surface_for_pane(engine.active_pane_id)),
        }
    }
    .filter(|surface| !surface.is_null())
    .ok_or(("not_found", "live terminal surface not found"))?;
    Ok(surface)
}

/// Workspace index and surface UUID that `terminal_target` resolves for the same arguments.
fn terminal_location(
    state: &crate::app_state::AppState,
    id: Option<&str>,
    workspace: Option<&str>,
) -> Option<(usize, uuid::Uuid)> {
    let index = match (id, workspace) {
        (Some(id), _) => state
            .split_engines
            .iter()
            .position(|engine| engine.find_surface_by_uuid(id).is_some())?,
        (None, Some(workspace)) => state
            .workspaces
            .iter()
            .position(|ws| ws.uuid.to_string() == workspace)?,
        (None, None) => state.active_index,
    };
    let surface = match id {
        Some(id) => id.to_owned(),
        None => state.split_engines.get(index)?.active_pane_uuid()?,
    };
    Some((index, uuid::Uuid::parse_str(&surface).ok()?))
}

/// Split text at CR, LF or CRLF breaks into `(segment, ends_with_break)` parts.
/// Empty trailing text yields no part; empty segments before a break are kept.
fn split_at_line_breaks(text: &str) -> Vec<(&str, bool)> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(|c| c == '\r' || c == '\n') {
        let is_cr = rest.as_bytes()[at] == b'\r';
        parts.push((&rest[..at], true));
        rest = &rest[at + 1..];
        if is_cr && rest.starts_with('\n') {
            rest = &rest[1..];
        }
    }
    if !rest.is_empty() {
        parts.push((rest, false));
    }
    parts
}

/// Send UTF-8 text to a live terminal in the active workspace without changing focus.
/// Text is pasted literally, and each line break is delivered as the Enter key so that
/// commands execute as on macOS. Reject missing targets and embedded NUL bytes.
fn send_terminal_text(
    state: &crate::app_state::AppStateRef,
    id: Option<&str>,
    workspace: Option<&str>,
    text: &str,
) -> Result<(), (&'static str, &'static str)> {
    if text.contains('\0') {
        return Err(("invalid_params", "text contains a NUL byte"));
    }
    let enter = crate::ghostty::named_key::parse("enter")
        .ok_or(("not_supported", "the Enter key is unavailable"))?;
    let surface = terminal_target(state, id, workspace)?;
    for (segment, has_break) in split_at_line_breaks(text) {
        if !segment.is_empty() {
            // SAFETY: target resolution found a live native terminal on GTK and released
            // the model borrow. No event-loop iteration or teardown occurs before delivery.
            unsafe { crate::ghostty::text::send_literal(surface, segment) }
                .map_err(|message| ("invalid_params", message))?;
        }
        if has_break {
            // SAFETY: same live-surface guarantee as above; no teardown between calls.
            unsafe { crate::ghostty::named_key::send(surface, enter) };
        }
    }
    Ok(())
}

#[cfg(test)]
mod send_split_tests {
    use super::split_at_line_breaks;

    #[test]
    fn splits_at_every_kind_of_break() {
        assert_eq!(split_at_line_breaks("echo a\r"), vec![("echo a", true)]);
        assert_eq!(split_at_line_breaks("a\r\nb"), vec![("a", true), ("b", false)]);
        assert_eq!(split_at_line_breaks("plain"), vec![("plain", false)]);
        assert_eq!(split_at_line_breaks("\r\r"), vec![("", true), ("", true)]);
        assert!(split_at_line_breaks("").is_empty());
    }
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

/// Resolve a surface UUID or `surface:N` ref; callers use explicit ids unless a ref remains
/// (dispatch rewrites `surface_id`, but `tab_id` keeps its original spelling).
fn resolve_surface_identifier(
    state: &crate::app_state::AppState,
    value: &str,
) -> Option<uuid::Uuid> {
    if let Ok(id) = uuid::Uuid::parse_str(value) {
        return Some(id);
    }
    if let Some((super::handles::HandleKind::Surface, ordinal)) =
        super::handles::parse_ref(value)
    {
        return state
            .handles
            .resolve(super::handles::HandleKind::Surface, ordinal)
            .and_then(|id| uuid::Uuid::parse_str(&id).ok());
    }
    resolve_surface_ref(value, &state.browser_surface_refs)
        .ok()
        .and_then(|id| uuid::Uuid::parse_str(&id).ok())
}

/// Workspace, pane and selected surface a `browser.open_split` source names, in upstream's
/// precedence: `surface_id`/`tab_id`, then `pane_id`, then the focused surface.
fn open_split_source(
    state: &crate::app_state::AppState,
    source: Option<&str>,
    pane: Option<&str>,
    workspace: Option<&str>,
) -> Result<(usize, uuid::Uuid, u64, uuid::Uuid), (&'static str, &'static str)> {
    if let Some(reference) = source {
        let Some(surface) = resolve_surface_identifier(state, reference) else {
            return Err(("not_found", "Source surface not found"));
        };
        for (index, engine) in state.split_engines.iter().enumerate() {
            if let Some(pane_id) = engine.find_pane_id_by_uuid(&surface.to_string()) {
                return Ok((index, state.workspaces[index].uuid, pane_id, surface));
            }
        }
        return Err(("not_found", "Source surface not found"));
    }

    if let Some(reference) = pane {
        for (index, engine) in state.split_engines.iter().enumerate() {
            let pane_id = engine.pane_for_ref(reference).or_else(|| {
                resolve_surface_identifier(state, reference)
                    .and_then(|surface| engine.find_pane_id_by_uuid(&surface.to_string()))
            });
            if let Some(pane_id) = pane_id {
                let surface = engine
                    .pane_info()
                    .into_iter()
                    .find(|pane| pane.id == pane_id)
                    .and_then(|pane| pane.selected_surface)
                    .ok_or(("not_found", "Source surface not found"))?;
                return Ok((index, state.workspaces[index].uuid, pane_id, surface));
            }
        }
        return Err(("not_found", "Source surface not found"));
    }

    let index = match workspace {
        Some(workspace) => state
            .workspaces
            .iter()
            .position(|candidate| candidate.uuid.to_string() == workspace)
            .ok_or(("not_found", "Source surface not found"))?,
        None => state.active_index,
    };
    let engine = state
        .split_engines
        .get(index)
        .ok_or(("not_found", "No focused surface to split"))?;
    let surface = engine
        .active_pane_uuid()
        .and_then(|surface| uuid::Uuid::parse_str(&surface).ok())
        .ok_or(("not_found", "No focused surface to split"))?;
    let pane_id = engine
        .find_pane_id_by_uuid(&surface.to_string())
        .ok_or(("not_found", "No focused surface to split"))?;
    Ok((index, state.workspaces[index].uuid, pane_id, surface))
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
                        state.render_metadata(index);
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
                "right_sidebar.apply",
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
                "workspace.action",
                "tab.action",
                "surface.action",
                "workspace.group.list",
                "workspace.group.create",
                "workspace.group.update",
                "workspace.group.assign",
                "workspace.group.delete",
                "surface.list",
                "surface.current",
                "surface.split",
                "surface.create",
                "surface.focus",
                "surface.close",
                "surface.move",
                "surface.reorder",
                "surface.drag_to_split",
                "surface.split_off",
                "surface.send_text",
                "surface.send_key",
                "surface.read_text",
                "surface.read_scrollback",
                "surface.resume.set",
                "surface.resume.show",
                "surface.resume.clear",
                "surface.health",
                "surface.trigger_flash",
                "surface.refresh",
                "pane.list",
                "pane.surfaces",
                "pane.create",
                "pane.focus",
                "pane.swap",
                "pane.break",
                "pane.join",
                "pane.last",
                "config.reload",
                "settings.open",
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
                "browser.open_split",
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
                "debug.app.activate",
                "debug.shortcut.set",
                "debug.terminal.render_stats",
                "debug.panel_snapshot",
                "debug.panel_snapshot.reset",
                "debug.shortcut.simulate",
                "debug.terminal.is_focused",
                "window.focus",
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
            // Upstream shape: window and workspace ids beside refs, the summary under `workspace`;
            // the record's own fields stay at the top level for existing clients.
            let response = match (
                workspace_record(&s, s.active_index),
                identify_location(&s, s.active_index, None),
            ) {
                (Some(workspace), Some(mut payload)) => {
                    if let (Some(fields), Some(record)) =
                        (payload.as_object_mut(), workspace.as_object())
                    {
                        for (key, value) in record {
                            fields.entry(key.clone()).or_insert(value.clone());
                        }
                        fields.insert("workspace".into(), workspace.clone());
                    }
                    ok(req_id, payload)
                }
                _ => err(req_id, "not_found", "No workspace selected"),
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
            focus,
            resp_tx,
        } => {
            let previous = {
                let s = state.borrow();
                s.workspaces.get(s.active_index).map(|ws| ws.uuid)
            };
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
                let mut payload = json!({"uuid": uuid_str, "remote": true,
                    "terminal_transport":terminal_transport,"terminal_profile":terminal_profile});
                created_identity(&state.borrow(), id, &mut payload);
                let _ = resp_tx.send(ok(req_id, payload));
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
                let mut payload = json!({
                    "uuid": uuid_str,
                    "working_directory": directory,
                });
                created_identity(&s, id, &mut payload);
                let _ = resp_tx.send(ok(req_id, payload));
            }
            // Background creation: the new terminal exists, but the previous workspace stays selected.
            if !focus {
                let mut s = state.borrow_mut();
                if let Some(index) = previous
                    .and_then(|previous| s.workspaces.iter().position(|ws| ws.uuid == previous))
                {
                    s.switch_to_index(index);
                }
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
            before,
            after,
            dry_run,
            resp_tx,
        } => {
            // SOCK-05: No focus side effects.
            let mut s = state.borrow_mut();
            let Some(from) = s.workspaces.iter().position(|ws| ws.uuid.to_string() == id) else {
                let _ = resp_tx.send(err(req_id, "not_found", "workspace not found"));
                return;
            };
            let to = match position {
                Some(position) => position.min(s.workspaces.len().saturating_sub(1)),
                // Relative placement: the anchor's slot, corrected for the removal itself.
                None => {
                    let reference = before
                        .as_deref()
                        .or(after.as_deref())
                        .expect("reorder dispatch requires an index or an anchor");
                    let Ok(anchor) = uuid::Uuid::parse_str(reference) else {
                        let _ =
                            resp_tx.send(err(req_id, "invalid_params", "invalid workspace UUID"));
                        return;
                    };
                    if anchor.to_string() == id {
                        let _ = resp_tx.send(err(
                            req_id,
                            "invalid_params",
                            "before/after cannot name the workspace being reordered",
                        ));
                        return;
                    }
                    let Some(anchor_index) = s.workspaces.iter().position(|ws| ws.uuid == anchor)
                    else {
                        let _ = resp_tx.send(err(
                            req_id,
                            "not_found",
                            "before/after workspace not found",
                        ));
                        return;
                    };
                    let shifted = anchor_index - usize::from(from < anchor_index);
                    if before.is_some() {
                        shifted
                    } else {
                        shifted + 1
                    }
                }
            };
            // The reply carries the resolved slot: `--dry-run` reports it and stops here.
            let workspace_ref = s
                .handles
                .ensure_ref(super::handles::HandleKind::Workspace, &id);
            let window_ref = s.handles.ensure_ref(
                super::handles::HandleKind::Window,
                super::handles::MAIN_WINDOW_ID,
            );
            let mut reply = json!({
                "workspace_id": id,
                "workspace_ref": workspace_ref,
                "window_id": super::handles::MAIN_WINDOW_ID,
                "window_ref": window_ref,
                "from_index": from,
                "index": to,
                "to_index": to,
            });
            if dry_run {
                reply["dry_run"] = json!(true);
                drop(s);
                let _ = resp_tx.send(ok(req_id, reply));
                return;
            }
            s.reorder_workspace(from, to);
            drop(s);
            crate::sidebar::rebuild_grouped_sidebar(state);
            let _ = resp_tx.send(ok(req_id, reply));
        }
        SocketCommand::WorkspaceAction {
            req_id,
            action,
            workspace,
            title,
            color,
            description,
            resp_tx,
        } => {
            // Upstream lowercases the action and maps dashes onto underscores.
            let action = action.to_lowercase().replace('-', "_");
            if !WORKSPACE_ACTIONS.contains(&action.as_str()) {
                let _ = resp_tx.send(err(
                    req_id,
                    "invalid_params",
                    &format!(
                        "Unknown workspace action: {action} (valid actions: {})",
                        WORKSPACE_ACTIONS.join(", ")
                    ),
                ));
                return;
            }
            let mut s = state.borrow_mut();
            // No target means the active workspace, as upstream.
            let index = match workspace.as_deref() {
                None => Some(s.active_index),
                Some(text) => uuid::Uuid::parse_str(text)
                    .ok()
                    .and_then(|uuid| s.workspaces.iter().position(|row| row.uuid == uuid)),
            };
            let Some(index) = index.filter(|index| *index < s.workspaces.len()) else {
                let _ = resp_tx.send(err(req_id, "not_found", "workspace not found"));
                return;
            };
            let target = s.workspaces[index].uuid;
            let mut extras = json!({});
            let mut order_changed = false;
            match action.as_str() {
                "pin" | "unpin" => {
                    let pinned = action == "pin";
                    if let Some(moved) = s.set_workspace_pinned(index, pinned) {
                        extras["index"] = json!(moved);
                    }
                    extras["pinned"] = json!(pinned);
                    order_changed = true;
                }
                "rename" => {
                    let Some(title) = title
                        .as_deref()
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                    else {
                        let _ =
                            resp_tx.send(err(req_id, "invalid_params", "Missing or invalid title"));
                        return;
                    };
                    let title = title.to_owned();
                    s.rename_workspace_at(index, title.clone());
                    extras["title"] = json!(title);
                }
                "clear_name" => {
                    s.clear_workspace_name(index);
                    extras["title"] = json!(s.workspaces[index].name);
                }
                "set_description" => {
                    let Some(description) = description
                        .as_deref()
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                    else {
                        let _ = resp_tx.send(err(
                            req_id,
                            "invalid_params",
                            "Missing or invalid description",
                        ));
                        return;
                    };
                    s.set_workspace_description(index, Some(description.to_owned()));
                    extras["description"] = json!(description);
                }
                "clear_description" => {
                    s.set_workspace_description(index, None);
                    extras["description"] = Value::Null;
                }
                "move_up" | "move_down" | "move_top" => {
                    let delta = match action.as_str() {
                        "move_up" => -1,
                        "move_down" => 1,
                        _ => 0,
                    };
                    let destination = s.workspace_move_target(index, action == "move_top", delta);
                    s.reorder_workspace(index, destination);
                    let final_index = s
                        .workspaces
                        .iter()
                        .position(|row| row.uuid == target)
                        .unwrap_or(destination);
                    extras["index"] = json!(final_index);
                    order_changed = true;
                }
                "close_others" | "close_above" | "close_below" => {
                    // Pinned workspaces survive every close range, as upstream.
                    let candidates: Vec<usize> = (0..s.workspaces.len())
                        .filter(|candidate| {
                            *candidate != index
                                && !s.workspaces[*candidate].pinned
                                && match action.as_str() {
                                    "close_above" => *candidate < index,
                                    "close_below" => *candidate > index,
                                    _ => true,
                                }
                        })
                        .collect();
                    // Close from the end so the surviving candidates keep their indexes.
                    let mut closed = 0;
                    for candidate in candidates.into_iter().rev() {
                        if s.close_workspace(candidate) {
                            closed += 1;
                        }
                    }
                    extras["closed"] = json!(closed);
                }
                "mark_read" => {
                    let scope = crate::inbox::Scope {
                        workspace_id: Some(target),
                        surface_id: None,
                    };
                    let _ = crate::inbox_actions::handle(
                        &mut s,
                        crate::inbox::Action::MarkRead {
                            id: None,
                            scope,
                            all: false,
                        },
                    );
                    crate::inbox_actions::refresh(&s);
                    s.clear_workspace_attention(index);
                    s.trigger_session_save();
                }
                "mark_unread" => {
                    crate::inbox_actions::mark_unread_where(&mut s, target, None);
                    // Light the dot even when the workspace had nothing to read.
                    s.workspaces[index].has_attention = true;
                    s.update_sidebar_attention(index);
                }
                "set_color" => {
                    let Some(hex) = color
                        .as_deref()
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                    else {
                        let _ =
                            resp_tx.send(err(req_id, "invalid_params", "Missing or invalid color"));
                        return;
                    };
                    if !crate::workspace::valid_workspace_color(hex) {
                        let _ = resp_tx.send(err(
                            req_id,
                            "invalid_params",
                            "Invalid color. Use a hex value (#RRGGBB).",
                        ));
                        return;
                    }
                    let workspace_id = s.workspaces[index].id;
                    let hex = hex.to_owned();
                    s.set_workspace_color(workspace_id, Some(hex.clone()));
                    extras["color"] = json!(hex);
                }
                "clear_color" => {
                    let workspace_id = s.workspaces[index].id;
                    s.set_workspace_color(workspace_id, None);
                    extras["color"] = Value::Null;
                }
                _ => unreachable!("the action list is validated above"),
            }
            drop(s);
            if order_changed {
                crate::sidebar::rebuild_grouped_sidebar(state);
            }
            let s = state.borrow();
            let workspace_ref = s
                .handles
                .ensure_ref(super::handles::HandleKind::Workspace, &target.to_string());
            let window_ref = s.handles.ensure_ref(
                super::handles::HandleKind::Window,
                super::handles::MAIN_WINDOW_ID,
            );
            drop(s);
            let mut reply = json!({
                "action": action,
                "workspace_id": target,
                "workspace_ref": workspace_ref,
                "window_id": super::handles::MAIN_WINDOW_ID,
                "window_ref": window_ref,
            });
            if let (Some(reply), Some(extras)) = (reply.as_object_mut(), extras.as_object()) {
                for (key, value) in extras {
                    reply.insert(key.clone(), value.clone());
                }
            }
            let _ = resp_tx.send(ok(req_id, reply));
        }
        SocketCommand::TabAction {
            req_id,
            action,
            surface,
            workspace,
            title,
            focus,
            resp_tx,
        } => {
            // Upstream lowercases the action, maps dashes onto underscores, then aliases.
            let echoed = action.to_lowercase().replace('-', "_");
            let action = canonical_tab_action(&echoed);
            if !TAB_ACTIONS.contains(&action.as_str()) {
                let message = if TAB_ACTIONS_UNSUPPORTED.contains(&action.as_str()) {
                    format!(
                        "tab action {action} is not supported on Linux ({})",
                        tab_support_note()
                    )
                } else {
                    format!("Unknown tab action: {echoed} ({})", tab_support_note())
                };
                let _ = resp_tx.send(err(req_id, "invalid_params", &message));
                return;
            }
            let mut s = state.borrow_mut();
            let (index, pane_id, surface_uuid) =
                match resolve_tab_target(&s, surface.as_deref(), workspace.as_deref()) {
                    Ok(target) => target,
                    Err((code, message)) => {
                        let _ = resp_tx.send(err(req_id, code, &message));
                        return;
                    }
                };
            let surface_id: uuid::Uuid = match surface_uuid.parse() {
                Ok(uuid) => uuid,
                Err(_) => {
                    let _ = resp_tx.send(err(req_id, "invalid_request", "invalid surface UUID"));
                    return;
                }
            };
            let workspace_uuid = s.workspaces[index].uuid;
            let mut extras = json!({});
            match action.as_str() {
                // Every close stays inside the anchor's pane and never closes the anchor.
                "close_left" | "close_right" | "close_others" => {
                    let Some((_, slot)) = s.split_engines[index].surface_location(&surface_uuid)
                    else {
                        let _ = resp_tx.send(err(req_id, "not_found", "surface not found"));
                        return;
                    };
                    let Some(pane) = s.split_engines[index]
                        .pane_info()
                        .into_iter()
                        .find(|pane| pane.id == pane_id)
                    else {
                        let _ = resp_tx.send(err(req_id, "not_found", "pane not found"));
                        return;
                    };
                    let ids = pane.surface_ids;
                    let targets: Vec<uuid::Uuid> = match action.as_str() {
                        "close_left" => ids[..slot].to_vec(),
                        "close_right" => ids[(slot + 1).min(ids.len())..].to_vec(),
                        _ => {
                            let mut others = ids;
                            others.remove(slot);
                            others
                        }
                    };
                    let mut closed = 0;
                    for target in targets {
                        if matches!(
                            s.split_engines[index].close_surface_and_empty_pane(target),
                            crate::split_engine::CloseSurfaceResult::Closed
                        ) {
                            closed += 1;
                        }
                    }
                    crate::inbox_actions::purge_closed_surfaces(&mut s);
                    s.trigger_session_save();
                    extras["closed"] = json!(closed);
                }
                // Upstream `setPanelCustomTitle`: a trimmed, non-empty title, echoed back.
                "rename" | "clear_name" => {
                    let title = match action.as_str() {
                        "rename" => match title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                            Some(title) => Some(title.to_owned()),
                            None => {
                                let _ = resp_tx.send(err(req_id, "invalid_params", "Missing or invalid title"));
                                return;
                            }
                        },
                        _ => None,
                    };
                    if !s.split_engines[index].set_custom_title(&surface_uuid, title.as_deref()) {
                        let _ = resp_tx.send(err(
                            req_id,
                            "invalid_state",
                            "Only terminal tabs can be renamed on Linux",
                        ));
                        return;
                    }
                    if let Some(title) = title {
                        extras["title"] = json!(title);
                    }
                }
                "mark_read" => {
                    let scope = crate::inbox::Scope {
                        workspace_id: Some(workspace_uuid),
                        surface_id: Some(surface_id),
                    };
                    let _ = crate::inbox_actions::handle(
                        &mut s,
                        crate::inbox::Action::MarkRead {
                            id: None,
                            scope,
                            all: false,
                        },
                    );
                    crate::inbox_actions::refresh(&s);
                    s.split_engines[index].root.set_attention(pane_id, false);
                    s.workspaces[index].has_attention = s.split_engines[index].root.any_attention();
                    s.update_sidebar_attention(index);
                    s.trigger_session_save();
                }
                "mark_unread" => {
                    crate::inbox_actions::mark_unread_where(
                        &mut s,
                        workspace_uuid,
                        Some(surface_id),
                    );
                    s.split_engines[index].root.set_attention(pane_id, true);
                    s.workspaces[index].has_attention = s.split_engines[index].root.any_attention();
                    s.update_sidebar_attention(index);
                }
                "new_terminal_right" => {
                    let Some((_, slot)) = s.split_engines[index].surface_location(&surface_uuid)
                    else {
                        let _ = resp_tx.send(err(req_id, "not_found", "surface not found"));
                        return;
                    };
                    if focus && index != s.active_index {
                        s.switch_to_index(index);
                    }
                    // The new shell inherits the pane's terminal context (same CWD fallback).
                    let launch = crate::split_engine::TerminalLaunch {
                        initial_input: None,
                        working_directory: None,
                    };
                    let created =
                        match s.split_engines[index].new_terminal_surface(pane_id, launch, focus) {
                            Ok(created) => created,
                            Err(message) => {
                                let _ = resp_tx.send(err(req_id, "create_failed", message));
                                return;
                            }
                        };
                    let _ = s.split_engines[index].reorder_surface(created, slot + 1);
                    extras["created_surface_id"] = json!(created);
                    extras["created_surface_ref"] = json!(s
                        .handles
                        .ensure_ref(super::handles::HandleKind::Surface, &created.to_string()));
                    s.trigger_session_save();
                }
                "move_to_new_workspace" => {
                    // Upstream refuses to detach the workspace's only tab.
                    let tabs: usize = s.split_engines[index]
                        .pane_info()
                        .iter()
                        .map(|pane| pane.surface_ids.len())
                        .sum();
                    if tabs <= 1 {
                        let _ = resp_tx.send(err(
                            req_id,
                            "invalid_state",
                            "Tab cannot be moved to a new workspace because it is the only tab in its workspace",
                        ));
                        return;
                    }
                    let source_workspace = workspace_uuid;
                    let previous = s.workspaces.get(s.active_index).map(|row| row.uuid);
                    // Creation selects the new workspace; `focus` decides whether it stays.
                    let created_id = s.create_workspace_with_input(None, None, None);
                    let Some(new_index) = s.workspaces.iter().position(|row| row.id == created_id)
                    else {
                        let _ = resp_tx.send(err(
                            req_id,
                            "internal_error",
                            "Failed to move tab to new workspace",
                        ));
                        return;
                    };
                    if let Some(title) = title
                        .as_deref()
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                    {
                        s.rename_workspace_at(new_index, title.to_owned());
                    }
                    let new_workspace = s.workspaces[new_index].uuid;
                    let moved = s.move_surface_between_workspaces(
                        surface_id,
                        new_workspace,
                        None,
                        None,
                        focus,
                    );
                    let result = match moved {
                        Ok((result, _)) => result,
                        Err(message) => {
                            // The workspace never received its tab: drop it again.
                            s.close_workspace(new_index);
                            let _ = resp_tx.send(err(req_id, "internal_error", message));
                            return;
                        }
                    };
                    // The new workspace was born with a starter terminal; the moved tab is
                    // the point of the command, so the starter goes.
                    let starter = s.split_engines[new_index]
                        .pane_info()
                        .into_iter()
                        .flat_map(|pane| pane.surface_ids)
                        .find(|uuid| *uuid != surface_id);
                    if let Some(starter) = starter {
                        let _ = s.split_engines[new_index].close_surface_and_empty_pane(starter);
                    }
                    if !focus {
                        if let Some(back) = previous
                            .and_then(|uuid| s.workspaces.iter().position(|row| row.uuid == uuid))
                        {
                            s.switch_to_index(back);
                        }
                    }
                    s.trigger_session_save();
                    let destination_ref = s.handles.ensure_ref(
                        super::handles::HandleKind::Workspace,
                        &new_workspace.to_string(),
                    );
                    let source_ref = s.handles.ensure_ref(
                        super::handles::HandleKind::Workspace,
                        &source_workspace.to_string(),
                    );
                    let moved_ref = s
                        .handles
                        .ensure_ref(super::handles::HandleKind::Surface, &surface_uuid);
                    extras["source_workspace_id"] = json!(source_workspace);
                    extras["source_workspace_ref"] = json!(source_ref);
                    // Upstream reports the destination as `workspace` / `created_workspace`.
                    extras["workspace_id"] = json!(new_workspace);
                    extras["workspace_ref"] = json!(destination_ref.clone());
                    extras["created_workspace_id"] = json!(new_workspace);
                    extras["created_workspace_ref"] = json!(destination_ref);
                    extras["pane_id"] = json!(result.pane_id);
                    extras["pane_ref"] = json!(format!("pane:{}", result.pane_id));
                    extras["tab_id"] = json!(surface_uuid.clone());
                    extras["tab_ref"] = json!(moved_ref);
                }
                _ => unreachable!("the action list is validated above"),
            }
            let surface_ref = s
                .handles
                .ensure_ref(super::handles::HandleKind::Surface, &surface_uuid);
            let workspace_ref = s.handles.ensure_ref(
                super::handles::HandleKind::Workspace,
                &workspace_uuid.to_string(),
            );
            let window_ref = s.handles.ensure_ref(
                super::handles::HandleKind::Window,
                super::handles::MAIN_WINDOW_ID,
            );
            drop(s);
            let mut reply = json!({
                "action": echoed,
                "surface_id": surface_uuid,
                "surface_ref": surface_ref,
                "workspace_id": workspace_uuid,
                "workspace_ref": workspace_ref,
                "window_id": super::handles::MAIN_WINDOW_ID,
                "window_ref": window_ref,
            });
            if let (Some(reply), Some(extras)) = (reply.as_object_mut(), extras.as_object()) {
                for (key, value) in extras {
                    reply.insert(key.clone(), value.clone());
                }
            }
            let _ = resp_tx.send(ok(req_id, reply));
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

        SocketCommand::SettingsOpen { req_id, resp_tx } => {
            use gtk4::prelude::*;
            let opened = gtk4::gio::Application::default()
                .and_then(|a| a.downcast::<gtk4::Application>().ok())
                .and_then(|app| app.active_window())
                .is_some_and(|window| window.activate_action("win.preferences", None).is_ok());
            let _ = resp_tx.send(ok(req_id, json!({"opened": opened})));
        }

        // -- config.* --
        SocketCommand::ConfigReload { req_id, resp_tx } => {
            let reloaded = crate::config::reload_shortcuts();
            let _ = resp_tx.send(ok(req_id, json!({"reloaded": reloaded})));
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
        // Upstream `window.focus`: one window here, named by id or ref.
        SocketCommand::WindowFocus {
            req_id,
            window,
            resp_tx,
        } => {
            use super::handles::{HandleKind, MAIN_WINDOW_ID};
            let s = state.borrow();
            let main_ref = s.handles.ensure_ref(HandleKind::Window, MAIN_WINDOW_ID);
            let identity = json!({"window_id": MAIN_WINDOW_ID, "window_ref": main_ref});
            if window == MAIN_WINDOW_ID || window == main_ref {
                if let Some(window) = s.gtk_app.active_window() {
                    window.present();
                }
                let _ = resp_tx.send(ok(req_id, identity));
            } else {
                let _ = resp_tx.send(err(req_id, "not_found", "Window not found"));
            }
        }

        // Upstream `debug.app.activate`: bring the main window forward, empty result.
        SocketCommand::DebugAppActivate { req_id, resp_tx } => {
            if let Some(window) = state.borrow().gtk_app.active_window() {
                window.present();
            }
            let _ = resp_tx.send(ok(req_id, json!({})));
        }

        // Upstream `isTerminalFocused`: only the selected workspace counts; an unknown surface
        // is `false`, not an error. GTK's focus widget of the window plays the first responder.
        SocketCommand::DebugTerminalIsFocused {
            req_id,
            surface_id,
            resp_tx,
        } => {
            let area = state
                .borrow()
                .active_split_engine()
                .and_then(|engine| engine.gl_area_for_surface(&surface_id));
            let focused = area.is_some_and(|area| area.is_focus());
            let _ = resp_tx.send(ok(req_id, json!({"focused": focused})));
        }

        // Upstream `renderStats`: frame counters of one terminal (`presentCount` = GL frames
        // drawn here) plus window state. Fields upstream reads from AppKit layers are omitted.
        SocketCommand::DebugRenderStats {
            req_id,
            surface_id,
            resp_tx,
        } => {
            let area = {
                let s = state.borrow();
                let id = surface_id.or_else(|| {
                    s.split_engines.get(s.active_index).and_then(|engine| engine.active_pane_uuid())
                });
                id.and_then(|id| {
                    s.split_engines.iter().find_map(|engine| engine.gl_area_for_surface(&id))
                })
            };
            let response = match area {
                None => err(req_id, "internal_error", "ERROR: Terminal surface not found"),
                Some(area) => {
                    let count = crate::socket::render_debug::present_count(&area);
                    let app_active = area
                        .root()
                        .and_then(|root| root.downcast::<gtk4::Window>().ok())
                        .is_some_and(|window| window.is_active());
                    ok(
                        req_id,
                        json!({"stats": {
                            "presentCount": count,
                            "drawCount": count,
                            "inWindow": area.is_mapped(),
                            "appIsActive": app_active,
                            "isFirstResponder": area.has_focus(),
                        }}),
                    )
                }
            };
            let _ = resp_tx.send(response);
        }

        // Upstream `panelSnapshot`: pixels changed since the previous snapshot, PNG kept.
        SocketCommand::DebugPanelSnapshot {
            req_id,
            surface_id,
            label,
            resp_tx,
        } => {
            let area = {
                let s = state.borrow();
                s.split_engines
                    .get(s.active_index)
                    .and_then(|engine| engine.gl_area_for_surface(&surface_id))
            };
            let Some(area) = area else {
                let _ = resp_tx.send(err(req_id, "internal_error", "ERROR: Terminal surface not found"));
                return;
            };
            let id = surface_id.clone();
            crate::socket::render_debug::panel_snapshot_when_painted(&area, surface_id, label, move |captured| {
                let response = match captured {
                    Ok((changed, width, height, path)) => ok(
                        req_id,
                        json!({
                            "surface_id": id,
                            "changed_pixels": changed,
                            "width": width,
                            "height": height,
                            "path": path,
                        }),
                    ),
                    Err(message) => err(req_id, "internal_error", &message),
                };
                let _ = resp_tx.send(response);
            });
        }

        SocketCommand::DebugPanelSnapshotReset {
            req_id,
            surface_id,
            resp_tx,
        } => {
            crate::socket::render_debug::panel_snapshot_reset(&surface_id);
            let _ = resp_tx.send(ok(req_id, json!({})));
        }

        // Upstream `setShortcut` (TerminalController+ShortcutConfiguration.swift): same names,
        // `clear`/`unbound`/`none` and `default`/`reset`, same error texts. Bound for this run
        // only; names whose action the port lacks are refused rather than silently accepted.
        SocketCommand::DebugShortcutSet {
            req_id,
            name,
            combo,
            resp_tx,
        } => {
            use crate::config::ShortcutAction as A;
            let action = match name.to_lowercase().as_str() {
                "focus_left" | "focusleft" => Ok(A::FocusLeft),
                "focus_right" | "focusright" => Ok(A::FocusRight),
                "focus_up" | "focusup" => Ok(A::FocusUp),
                "focus_down" | "focusdown" => Ok(A::FocusDown),
                "split_right" | "splitright" => Ok(A::SplitRight),
                "split_down" | "splitdown" => Ok(A::SplitDown),
                known @ ("resize_pane_left" | "resizepaneleft" | "resize-pane-left"
                | "resize_pane_right" | "resizepaneright" | "resize-pane-right"
                | "resize_pane_up" | "resizepaneup" | "resize-pane-up"
                | "resize_pane_down" | "resizepanedown" | "resize-pane-down"
                | "workspace_digits" | "workspace_number" | "select_workspace_by_number"
                | "surface_digits" | "surface_number" | "select_surface_by_number"
                | "focus_sidebar" | "focussidebar"
                | "pane_digits" | "pane_number" | "focus_pane_by_number") => Err(format!(
                    "ERROR: Shortcut {known} has no action on Linux yet"
                )),
                _ => Err("ERROR: Unknown shortcut name. Supported: focus_left, focus_right, focus_up, focus_down, split_right, split_down, resize_pane_left, resize_pane_right, resize_pane_up, resize_pane_down, workspace_digits, surface_digits, focus_sidebar, pane_digits".to_owned()),
            };
            let binding = action.and_then(|action| {
                let binding = match combo.to_lowercase().as_str() {
                    "clear" | "unbound" | "none" => None,
                    "default" | "reset" => crate::config::default_binding(action),
                    _ => Some(
                        crate::config::parse_combo(&combo)
                            .ok_or_else(|| "ERROR: Invalid combo. Example: cmd+ctrl+h".to_owned())?,
                    ),
                };
                Ok((action, binding))
            });
            let response = match binding {
                Ok((action, binding)) if crate::config::set_live_binding(action, binding) => {
                    ok(req_id, json!({}))
                }
                Ok(_) => err(req_id, "internal_error", "ERROR: shortcut map not ready"),
                Err(message) => err(req_id, "internal_error", &message),
            };
            let _ = resp_tx.send(response);
        }

        // Upstream `simulateShortcut`: the app's shortcut matcher first; an unmatched combo
        // goes on as a key press to the focused terminal. Either way the answer is `{}`.
        SocketCommand::DebugShortcutSimulate {
            req_id,
            combo,
            resp_tx,
        } => {
            let response = match crate::shortcuts::simulate_shortcut(&combo) {
                Err(()) => err(
                    req_id,
                    "internal_error",
                    "ERROR: Invalid combo. Example: cmd+ctrl+h",
                ),
                Ok(true) => ok(req_id, json!({})),
                Ok(false) => {
                    let key = combo.to_ascii_lowercase().replace("cmd", "ctrl").replace('+', "-");
                    let mut characters = key.chars();
                    if let Ok(surface) = terminal_target(state, None, None) {
                        // SAFETY: resolution releases the model borrow and returns a
                        // live GTK-owned terminal; no teardown occurs before input.
                        unsafe {
                            if let Some(named) = crate::ghostty::named_key::parse(&key) {
                                crate::ghostty::named_key::send(surface, named);
                            } else if let (Some(character), None) =
                                (characters.next(), characters.next())
                            {
                                let _ = crate::ghostty::text::send_character(surface, character);
                            }
                        }
                    }
                    ok(req_id, json!({}))
                }
            };
            let _ = resp_tx.send(response);
        }

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
                            // Upstream `insertText` is typed input, not a paste: a newline
                            // submits the line instead of landing inside bracketed paste.
                            for character in text.chars() {
                                // SAFETY: the surface comes from the live tree on the GTK
                                // thread and nothing tears it down during this loop.
                                let _ = unsafe { crate::ghostty::text::send_character(surface, character) };
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
            all_workspaces,
            resp_tx,
        } => {
            // SOCK-05: No focus side effects. `index` counts within each workspace, as on macOS.
            let s = state.borrow();
            // A named workspace restricts the reply to it; otherwise only the selected
            // workspace is listed unless the caller explicitly asks for all of them.
            let indices: Vec<usize> = match workspace {
                Some(workspace) => match s.workspaces.iter().position(|ws| ws.uuid == workspace) {
                    Some(index) => vec![index],
                    None => {
                        let _ =
                            resp_tx.send(err(req_id, "not_found", "Workspace not found"));
                        return;
                    }
                },
                None if all_workspaces => (0..s.workspaces.len()).collect(),
                None => Some(s.active_index)
                    .filter(|index| *index < s.workspaces.len())
                    .into_iter()
                    .collect(),
            };
            let mut surfaces: Vec<Value> = Vec::new();
            for &ws_idx in &indices {
                let ws = &s.workspaces[ws_idx];
                let Some(engine) = s.split_engines.get(ws_idx) else {
                    continue;
                };
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
                            "focused": selected && pane.id == engine.active_pane_id,
                            "active": selected && pane.id == engine.active_pane_id && ws_idx == s.active_index,
                        }));
                        index += 1;
                    }
                }
            }
            let mut response = json!({"surfaces": surfaces});
            if let [index] = indices.as_slice() {
                use super::handles::{HandleKind, MAIN_WINDOW_ID};
                let workspace = s.workspaces[*index].uuid.to_string();
                if let Some(fields) = response.as_object_mut() {
                    fields.insert("workspace_id".into(), json!(workspace));
                    fields.insert(
                        "workspace_ref".into(),
                        json!(s.handles.ensure_ref(HandleKind::Workspace, &workspace)),
                    );
                    fields.insert("window_id".into(), json!(MAIN_WINDOW_ID));
                    fields.insert(
                        "window_ref".into(),
                        json!(s.handles.ensure_ref(HandleKind::Window, MAIN_WINDOW_ID)),
                    );
                }
            }
            let _ = resp_tx.send(ok(req_id, response));
        }

        SocketCommand::SurfaceCurrent {
            req_id,
            workspace,
            resp_tx,
        } => {
            // SOCK-05: read-only; keeps the selected workspace and focus unchanged.
            let s = state.borrow();
            let index = match workspace {
                Some(workspace) => s.workspaces.iter().position(|ws| ws.uuid == workspace),
                None => Some(s.active_index).filter(|index| *index < s.workspaces.len()),
            };
            let response = match index {
                Some(index) => {
                    let surface = s
                        .split_engines
                        .get(index)
                        .and_then(|engine| engine.active_pane_uuid())
                        .and_then(|surface| uuid::Uuid::parse_str(&surface).ok());
                    match identify_location(&s, index, surface)
                        .or_else(|| identify_location(&s, index, None))
                    {
                        Some(identity) => ok(
                            req_id,
                            json!({
                                "window_id": identity["window_id"],
                                "window_ref": identity["window_ref"],
                                "workspace_id": identity["workspace_id"],
                                "workspace_ref": identity["workspace_ref"],
                                "pane_id": identity["pane_id"],
                                "pane_ref": identity["pane_ref"],
                                "surface_id": identity["surface_id"],
                                "surface_ref": identity["surface_ref"],
                                "surface_type": identity["surface_type"],
                            }),
                        ),
                        None => err(req_id, "not_found", "Workspace not found"),
                    }
                }
                None => err(req_id, "not_found", "Workspace not found"),
            };
            let _ = resp_tx.send(response);
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
                    // Upstream shows the new tab in its pane even without focus
                    // (`preserveBonsplitSelectionWhenUnfocused` defaults to false).
                    if !focus {
                        engine.select_surface(surface);
                    }
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
                    crate::inbox_actions::purge_closed_surfaces(&mut state.borrow_mut());
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
            before,
            after,
            focus,
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
                // Relative placement stays inside the pane: the anchor's slot, corrected for
                // the removal of the tab being reordered.
                let position = match position {
                    Some(position) => position,
                    None => {
                        let reference = before
                            .as_deref()
                            .or(after.as_deref())
                            .expect("reorder dispatch requires an index or an anchor");
                        let Ok(anchor) = uuid::Uuid::parse_str(reference) else {
                            let _ =
                                resp_tx.send(err(req_id, "invalid_params", "invalid surface UUID"));
                            return;
                        };
                        if anchor == uuid {
                            let _ = resp_tx.send(err(
                                req_id,
                                "invalid_params",
                                "before/after cannot name the surface being reordered",
                            ));
                            return;
                        }
                        let text = anchor.to_string();
                        let found = s.split_engines.iter().enumerate().find_map(
                            |(engine_index, engine)| {
                                engine
                                    .surface_location(&text)
                                    .map(|(pane, slot)| (engine_index, pane, slot))
                            },
                        );
                        let Some((engine_index, anchor_pane, anchor_slot)) = found else {
                            let _ = resp_tx.send(err(
                                req_id,
                                "not_found",
                                "before/after surface not found",
                            ));
                            return;
                        };
                        let Some((source_pane, source_slot)) =
                            s.split_engines[index].surface_location(&id)
                        else {
                            let _ = resp_tx.send(err(req_id, "not_found", "surface not found"));
                            return;
                        };
                        if engine_index != index || anchor_pane != source_pane {
                            let _ = resp_tx.send(err(
                                req_id,
                                "invalid_params",
                                "before/after must name a surface of the same pane",
                            ));
                            return;
                        }
                        let shifted = anchor_slot - usize::from(source_slot < anchor_slot);
                        if before.is_some() {
                            shifted
                        } else {
                            shifted + 1
                        }
                    }
                };
                match s.split_engines[index].reorder_surface(uuid, position) {
                    Ok(result) => {
                        // `focus` selects the tab it reordered and shows its workspace,
                        // the same pairing `surface.move` uses for a focused move.
                        if focus {
                            s.split_engines[index].focus_surface(&id);
                            s.switch_to_index(index);
                        }
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

        SocketCommand::SurfaceSplitOff {
            req_id,
            id,
            direction,
            focus,
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
                // The anchor is the surface's own pane: the tab becomes a sibling of it.
                let Some((own_pane, _)) = s.split_engines[index].surface_location(&id) else {
                    let _ = resp_tx.send(err(req_id, "not_found", "surface not found"));
                    return;
                };
                let previous_pane = s.split_engines[index].active_pane_id;
                match s.split_engines[index].drag_surface_to_split(uuid, own_pane, direction) {
                    Ok(new_pane) => {
                        if focus {
                            s.switch_to_index(index);
                        } else {
                            // The split focuses its new pane; put focus back where it was.
                            s.split_engines[index].activate_pane(previous_pane);
                            s.split_engines[s.active_index].focus_active_surface();
                        }
                        s.trigger_session_save();
                        ok(
                            req_id,
                            json!({
                                "id": id,
                                "workspace_id": s.workspaces[index].uuid,
                                "pane": format!("pane:{new_pane}"),
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
            workspace,
            text,
            resp_tx,
        } => {
            let response = match send_terminal_text(state, id.as_deref(), workspace.as_deref(), &text) {
                Ok(()) => ok(req_id, json!({})),
                Err((code, message)) => err(req_id, code, message),
            };
            let _ = resp_tx.send(response);
        }

        SocketCommand::SurfaceSendKey {
            req_id,
            id,
            workspace,
            key,
            resp_tx,
        } => {
            // Named keys are synthesized as a press/release pair, literal
            // characters use typed input; an unknown key is never reported as delivered.
            let mut characters = key.chars();
            let result = if let Some(named) = crate::ghostty::named_key::parse(&key) {
                terminal_target(state, id.as_deref(), workspace.as_deref()).map(|surface| {
                    // SAFETY: resolution releases the model borrow and returns a
                    // live GTK-owned terminal; no teardown occurs before input.
                    unsafe { crate::ghostty::named_key::send(surface, named) }
                })
            } else if let (Some(character), None) = (characters.next(), characters.next()) {
                terminal_target(state, id.as_deref(), workspace.as_deref()).and_then(|surface| {
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
            workspace,
            resp_tx,
        } => {
            let result = terminal_target(state, id.as_deref(), workspace.as_deref()).and_then(|surface| {
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
                Ok(text) => {
                    use base64::Engine as _;
                    let mut payload = json!({
                        "text": text,
                        "base64": base64::engine::general_purpose::STANDARD.encode(&text),
                    });
                    let s = state.borrow();
                    let location = terminal_location(&s, id.as_deref(), workspace.as_deref())
                        .and_then(|(index, surface)| identify_location(&s, index, Some(surface)));
                    if let (Some(location), Some(fields)) = (location, payload.as_object_mut()) {
                        for key in [
                            "workspace_id",
                            "workspace_ref",
                            "surface_id",
                            "surface_ref",
                            "window_id",
                            "window_ref",
                        ] {
                            if let Some(value) = location.get(key) {
                                fields.insert(key.into(), value.clone());
                            }
                        }
                    }
                    ok(req_id, payload)
                }
                Err((code, message)) => err(req_id, code, message),
            };
            let _ = resp_tx.send(response);
        }

        SocketCommand::SurfaceTriggerFlash {
            req_id,
            surface,
            workspace,
            resp_tx,
        } => {
            let flash_state = state.clone();
            let response = {
                let mut s = state.borrow_mut();
                match resolve_tab_target(&s, surface.as_deref(), workspace.as_deref()) {
                    Err((code, message)) => err(req_id, code, &message),
                    Ok((index, pane_id, surface_uuid)) => {
                        // The two visual markers this build has: the pane's attention
                        // (sidebar dot, cleared with the workspace switch) and the tab's
                        // unread dot, which the notifications own and which therefore
                        // goes back to the truth two seconds later.
                        s.split_engines[index].root.set_attention(pane_id, true);
                        s.workspaces[index].has_attention =
                            s.split_engines[index].root.any_attention();
                        s.update_sidebar_attention(index);
                        let workspace_uuid = s.workspaces[index].uuid;
                        let mut unread: std::collections::HashSet<String> = s
                            .inbox
                            .records
                            .iter()
                            .filter(|record| {
                                !record.is_read && record.workspace_id == workspace_uuid
                            })
                            .filter_map(|record| record.surface_id.map(|id| id.to_string()))
                            .collect();
                        unread.insert(surface_uuid.clone());
                        s.split_engines[index].set_unread_tabs(&unread);
                        let surface_ref = s
                            .handles
                            .ensure_ref(super::handles::HandleKind::Surface, &surface_uuid);
                        let workspace_ref = s.handles.ensure_ref(
                            super::handles::HandleKind::Workspace,
                            &workspace_uuid.to_string(),
                        );
                        let window_ref = s.handles.ensure_ref(
                            super::handles::HandleKind::Window,
                            super::handles::MAIN_WINDOW_ID,
                        );
                        ok(
                            req_id,
                            json!({
                                "surface_id": surface_uuid,
                                "surface_ref": surface_ref,
                                "workspace_id": workspace_uuid,
                                "workspace_ref": workspace_ref,
                                "window_id": super::handles::MAIN_WINDOW_ID,
                                "window_ref": window_ref,
                            }),
                        )
                    }
                }
            };
            // Re-derive the tab dots once the flash has had its time.
            glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || {
                if let Ok(s) = flash_state.try_borrow() {
                    crate::inbox_actions::refresh(&s);
                }
            });
            let _ = resp_tx.send(response);
        }
        SocketCommand::SurfaceHealth {
            req_id,
            id,
            workspace,
            resp_tx,
        } => {
            // SOCK-05: health is NOT focus-intent — NO focus change.
            let response = {
                let s = state.borrow();
                // Upstream lists the selected workspace when nothing is named; a targeted
                // surface keeps this port's single-record answer (`health --id`).
                let listed = workspace.clone().or_else(|| {
                    id.is_none()
                        .then(|| s.workspaces.get(s.active_index).map(|row| row.uuid.to_string()))
                        .flatten()
                });
                match listed.as_deref() {
                    // Upstream `surface-health`: one record per surface of one workspace.
                    Some(workspace) => {
                        let found = uuid::Uuid::parse_str(workspace)
                            .ok()
                            .and_then(|uuid| s.workspaces.iter().position(|row| row.uuid == uuid));
                        match found {
                            None => err(req_id, "not_found", "workspace not found"),
                            Some(index) => {
                                let workspace_uuid = s.workspaces[index].uuid;
                                let workspace_ref = s.handles.ensure_ref(
                                    super::handles::HandleKind::Workspace,
                                    &workspace_uuid.to_string(),
                                );
                                let mut surfaces: Vec<Value> = Vec::new();
                                if let Some(engine) = s.split_engines.get(index) {
                                    for pane in engine.pane_info() {
                                        for (uuid, browser) in
                                            pane.surface_ids.iter().zip(&pane.browser_urls)
                                        {
                                            let id = uuid.to_string();
                                            // Upstream `in_window`: the hosted view is in a window;
                                            // here, the terminal's GL area is mapped.
                                            let in_window = match browser {
                                                None => json!(engine
                                                    .gl_area_for_surface(&id)
                                                    .is_some_and(|area| area.is_mapped())),
                                                Some(_) => Value::Null,
                                            };
                                            let mut record = json!({
                                                "index": surfaces.len(),
                                                "id": id,
                                                "in_window": in_window,
                                                "ref": s.handles.ensure_ref(
                                                    super::handles::HandleKind::Surface, &id),
                                                "type": if browser.is_some() { "browser" } else { "terminal" },
                                                "pane_id": pane.id,
                                                "pane_ref": format!("pane:{}", pane.id),
                                                "has_attention": engine.root.pane_has_attention(pane.id),
                                            });
                                            match browser {
                                                // A terminal is alive when its native surface is registered.
                                                None => {
                                                    record["alive"] = json!(engine
                                                        .find_surface_by_uuid(&id)
                                                        .is_some())
                                                }
                                                // A browser is alive while its daemon session exists.
                                                Some(_) => {
                                                    let session = s.browser_sessions.get(&uuid);
                                                    let status = match session {
                                                        Some(browser) if matches!(browser.preview_state, crate::browser::PreviewState::Connected | crate::browser::PreviewState::Streaming) => "connected",
                                                        Some(_) => "starting",
                                                        None => "suspended",
                                                    };
                                                    record["alive"] = json!(session.is_some());
                                                    record["browser_status"] = json!(status);
                                                }
                                            }
                                            surfaces.push(record);
                                        }
                                    }
                                }
                                ok(
                                    req_id,
                                    json!({
                                        "workspace_id": workspace_uuid,
                                        "workspace_ref": workspace_ref,
                                        "surfaces": surfaces,
                                        "window_id": super::handles::MAIN_WINDOW_ID,
                                        "window_ref": s.handles.ensure_ref(
                                            super::handles::HandleKind::Window,
                                            super::handles::MAIN_WINDOW_ID,
                                        ),
                                    }),
                                )
                            }
                        }
                    }
                    None => {
                        let (found, has_attention) = {
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
                                    let attn =
                                        engine.root.pane_has_attention(engine.active_pane_id);
                                    (alive, attn)
                                }
                            } else {
                                (false, false)
                            }
                        };
                        ok(
                            req_id,
                            json!({"alive": found, "has_attention": has_attention}),
                        )
                    }
                }
            };
            let _ = resp_tx.send(response);
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

        SocketCommand::PaneSurfaces {
            req_id,
            workspace,
            pane,
            resp_tx,
        } => {
            // Read-only listing; resolve the pane without changing the workspace or focus.
            let s = state.borrow();
            let located = match workspace {
                Some(workspace) => s
                    .workspaces
                    .iter()
                    .position(|ws| ws.uuid == workspace)
                    .and_then(|index| {
                        let engine = s.split_engines.get(index)?;
                        let pane_id = match pane.as_deref() {
                            Some(reference) => engine.pane_for_ref(reference)?,
                            None => engine.active_pane(),
                        };
                        Some((index, pane_id))
                    }),
                None => match pane.as_deref() {
                    Some(reference) => s
                        .split_engines
                        .iter()
                        .enumerate()
                        .find_map(|(index, engine)| {
                            engine
                                .pane_for_ref(reference)
                                .map(|pane_id| (index, pane_id))
                        }),
                    None => s
                        .split_engines
                        .get(s.active_index)
                        .map(|engine| (s.active_index, engine.active_pane())),
                },
            };
            let response = match located {
                Some((index, pane_id)) => {
                    let engine = &s.split_engines[index];
                    let Some(info) = engine
                        .pane_info()
                        .into_iter()
                        .find(|info| info.id == pane_id)
                    else {
                        let _ =
                            resp_tx.send(err(req_id, "not_found", "Pane or workspace not found"));
                        return;
                    };
                    use super::handles::{HandleKind, MAIN_WINDOW_ID};
                    let workspace = s.workspaces[index].uuid.to_string();
                    let pane_ref = format!("pane:{pane_id}");
                    let surfaces: Vec<Value> = info
                        .surface_ids
                        .iter()
                        .enumerate()
                        .map(|(index, surface)| {
                            let id = surface.to_string();
                            let browser = info
                                .browser_urls
                                .get(index)
                                .is_some_and(|url| url.is_some());
                            json!({
                                "id": id,
                                "ref": s.handles.ensure_ref(HandleKind::Surface, &id),
                                "index": index,
                                "title": if browser { None } else { engine.surface_title(&id) },
                                "type": if browser { "browser" } else { "terminal" },
                                "selected": info.selected_surface == Some(*surface),
                            })
                        })
                        .collect();
                    ok(
                        req_id,
                        json!({
                            "workspace_id": workspace,
                            "workspace_ref": s.handles.ensure_ref(HandleKind::Workspace, &workspace),
                            "pane_id": pane_ref,
                            "pane_ref": pane_ref,
                            "window_id": MAIN_WINDOW_ID,
                            "window_ref": s.handles.ensure_ref(HandleKind::Window, MAIN_WINDOW_ID),
                            "surfaces": surfaces,
                        }),
                    )
                }
                None => err(req_id, "not_found", "Pane or workspace not found"),
            };
            let _ = resp_tx.send(response);
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

        // Upstream `controlPaneJoin`: the surface, else the source pane's selected one, moved
        // by `surface.move` into the target pane; its reply is forwarded as is.
        SocketCommand::PaneJoin {
            req_id,
            target_pane,
            surface,
            source_pane,
            focus,
            resp_tx,
        } => {
            let surface = match (surface, source_pane) {
                (Some(surface), _) => surface,
                (None, Some(source_pane)) => {
                    let s = state.borrow();
                    let selected = s.split_engines.iter().find_map(|engine| {
                        let pane = engine.pane_for_ref(&source_pane)?;
                        engine
                            .pane_info()
                            .into_iter()
                            .find(|info| info.id == pane)?
                            .selected_surface
                    });
                    match selected {
                        Some(selected) => selected.to_string(),
                        None => {
                            let _ = resp_tx.send(err_data(
                                req_id,
                                "not_found",
                                "Unable to resolve selected surface in source pane",
                                json!({"pane_id": source_pane}),
                            ));
                            return;
                        }
                    }
                }
                (None, None) => {
                    let _ = resp_tx.send(err(
                        req_id,
                        "invalid_params",
                        "Missing surface_id (or pane_id with selected surface)",
                    ));
                    return;
                }
            };
            handle_socket_command(
                SocketCommand::SurfaceMove {
                    req_id,
                    id: surface,
                    workspace: None,
                    pane: Some(target_pane),
                    position: None,
                    before: None,
                    after: None,
                    focus: focus.unwrap_or(true),
                    resp_tx,
                },
                state,
            );
        }

        // Upstream `controlPaneBreak`: surface given, else the pane's selected surface, else
        // the focused pane's. The move itself is `tab.action move_to_new_workspace`, whose
        // reply is reshaped to upstream's (destination workspace, pane and surface). Écart:
        // an unknown pane is `not_found` here; upstream silently falls back to the focused pane.
        SocketCommand::PaneBreak {
            req_id,
            pane,
            surface,
            workspace,
            focus,
            resp_tx,
        } => {
            let resolved = {
                let s = state.borrow();
                let index = match (&workspace, &surface) {
                    (Some(workspace), _) => s.workspaces.iter().position(|row| row.uuid.to_string() == *workspace),
                    (None, Some(surface)) => s
                        .split_engines
                        .iter()
                        .position(|engine| engine.find_pane_id_by_uuid(surface).is_some()),
                    (None, None) => Some(s.active_index),
                };
                match index {
                    None if workspace.is_some() => Err(("not_found", "Workspace not found")),
                    None => Err(("not_found", "Surface not found")),
                    Some(index) => {
                        let engine = &s.split_engines[index];
                        let source_pane = match &pane {
                            Some(pane) => engine.pane_for_ref(pane).ok_or(("not_found", "Pane not found")),
                            None => Ok(engine.active_pane_id),
                        };
                        source_pane.and_then(|source_pane| match &surface {
                            Some(surface) if engine.find_pane_id_by_uuid(surface).is_some() => {
                                Ok((index, surface.clone()))
                            }
                            Some(_) => Err(("not_found", "Surface not found")),
                            None => engine
                                .pane_info()
                                .into_iter()
                                .find(|info| info.id == source_pane)
                                .and_then(|info| info.selected_surface)
                                .map(|selected| (index, selected.to_string()))
                                .ok_or(("not_found", "No source surface to break")),
                        })
                        .map(|(index, surface)| (s.workspaces[index].uuid.to_string(), surface))
                    }
                }
            };
            let (workspace_id, surface_id) = match resolved {
                Ok(target) => target,
                Err((code, message)) => {
                    let _ = resp_tx.send(err(req_id, code, message));
                    return;
                }
            };
            let (inner_tx, mut inner_rx) = tokio::sync::oneshot::channel();
            handle_socket_command(
                SocketCommand::TabAction {
                    req_id: req_id.clone(),
                    action: "move_to_new_workspace".to_owned(),
                    surface: Some(surface_id),
                    workspace: Some(workspace_id),
                    title: None,
                    focus,
                    resp_tx: inner_tx,
                },
                state,
            );
            let reply = inner_rx.try_recv().unwrap_or_else(|_| {
                err(req_id.clone(), "internal_error", "Failed to detach source surface")
            });
            let Some(result) = reply.get("result") else {
                let _ = resp_tx.send(reply);
                return;
            };
            let pane_ref = result
                .get("pane_ref")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let mut shaped = serde_json::Map::new();
            for key in [
                "window_id",
                "window_ref",
                "workspace_id",
                "workspace_ref",
                "surface_id",
                "surface_ref",
            ] {
                shaped.insert(key.to_owned(), result.get(key).cloned().unwrap_or_default());
            }
            shaped.insert("pane_id".to_owned(), pane_ref.clone());
            shaped.insert("pane_ref".to_owned(), pane_ref);
            let _ = resp_tx.send(ok(req_id, serde_json::Value::Object(shaped)));
        }

        SocketCommand::PaneSwap {
            req_id,
            pane_id,
            target_pane_id,
            focus,
            resp_tx,
        } => {
            let mut s = state.borrow_mut();
            let located = s
                .split_engines
                .iter()
                .enumerate()
                .find_map(|(index, engine)| {
                    engine.pane_for_ref(&pane_id).map(|pane| (index, pane))
                });
            let Some((index, source_pane)) = located else {
                let _ = resp_tx.send(err_data(
                    req_id,
                    "not_found",
                    "Source pane not found",
                    json!({"pane_id": pane_id}),
                ));
                return;
            };
            let Some(target_pane) = s.split_engines[index].pane_for_ref(&target_pane_id)
            else {
                let _ = resp_tx.send(err_data(
                    req_id,
                    "not_found",
                    "Target pane not found in source workspace",
                    json!({"target_pane_id": target_pane_id}),
                ));
                return;
            };
            if source_pane == target_pane {
                let _ = resp_tx.send(err(
                    req_id,
                    "invalid_params",
                    "pane_id and target_pane_id must be different",
                ));
                return;
            }
            let panes = s.split_engines[index].pane_info();
            let selected = |pane: u64| {
                panes
                    .iter()
                    .find(|info| info.id == pane)
                    .and_then(|info| info.selected_surface)
            };
            let (Some(source_surface), Some(target_surface)) =
                (selected(source_pane), selected(target_pane))
            else {
                let _ = resp_tx.send(err(
                    req_id,
                    "invalid_state",
                    "Both panes must have a selected surface",
                ));
                return;
            };
            let surface_count = |pane: u64| {
                panes
                    .iter()
                    .find(|info| info.id == pane)
                    .map_or(0, |info| info.surface_ids.len())
            };
            // Upstream keeps both pane identities when a side holds only the surface being
            // moved: a temporary terminal stabilizes that pane until both swaps are done.
            let mut placeholders = Vec::new();
            if surface_count(source_pane) <= 1 {
                match s.split_engines[index].new_terminal_surface(
                    source_pane,
                    crate::split_engine::TerminalLaunch::default(),
                    false,
                ) {
                    Ok(placeholder) => placeholders.push(placeholder),
                    Err(_) => {
                        let _ = resp_tx.send(err(
                            req_id,
                            "internal_error",
                            "Failed to create source placeholder surface",
                        ));
                        return;
                    }
                }
            }
            if surface_count(target_pane) <= 1 {
                match s.split_engines[index].new_terminal_surface(
                    target_pane,
                    crate::split_engine::TerminalLaunch::default(),
                    false,
                ) {
                    Ok(placeholder) => placeholders.push(placeholder),
                    Err(_) => {
                        let _ = resp_tx.send(err(
                            req_id,
                            "internal_error",
                            "Failed to create target placeholder surface",
                        ));
                        return;
                    }
                }
            }
            if s.split_engines[index]
                .move_surface(source_surface, target_pane, None, false)
                .is_err()
            {
                let _ = resp_tx.send(err(
                    req_id,
                    "internal_error",
                    "Failed moving source surface into target pane",
                ));
                return;
            }
            if s.split_engines[index]
                .move_surface(target_surface, source_pane, None, false)
                .is_err()
            {
                let _ = resp_tx.send(err(
                    req_id,
                    "internal_error",
                    "Failed moving target surface into source pane",
                ));
                return;
            }
            // The moved tabs were appended unselected: show each pane's new surface before
            // dropping the placeholders, so a focused pane does not fall back to another tab.
            let _ = s.split_engines[index].select_surface(target_surface);
            let _ = s.split_engines[index].select_surface(source_surface);
            for placeholder in placeholders {
                let _ = s.split_engines[index].close_surface_tab(placeholder);
            }
            if focus {
                if index != s.active_index {
                    s.switch_to_index(index);
                }
                let _ = s.split_engines[index].focus_pane_ref(&format!("pane:{target_pane}"));
            } else if index != s.active_index {
                // Closing a placeholder reconciles focus inside the source workspace; put it
                // back on the workspace the user was in when focus was not requested.
                s.split_engines[s.active_index].focus_active_surface();
            }
            s.trigger_session_save();
            use super::handles::{HandleKind, MAIN_WINDOW_ID};
            let workspace_id = s.workspaces[index].uuid.to_string();
            let source_surface_id = source_surface.to_string();
            let target_surface_id = target_surface.to_string();
            let response = ok(
                req_id,
                json!({
                    "window_id": MAIN_WINDOW_ID,
                    "window_ref": s.handles.ensure_ref(HandleKind::Window, MAIN_WINDOW_ID),
                    "workspace_id": workspace_id,
                    "workspace_ref": s.handles.ensure_ref(HandleKind::Workspace, &workspace_id),
                    "pane_id": format!("pane:{source_pane}"),
                    "pane_ref": format!("pane:{source_pane}"),
                    "target_pane_id": format!("pane:{target_pane}"),
                    "target_pane_ref": format!("pane:{target_pane}"),
                    "source_surface_id": source_surface_id,
                    "source_surface_ref": s.handles.ensure_ref(HandleKind::Surface, &source_surface_id),
                    "target_surface_id": target_surface_id,
                    "target_surface_ref": s.handles.ensure_ref(HandleKind::Surface, &target_surface_id),
                }),
            );
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

        // -- right_sidebar.* (right sidebar / Files panel) --
        // SOCK-05: visibility is a layout mutation, never a focus mutation.
        SocketCommand::RightSidebarApply {
            req_id,
            action,
            mode,
            resp_tx,
        } => {
            let s = state.borrow();
            match action.as_str() {
                "show" | "set" => s.set_right_sidebar_visible(true),
                "hide" => s.set_right_sidebar_visible(false),
                _ => s.toggle_right_sidebar(),
            }
            let visible = s.right_sidebar_visible();
            let mode = mode.unwrap_or_else(|| "files".to_owned());
            let _ = resp_tx.send(ok(req_id, json!({"visible": visible, "mode": mode})));
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
                BrowserPlacement::ActivePane,
            );
        }

        SocketCommand::BrowserOpenSplit {
            req_id,
            url,
            source,
            pane,
            workspace,
            profile,
            focus,
            resp_tx,
        } => {
            let prepared = {
                let s = state.borrow();
                open_split_source(
                    &s,
                    source.as_deref(),
                    pane.as_deref(),
                    workspace.as_deref(),
                )
                .map(|(index, workspace, pane, surface)| {
                    let target = s.split_engines[index].right_neighbor(pane);
                    (workspace, pane, surface, target)
                })
            };
            let (workspace, source_pane, source_surface, target_pane) = match prepared {
                Ok(prepared) => prepared,
                Err((code, message)) => {
                    let _ = resp_tx.send(err(req_id, code, message));
                    return;
                }
            };
            let mut params = json!({
                "url": crate::browser_address::normalize(&url),
                "workspace": workspace,
            });
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
                BrowserPlacement::OpenSplit {
                    source_surface,
                    source_pane,
                    target_pane,
                    focus,
                },
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
                BrowserPlacement::ActivePane,
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
                let trace = trace_id
                    .as_deref()
                    .and_then(|id| uuid::Uuid::parse_str(id).ok());
                // The daemon emulates the position but never grants the page the permission to read
                // it, so a page would get "User denied Geolocation" behind a reported success.
                let grant_permission = (daemon_action == "geolocation").then(|| {
                    bm.send_command_async("permissions", json!({"permissions": ["geolocation"]}), trace)
                });
                let exchange = bm.send_command_async(&daemon_action, params, trace);
                let (url_tx, url_rx) = tokio::sync::oneshot::channel();
                drop(s);
                spawn_browser_exchange(
                    &runtime,
                    async move {
                        if let Some(grant) = grant_permission {
                            let granted = grant.await?;
                            if granted.get("success").and_then(Value::as_bool) == Some(false) {
                                return Ok(granted);
                            }
                        }
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

/// Where a completed browser lifecycle installs its new surface.
#[derive(Clone, Copy)]
pub(super) enum BrowserPlacement {
    /// The active pane of the request's workspace, as `browser.open` has always done.
    ActivePane,
    /// A source pane and its right neighbor, or a new split to the source's right.
    OpenSplit {
        source_surface: uuid::Uuid,
        source_pane: u64,
        target_pane: Option<u64>,
        focus: bool,
    },
}

/// Initialize and command the daemon on Tokio, then apply surviving results on GTK without stealing focus.
pub(super) fn start_browser_lifecycle(
    state: &std::rc::Rc<std::cell::RefCell<crate::app_state::AppState>>,
    mut request: crate::browser::StartupRequest,
    req_id: Value,
    mut resp_tx: super::commands::RespTx,
    trace_id: Option<String>,
    project_surface: Option<String>,
    placement: BrowserPlacement,
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
        let mut placement_outcome = None;
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
            let Some(engine) = s.split_engines.get_mut(index) else {
                activity.finish("missing_workspace");
                let _ = resp_tx.send(err(
                    req_id,
                    "not_found",
                    "Target workspace closed during browser startup",
                ));
                return;
            };
            let created = match placement {
                BrowserPlacement::ActivePane => {
                    let pane = engine.active_pane();
                    engine
                        .add_preview_with_profile(false, initial_profile.clone())
                        .map(|widgets| (widgets, pane, false))
                }
                BrowserPlacement::OpenSplit {
                    source_pane,
                    target_pane,
                    focus,
                    ..
                } => {
                    if !engine.contains_pane(source_pane) {
                        None
                    } else if let Some(target) =
                        target_pane.filter(|target| engine.contains_pane(*target))
                    {
                        engine
                            .add_preview_to_pane(target, focus, initial_profile.clone())
                            .map(|widgets| (widgets, target, false))
                    } else {
                        engine
                            .split_pane_browser(
                                source_pane,
                                crate::split_engine::FocusDirection::Right,
                                initial_profile.clone(),
                                focus,
                            )
                            .ok()
                            .map(|(pane, widgets)| (widgets, pane, true))
                    }
                }
            };
            match created {
                Some((widgets, target_pane, created_split)) => {
                    placement_outcome = Some((target_pane, created_split));
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
                    Some(widgets)
                }
                None => None,
            }
        };
        let Some(widgets) = new_widgets else {
            activity.finish("missing_surface");
            let (code, message) = match placement {
                BrowserPlacement::OpenSplit { .. } => {
                    ("internal_error", "Failed to create browser")
                }
                BrowserPlacement::ActivePane => {
                    ("not_found", "Could not create browser surface")
                }
            };
            let _ = resp_tx.send(err(req_id, code, message));
            return;
        };
        {
            let surface = widgets.uuid;
            crate::browser::ui::wire_browser_tab(&state, widgets, activity.id);
            match placement {
                BrowserPlacement::OpenSplit {
                    source_surface,
                    source_pane,
                    focus,
                    ..
                } => {
                    let Some((target_pane, created_split)) = placement_outcome else {
                        activity.finish("missing_surface");
                        let _ = resp_tx.send(err(
                            req_id,
                            "internal_error",
                            "Failed to create browser",
                        ));
                        return;
                    };
                    let mut s = state.borrow_mut();
                    if focus {
                        if let Some(index) = s
                            .workspaces
                            .iter()
                            .position(|row| Some(row.uuid) == workspace)
                        {
                            s.switch_to_index(index);
                            s.split_engines[index].focus_surface(&surface.to_string());
                        }
                    }
                    use super::handles::{HandleKind, MAIN_WINDOW_ID};
                    result = json!({
                        "surface_id": surface.to_string(),
                        "surface_ref": s.handles.ensure_ref(HandleKind::Surface, &surface.to_string()),
                        "workspace_id": workspace.map(|id| id.to_string()),
                        "workspace_ref": workspace.map(|id| s.handles.ensure_ref(HandleKind::Workspace, &id.to_string())),
                        "window_id": MAIN_WINDOW_ID,
                        "window_ref": s.handles.ensure_ref(HandleKind::Window, MAIN_WINDOW_ID),
                        "pane_id": format!("pane:{target_pane}"),
                        "pane_ref": format!("pane:{target_pane}"),
                        "source_surface_id": source_surface.to_string(),
                        "source_surface_ref": s.handles.ensure_ref(HandleKind::Surface, &source_surface.to_string()),
                        "source_pane_id": format!("pane:{source_pane}"),
                        "source_pane_ref": format!("pane:{source_pane}"),
                        "target_pane_id": format!("pane:{target_pane}"),
                        "target_pane_ref": format!("pane:{target_pane}"),
                        "created_split": created_split,
                        "placement_strategy": if created_split { "split_right" } else { "reuse_right_sibling" },
                        "url": initial_url.as_deref().unwrap_or("about:blank"),
                    });
                }
                BrowserPlacement::ActivePane => {
                    if project_surface.is_some() {
                        let mut s = state.borrow_mut();
                        if let Some(index) = s
                            .workspaces
                            .iter()
                            .position(|row| Some(row.uuid) == workspace)
                        {
                            s.switch_to_index(index);
                            s.split_engines[index].focus_surface(&surface.to_string());
                        }
                        result = json!({"workspace_id":workspace,"source_workspace_id":workspace,"surface_id":surface,"status":"submitted"});
                        crate::diagnostics::record("project.actions.run", json!({"trace_id":trace,"workspace_id":workspace,"source_workspace_id":workspace,"surface_id":surface,"outcome":"submitted","duration_us":started.elapsed().as_micros() as u64}));
                    }
                }
            }
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
