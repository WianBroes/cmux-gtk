//! GTK-thread notification targeting, state mutation and focus-intent navigation.
use crate::{
    app_state::AppState,
    inbox::{Action, Record, Scope},
};
use gtk4::prelude::*;
use serde_json::{json, Value};
use uuid::Uuid;

type Error = (&'static str, &'static str);

/// Publish a notification-store event; text fields stay local and only their lengths travel.
fn publish(name: &str, workspace: Option<Uuid>, surface: Option<Uuid>, payload: Value) {
    crate::events::publish(
        name,
        "notification.store",
        crate::events::Scope {
            workspace,
            surface: surface.map(|id| id.to_string()),
            ..Default::default()
        },
        payload,
    );
}

/// Resolve explicit surface identity across workspaces and reject conflicting scope selectors.
fn target(state: &AppState, scope: &Scope) -> Result<(usize, Uuid), Error> {
    let index = if let Some(surface) = scope.surface_id {
        state
            .split_engines
            .iter()
            .position(|engine| engine.find_pane_id_by_uuid(&surface.to_string()).is_some())
            .ok_or(("not_found", "notification surface not found"))?
    } else if let Some(workspace) = scope.workspace_id {
        state
            .workspaces
            .iter()
            .position(|row| row.uuid == workspace)
            .ok_or(("not_found", "notification workspace not found"))?
    } else {
        state.active_index
    };
    let workspace = state
        .workspaces
        .get(index)
        .ok_or(("not_found", "notification workspace not found"))?;
    if scope.workspace_id.is_some_and(|id| id != workspace.uuid) {
        return Err(("not_found", "surface is not in the selected workspace"));
    }
    let surface = scope
        .surface_id
        .or_else(|| {
            state
                .split_engines
                .get(index)?
                .active_pane_uuid()?
                .parse()
                .ok()
        })
        .ok_or(("not_found", "notification surface not found"))?;
    Ok((index, surface))
}

/// Match historical routing identities without redirecting messages whose surfaces have closed.
fn matches(record: &Record, scope: &Scope) -> bool {
    scope
        .workspace_id
        .is_none_or(|id| id == record.workspace_id)
        && scope
            .surface_id
            .is_none_or(|id| Some(id) == record.surface_id)
}

