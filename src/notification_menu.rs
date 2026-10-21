//! Notifications menu of the menu bar (upstream `cmuxApp.swift:602-648`) and the notification
//! entries of a workspace row menu (`SidebarWorkspaceRowCommands.swift:681-745`).
use crate::{app_state::AppState, inbox::Record};
use gtk4::{gio, glib, prelude::*};
use std::cell::RefCell;

/// Upstream `NotificationMenuSnapshotBuilder.defaultInlineNotificationLimit`.
const RECENT_LIMIT: usize = 6;

/// The live Notifications submenu and the content it shows, rebuilt only when that changes.
pub struct MenuBarEntry {
    menu: gio::Menu,
    shown: RefCell<String>,
}

impl MenuBarEntry {
    pub fn new(menu: gio::Menu) -> Self {
        Self {
            menu,
            shown: RefCell::new(String::from("\0")),
        }
    }
}

/// Upstream `unreadCount`: unread messages plus the workspaces marked unread.
fn unread_count(state: &AppState) -> usize {
    state.inbox.records.iter().filter(|record| !record.is_read).count()
        + state
            .workspaces
            .iter()
            .filter(|workspace| workspace.has_attention)
            .count()
}

/// Upstream `stateHintTitle`.
fn state_hint(unread: usize) -> String {
    match unread {
        0 => "No unread notifications".into(),
        1 => "1 unread notification".into(),
        count => format!("{count} unread notifications"),
    }
}

/// Menu labels read `_` as a mnemonic marker; notification text is shown as written.
fn menu_text(text: &str) -> String {
    text.replace('_', "__")
}

/// The stored UTC instant in local time, e.g. `%H:%M`; empty when it does not parse.
fn local_time(created_at: &str, format: &str) -> String {
    glib::DateTime::from_iso8601(created_at, None)
        .and_then(|date| date.to_local())
        .and_then(|date| date.format(format))
        .map(|text| text.to_string())
        .unwrap_or_default()
}

/// First line, trimmed, cut to `limit` characters with `...` (upstream `bounded`).
fn bounded(value: &str, limit: usize) -> String {
    let line = value.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= limit {
        return line.into();
    }
    format!("{}...", line.chars().take(limit).collect::<String>().trim_end())
}

/// Upstream `MenuBarNotificationLineFormatter.menuTitle`: dot when unread, title and time, the
/// body (or subtitle), the workspace title; at most three lines of about 280 pt.
fn menu_bar_title(state: &AppState, record: &Record) -> String {
    let dot = if record.is_read { "  " } else { "● " };
    let detail = if record.content.body.is_empty() {
        &record.content.subtitle
    } else {
        &record.content.body
    };
    let workspace = state
        .workspaces
        .iter()
        .find(|workspace| workspace.uuid == record.workspace_id)
        .map(|workspace| workspace.name.as_str())
        .unwrap_or_default();
    let mut lines = vec![format!(
        "{dot}{}  {}",
        record.content.title,
        local_time(&record.created_at, "%H:%M")
    )];
    lines.extend(
        [detail.as_str(), workspace]
            .into_iter()
            .filter(|text| !text.is_empty())
            .map(str::to_owned),
    );
    lines
        .iter()
        .take(3)
        .map(|line| bounded(line, 48))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Upstream `SidebarWorkspaceRowCommands.notificationMenuTitle`: dot when unread, date and
/// time, title, then the body (or subtitle) on a second line.
fn row_title(record: &Record) -> String {
    let dot = if record.is_read { "" } else { "• " };
    let time = local_time(&record.created_at, "%b %-d, %Y, %H:%M");
    let title = bounded(&record.content.title, 80);
    let detail = bounded(
        if record.content.body.is_empty() {
            &record.content.subtitle
        } else {
            &record.content.body
        },
        120,
    );
    let first = if title.is_empty() {
        format!("{dot}{time}")
    } else {
        format!("{dot}{time}  {title}")
    };
    if detail.is_empty() {
        first
    } else {
        format!("{first}\n{detail}")
    }
}

/// An entry that opens one notification through `win.open-notification`.
fn open_item(label: &str, record: &Record) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(&menu_text(label)), None);
    item.set_action_and_target_value(
        Some("win.open-notification"),
        Some(&record.id.to_string().to_variant()),
    );
    item
}

/// Enable a window action of the first application window that has it.
fn set_enabled(state: &AppState, name: &str, enabled: bool) {
    for window in state.gtk_app.windows() {
        if let Some(action) = window
            .downcast_ref::<gtk4::ApplicationWindow>()
            .and_then(|window| window.lookup_action(name))
            .and_then(|action| action.downcast::<gio::SimpleAction>().ok())
        {
            action.set_enabled(enabled);
        }
    }
}