/// Apply one admitted operation; only Open and JumpToUnread are allowed to focus the target.
pub fn handle(state: &mut AppState, action: Action) -> Result<Value, Error> {
    let result = match action {
        Action::Create { scope, content } => {
            let (index, surface) = target(state, &scope)?;
            create(state, index, Some(surface), content)
        }
        Action::CreateForCaller { caller, content } => {
            let scope = crate::notification_caller::resolve(state, &caller)?;
            let index = state
                .workspaces
                .iter()
                .position(|workspace| Some(workspace.uuid) == scope.workspace_id)
                .ok_or(("not_found", "notification workspace not found"))?;
            create(state, index, scope.surface_id, content)
        }
        Action::ClearForCaller(caller) => {
            let scope = crate::notification_caller::resolve(state, &caller)?;
            return handle(state, Action::Clear(scope));
        }
        Action::Clear(scope) => {
            if scope.workspace_id.is_some() || scope.surface_id.is_some() {
                target(state, &scope)?;
            }
            state
                .inbox
                .records
                .retain(|record| !matches(record, &scope));
            for index in 0..state.workspaces.len() {
                if scope
                    .workspace_id
                    .is_none_or(|id| state.workspaces[index].uuid == id)
                    && scope.surface_id.is_none()
                {
                    state.clear_workspace_attention(index);
                }
            }
            publish(
                "notification.cleared",
                scope.workspace_id,
                scope.surface_id,
                json!({}),
            );
            json!({"cleared": true, "workspace_id": scope.workspace_id, "surface_id": scope.surface_id})
        }
        Action::MarkRead { id, scope, all } => {
            if id.is_some_and(|id| !state.inbox.records.iter().any(|record| record.id == id)) {
                return Err(("not_found", "notification not found"));
            }
            let mut marked = 0;
            for record in &mut state.inbox.records {
                if (all || id == Some(record.id) || (id.is_none() && matches(record, &scope)))
                    && !record.is_read
                {
                    record.is_read = true;
                    marked += 1;
                    publish(
                        "notification.read",
                        Some(record.workspace_id),
                        record.surface_id,
                        json!({"notification_id": record.id}),
                    );
                }
            }
            json!({"marked_read": marked})
        }
        Action::Dismiss { id, all_read } => {
            let before = state.inbox.records.len();
            state.inbox.records.retain(|record| {
                let remove = id == Some(record.id) || (all_read && record.is_read);
                if remove {
                    publish(
                        "notification.removed",
                        Some(record.workspace_id),
                        record.surface_id,
                        json!({"notification_id": record.id}),
                    );
                }
                !remove
            });
            let dismissed = before - state.inbox.records.len();
            if id.is_some() && dismissed == 0 {
                return Err(("not_found", "notification not found"));
            }
            json!({"dismissed": dismissed, "all_read": all_read})
        }
        Action::Open(id) => open(state, id)?,
        Action::JumpToUnread => {
            let id = state
                .inbox
                .records
                .iter()
                .rev()
                .find(|record| !record.is_read && saved_target(state, record).is_ok())
                .map(|record| record.id);
            match id {
                Some(id) => open(state, id)?,
                None => json!({"opened": false}),
            }
        }
    };
    refresh(state);
    if let Some(sender) = &state.inbox_updates {
        sender.send_replace(());
    }
    state.trigger_session_save();
    Ok(result)
}

/// Resolve retained identity without guessing an active terminal for a workspace-only message.
/// Closed terminals stay unavailable even when their old workspace has a different selected terminal.
fn saved_target(state: &AppState, record: &Record) -> Result<(usize, Option<Uuid>), Error> {
    let index = state
        .workspaces
        .iter()
        .position(|workspace| workspace.uuid == record.workspace_id)
        .ok_or(("not_found", "notification workspace not found"))?;
    if let Some(surface) = record.surface_id {
        if state
            .split_engines
            .get(index)
            .and_then(|engine| engine.find_pane_id_by_uuid(&surface.to_string()))
            .is_none()
        {
            return Err(("not_found", "notification surface not found"));
        }
    }
    Ok((index, record.surface_id))
}

/// Select the exact saved terminal tab and mark only this message read after successful routing.
fn open(state: &mut AppState, id: Uuid) -> Result<Value, Error> {
    let record = state
        .inbox
        .records
        .iter()
        .find(|record| record.id == id)
        .cloned()
        .ok_or(("not_found", "notification not found"))?;
    let (index, surface) = saved_target(state, &record)?;
    state.switch_to_index(index);
    if let Some(surface) = surface {
        if !state.split_engines[index].focus_surface(&surface.to_string()) {
            return Err(("not_found", "notification surface not found"));
        }
    }
    let record = state
        .inbox
        .records
        .iter_mut()
        .find(|record| record.id == id)
        .expect("focus does not remove notification records");
    record.is_read = true;
    let mut result = serde_json::to_value(record).expect("notification record serializes");
    result["opened"] = json!(true);
    Ok(result)
}

/// Announce purged records, then reconcile badge and rings (upstream clears on close).
fn purged(state: &AppState, removed: Vec<Record>) {
    for record in &removed {
        publish(
            "notification.removed",
            Some(record.workspace_id),
            record.surface_id,
            json!({"notification_id": record.id}),
        );
    }
    if !removed.is_empty() {
        refresh(state);
    }
}

/// Drop the messages of a workspace that was just closed.
pub fn purge_workspace(state: &mut AppState, workspace: Uuid) {
    let removed = state.inbox.purge_workspace(workspace);
    purged(state, removed);
}

/// Drop the messages of terminals that no longer exist; call after any tab or pane closure.
pub fn purge_closed_surfaces(state: &mut AppState) {
    let live: std::collections::HashSet<Uuid> = state
        .split_engines
        .iter()
        .flat_map(|engine| engine.pane_info())
        .flat_map(|pane| pane.surface_ids)
        .collect();
    let removed = state.inbox.purge_dead_surfaces(&live);
    purged(state, removed);
    for index in 0..state.workspaces.len() {
        if !state.workspaces[index].agents.is_empty() {
            state.update_agents(index, |_| {});
        }
    }
}

/// Reconcile unread rings and sidebar dots without disturbing independent terminal BEL attention.
pub fn refresh(state: &AppState) {
    if let Some(badge) = &state.notifications_badge {
        let unread = state.inbox.records.iter().filter(|record| !record.is_read).count();
        badge.set_text(&if unread > 99 { "99+".into() } else { unread.to_string() });
        badge.set_visible(unread > 0);
    }
    for (index, engine) in state.split_engines.iter().enumerate() {
        let unread_panes: std::collections::HashSet<_> = state
            .inbox
            .records
            .iter()
            .filter(|record| !record.is_read && record.workspace_id == state.workspaces[index].uuid)
            .filter_map(|record| {
                record
                    .surface_id
                    .and_then(|surface| engine.find_pane_id_by_uuid(&surface.to_string()))
            })
            .collect();
        let unread_tabs: std::collections::HashSet<String> = state
            .inbox
            .records
            .iter()
            .filter(|record| !record.is_read && record.workspace_id == state.workspaces[index].uuid)
            .filter_map(|record| record.surface_id.map(|surface| surface.to_string()))
            .collect();
        engine.set_unread_tabs(&unread_tabs);
        for (_, pane, _) in engine.all_panes() {
            if let Some(node) = engine.root.find_node(pane) {
                let unread = unread_panes.contains(&pane);
                let widget = node.widget();
                if unread {
                    widget.add_css_class("notification-unread");
                } else {
                    widget.remove_css_class("notification-unread");
                }
            }
        }
        state.update_sidebar_attention(index);
    }
}

/// Retain a message with proven terminal identity, or workspace-only attribution, without focus changes.
fn create(
    state: &mut AppState,
    index: usize,
    surface: Option<Uuid>,
    content: crate::inbox::Content,
) -> Value {
    let focused = state.active_index == index
        && surface.is_none_or(|surface| {
            state.split_engines[index].active_pane_uuid().as_deref() == Some(&surface.to_string())
        })
        && state
            .gtk_app
            .active_window()
            .is_some_and(|window| window.is_active());
    let id = Uuid::new_v4();
    let workspace = state.workspaces[index].uuid;
    let desktop = (!focused).then(|| {
        cmux_platform::notification::message(&content.title, &content.subtitle, &content.body)
    });
    let created_at = glib::DateTime::now_utc()
        .and_then(|date| date.format_iso8601())
        .map(|value| value.to_string())
        .unwrap_or_default();
    publish(
        "notification.created",
        Some(workspace),
        surface,
        json!({
            "notification_id": id, "title": null, "subtitle": null, "body": null,
            "title_length": content.title.chars().count(),
            "subtitle_length": content.subtitle.chars().count(),
            "body_length": content.body.chars().count(),
            "redacted_fields": ["title", "subtitle", "body"],
            "delivery": if focused { "store" } else { "desktop" },
        }),
    );
    // Upstream keeps one live notification per terminal (or per workspace without one):
    // a newer message supersedes the older ones instead of piling up.
    state
        .inbox
        .records
        .retain(|record| !(record.workspace_id == workspace && record.surface_id == surface));
    let evicted = state.inbox.push(Record {
        id,
        workspace_id: workspace,
        surface_id: surface,
        content,
        created_at,
        is_read: focused,
    });
    if let (Some(runtime), Some(command)) = (&state.runtime_handle, desktop) {
        crate::notification::send_message(runtime, command, workspace, id);
    }
    crate::diagnostics::record(
        "notification.inbox.create",
        json!({"id":id,"workspace":workspace,"surface":surface,"focused":focused,"evicted":evicted}),
    );
    json!({"id": id, "workspace_id": workspace, "surface_id": surface})
}