/// Follow the inbox and the workspace unread marks: greyed entries as upstream, the state line
/// and the six most recent notifications.
pub fn refresh(state: &AppState) {
    let Some(entry) = &state.notification_menu else {
        return;
    };
    let unread = unread_count(state);
    let any = !state.inbox.records.is_empty() || unread > 0;
    set_enabled(state, "jump-to-unread", unread > 0);
    set_enabled(state, "mark-all-read", unread > 0);
    set_enabled(state, "clear-all-notifications", any);
    let recent: Vec<_> = state
        .inbox
        .records
        .iter()
        .rev()
        .take(RECENT_LIMIT)
        .map(|record| (record, menu_bar_title(state, record)))
        .collect();
    let shown = format!(
        "{unread}\n{}",
        recent
            .iter()
            .map(|(record, title)| format!("{}{title}", record.id))
            .collect::<String>()
    );
    if *entry.shown.borrow() == shown {
        return;
    }
    *entry.shown.borrow_mut() = shown;
    let menu = &entry.menu;
    menu.remove_all();
    let hint = gio::Menu::new();
    hint.append(Some(&state_hint(unread)), Some("win.menu-disabled"));
    menu.append_section(None, &hint);
    if !recent.is_empty() {
        let list = gio::Menu::new();
        for (record, title) in &recent {
            list.append_item(&open_item(title, record));
        }
        menu.append_section(None, &list);
    }
    let commands = gio::Menu::new();
    for (label, action) in [
        ("Show Notifications", "win.notifications"),
        ("Jump to Latest Unread", "win.jump-to-unread"),
        ("Toggle Unread", "win.toggle-unread"),
        ("Mark All Read", "win.mark-all-read"),
        ("Clear All", "win.clear-all-notifications"),
    ] {
        commands.append(Some(label), Some(action));
    }
    menu.append_section(None, &commands);
}

/// Fill a workspace row menu's notification entries for the workspace at `index`, just before
/// it opens: Mark as Read / Unread and Clear Latest greyed as upstream, and its notifications
/// newest first (or a greyed « No Notifications »).
pub fn fill_row(state: &AppState, index: usize, actions: &gio::SimpleActionGroup, list: &gio::Menu) {
    let unread = crate::inbox_actions::workspace_is_unread(state, index);
    let uuid = state.workspaces[index].uuid;
    let records: Vec<_> = state
        .inbox
        .records
        .iter()
        .rev()
        .filter(|record| record.workspace_id == uuid)
        .collect();
    for (name, enabled) in [
        ("mark-read", unread),
        ("mark-unread", !unread),
        ("clear-latest-notification", !records.is_empty()),
    ] {
        if let Some(action) = actions
            .lookup_action(name)
            .and_then(|action| action.downcast::<gio::SimpleAction>().ok())
        {
            action.set_enabled(enabled);
        }
    }
    list.remove_all();
    if records.is_empty() {
        list.append(Some("No Notifications"), Some("win.menu-disabled"));
    }
    for record in records {
        list.append_item(&open_item(&row_title(record), record));
    }
}

/// Window actions behind the menu: the greyed placeholder, opening one notification, Toggle
/// Unread, Mark All Read and Clear All.
pub fn register_actions(window: &gtk4::ApplicationWindow, state: &crate::app_state::AppStateRef) {
    let disabled = gio::SimpleAction::new("menu-disabled", None);
    disabled.set_enabled(false);
    window.add_action(&disabled);
    let open = gio::SimpleAction::new("open-notification", Some(glib::VariantTy::STRING));
    open.connect_activate({
        let state = state.clone();
        move |_, parameter| {
            let Some(id) = parameter
                .and_then(|value| value.str())
                .and_then(|value| uuid::Uuid::parse_str(value).ok())
            else {
                return;
            };
            if let Err((code, _)) = crate::inbox_actions::handle(
                &mut state.borrow_mut(),
                crate::inbox::Action::Open(id),
            ) {
                crate::diagnostics::event(format_args!("notification.menu.open outcome={code}"));
            }
        }
    });
    window.add_action(&open);
    let toggle = gio::SimpleAction::new("toggle-unread", None);
    toggle.connect_activate({
        let state = state.clone();
        move |_, _| crate::inbox_actions::toggle_unread(&mut state.borrow_mut())
    });
    window.add_action(&toggle);
    let mark_all = gio::SimpleAction::new("mark-all-read", None);
    mark_all.connect_activate({
        let state = state.clone();
        move |_, _| {
            let mut s = state.borrow_mut();
            let _ = crate::inbox_actions::handle(
                &mut s,
                crate::inbox::Action::MarkRead {
                    id: None,
                    scope: crate::inbox::Scope::default(),
                    all: true,
                },
            );
            // Upstream `markAllRead` also clears the workspaces marked unread.
            for index in 0..s.workspaces.len() {
                s.clear_workspace_attention(index);
            }
        }
    });
    window.add_action(&mark_all);
    let clear_all = gio::SimpleAction::new("clear-all-notifications", None);
    clear_all.connect_activate({
        let state = state.clone();
        move |_, _| {
            let _ = crate::inbox_actions::handle(
                &mut state.borrow_mut(),
                crate::inbox::Action::Clear(crate::inbox::Scope::default()),
            );
        }
    });
    window.add_action(&clear_all);
}