/// Upstream read rule: mark exactly this terminal's (or, with `None`, this workspace's own)
/// notifications read. Returns whether anything changed.
pub fn mark_read_where(state: &mut AppState, workspace: Uuid, surface: Option<Uuid>) -> bool {
    let mut changed = false;
    for record in &mut state.inbox.records {
        if !record.is_read && record.workspace_id == workspace && record.surface_id == surface {
            record.is_read = true;
            changed = true;
        }
    }
    if changed {
        refresh(state);
        if let Some(sender) = &state.inbox_updates {
            sender.send_replace(());
        }
        state.trigger_session_save();
    }
    changed
}

/// Upstream `markRead(forTabId:)`: every message of the workspace read and its dots cleared
/// (`workspace.action mark_read`, the row menu and Toggle Unread).
pub fn mark_workspace_read(state: &mut AppState, index: usize) {
    let scope = Scope {
        workspace_id: Some(state.workspaces[index].uuid),
        surface_id: None,
    };
    let _ = handle(
        state,
        Action::MarkRead {
            id: None,
            scope,
            all: false,
        },
    );
    state.clear_workspace_attention(index);
    state.trigger_session_save();
}

/// Upstream `markUnread(forTabId:)`: a mark on the workspace, read messages stay read.
pub fn mark_workspace_unread(state: &mut AppState, index: usize) {
    state.workspaces[index].has_attention = true;
    state.update_sidebar_attention(index);
}

/// Upstream `markPanelUnread`: a mark on the tab's pane (`tab.action mark_unread`, Toggle Unread).
pub fn mark_tab_unread(state: &mut AppState, index: usize, pane_id: u64) {
    state.split_engines[index].root.set_attention(pane_id, true);
    state.workspaces[index].has_attention = state.split_engines[index].root.any_attention();
    state.update_sidebar_attention(index);
}

/// Upstream `workspaceIsUnread`: an unread message or the workspace's own unread mark.
pub fn workspace_is_unread(state: &AppState, index: usize) -> bool {
    let workspace = &state.workspaces[index];
    workspace.has_attention
        || state
            .inbox
            .records
            .iter()
            .any(|record| !record.is_read && record.workspace_id == workspace.uuid)
}

/// Upstream `toggleFocusedNotificationUnread`: an unread workspace is marked read, otherwise
/// its focused tab becomes unread.
pub fn toggle_unread(state: &mut AppState) {
    let index = state.active_index;
    if index >= state.workspaces.len() {
        return;
    }
    if workspace_is_unread(state, index) {
        mark_workspace_read(state, index);
        return;
    }
    let pane_id = state.split_engines[index].active_pane_id;
    mark_tab_unread(state, index, pane_id);
}

/// Focusing a terminal marks its notifications read and lets it name its workspace, as upstream.
pub fn terminal_focused(state: &crate::app_state::AppStateRef, surface: Uuid) {
    let Ok(mut s) = state.try_borrow_mut() else {
        let state = state.clone();
        glib::idle_add_local_once(move || terminal_focused(&state, surface));
        return;
    };
    let index = s.active_index;
    // Only a terminal of the selected workspace counts, whichever pane its tab is in now.
    if s.split_engines
        .get(index)
        .and_then(|engine| engine.find_pane_id_by_uuid(&surface.to_string()))
        .is_none()
    {
        return;
    }
    let workspace = s.workspaces[index].uuid;
    mark_read_where(&mut s, workspace, Some(surface));
    s.record_focus(index, surface);
    // The focused tab also names a workspace that has no user-chosen name.
    s.apply_focused_title(index);
}
