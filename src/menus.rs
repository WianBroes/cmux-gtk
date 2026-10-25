use gtk4::gio;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Register all GIO actions on the ApplicationWindow.
/// Actions are named "win.{action-name}" and can be invoked by buttons, menus, and shortcuts.
/// Per D-11, D-12: all menu items mirror existing keyboard shortcut actions.
pub fn register_actions(
    window: &gtk4::ApplicationWindow,
    state: Rc<RefCell<crate::app_state::AppState>>,
    sidebar: &gtk4::Box,
    app: &gtk4::Application,
) {
    if !crate::browser::agent_browser_available() {
        eprintln!(
            "cmux: browser tabs disabled; install agent-browser with: npm install -g agent-browser && agent-browser install"
        );
    }
    // --- File section actions ---

    // win.new-workspace (D-01, D-05)
    let action = gio::SimpleAction::new("new-workspace", None);
    action.connect_activate({
        let state = state.clone();
        let app = app.clone();
        move |_, _| {
            crate::shortcuts::handle_new_workspace(&state, &app);
        }
    });
    window.add_action(&action);

    // win.project-palette — searchable project-defined actions for the active workspace.
    let action = gio::SimpleAction::new("project-palette", None);
    action.connect_activate({
        let state = state.clone();
        let window = window.downgrade();
        move |_, _| {
            if let Some(window) = window.upgrade() {
                crate::project_palette::show(&window, &state);
            }
        }
    });
    window.add_action(&action);

    // win.new-terminal-tab — sibling surface in the focused pane.
    let action = gio::SimpleAction::new("new-terminal-tab", None);
    action.connect_activate({
        let state = state.clone();
        move |_, _| crate::shortcuts::handle_new_terminal_tab(&state)
    });
    window.add_action(&action);

    // win.focus-pane — keep pointer-selected pane state aligned with GTK focus.
    let action = gio::SimpleAction::new("focus-pane", Some(&u64::static_variant_type()));
    action.connect_activate({
        let state = state.clone();
        move |_, parameter| {
            let Some(pane_id) = parameter.and_then(|value| value.get::<u64>()) else {
                return;
            };
            crate::shortcuts::handle_focus_pane(&state, pane_id);
        }
    });
    window.add_action(&action);

    // win.focus-terminal — a terminal was clicked; activate the pane that holds it now.
    let action = gio::SimpleAction::new("focus-terminal", Some(&String::static_variant_type()));
    action.connect_activate({
        let state = state.clone();
        move |_, parameter| {
            if let Some(surface) = terminal_parameter(parameter) {
                crate::shortcuts::handle_focus_terminal(&state, surface);
            }
        }
    });
    window.add_action(&action);

    // win.terminal-focused — keyboard focus entered a terminal; mark its notifications read.
    let action = gio::SimpleAction::new("terminal-focused", Some(&String::static_variant_type()));
    action.connect_activate({
        let state = state.clone();
        move |_, parameter| {
            if let Some(surface) = terminal_parameter(parameter) {
                crate::inbox_actions::terminal_focused(&state, surface);
            }
        }
    });
    window.add_action(&action);

    // win.new-browser-tab — sibling surface in the focused pane.
    let action = gio::SimpleAction::new("new-browser-tab", None);
    action.set_enabled(crate::browser::agent_browser_available());
    action.connect_activate({
        let state = state.clone();
        move |_, _| crate::shortcuts::handle_browser_open(&state)
    });
    window.add_action(&action);

    // win.new-ssh-workspace
    let action = gio::SimpleAction::new("new-ssh-workspace", None);
    action.connect_activate({
        let state = state.clone();
        let app = app.clone();
        move |_, _| {
            crate::shortcuts::handle_new_ssh_workspace(&state, &app);
        }
    });
    window.add_action(&action);

    // win.browser-open (D-07)
    let action = gio::SimpleAction::new("browser-open", None);
    action.set_enabled(crate::browser::agent_browser_available());
    action.connect_activate({
        let state = state.clone();
        move |_, _| {
            crate::shortcuts::handle_browser_open(&state);
        }
    });
    window.add_action(&action);

    // win.close-surface-tab — close one terminal/browser tab in its pane.
    let action = gio::SimpleAction::new("close-surface-tab", Some(&String::static_variant_type()));
    action.connect_activate({
        let state = state.clone();
        let app = app.clone();
        move |_, parameter| {
            let Some(uuid) = parameter
                .and_then(|value| value.str())
                .and_then(|value| uuid::Uuid::parse_str(value).ok())
            else {
                return;
            };
            crate::shortcuts::handle_close_surface_tab(&state, &app, uuid);
        }
    });
    window.add_action(&action);

    // win.tab-close-left / -right / -others — close tabs around one surface inside its pane.
    for (name, scope) in [
        ("tab-close-left", crate::tab_actions::CloseScope::Left),
        ("tab-close-right", crate::tab_actions::CloseScope::Right),
        ("tab-close-others", crate::tab_actions::CloseScope::Others),
    ] {
        let action = gio::SimpleAction::new(name, Some(&String::static_variant_type()));
        action.connect_activate({
            let state = state.clone();
            move |_, parameter| {
                let Some(uuid) = parameter.and_then(|value| value.str()).map(str::to_owned)
                else {
                    return;
                };
                let mut s = state.borrow_mut();
                let Some(index) = crate::tab_actions::workspace_of(&s, &uuid) else {
                    return;
                };
                let _ = crate::tab_actions::close_around(&mut s, index, &uuid, scope);
            }
        });
        window.add_action(&action);
    }

    // win.tab-rename — prompt for a new custom name on one terminal tab.
    let action = gio::SimpleAction::new("tab-rename", Some(&String::static_variant_type()));
    action.connect_activate({
        let state = state.clone();
        move |_, parameter| {
            let Some(uuid) = parameter.and_then(|value| value.str()).map(str::to_owned) else {
                return;
            };
            let s = state.borrow();
            let Some(index) = crate::tab_actions::workspace_of(&s, &uuid) else {
                return;
            };
            let _ = s.split_engines[index].begin_tab_rename(&uuid);
        }
    });
    window.add_action(&action);

    // win.tab-clear-name — drop a tab's custom name and show its program title again.
    let action = gio::SimpleAction::new("tab-clear-name", Some(&String::static_variant_type()));
    action.connect_activate({
        let state = state.clone();
        move |_, parameter| {
            let Some(uuid) = parameter.and_then(|value| value.str()).map(str::to_owned) else {
                return;
            };
            let s = state.borrow();
            let Some(index) = crate::tab_actions::workspace_of(&s, &uuid) else {
                return;
            };
            let _ = s.split_engines[index].set_custom_title(&uuid, None);
        }
    });
    window.add_action(&action);

    // win.tab-toggle-zoom — fill the workspace with the tab's pane, or leave zoom.
    let action = gio::SimpleAction::new("tab-toggle-zoom", Some(&String::static_variant_type()));
    action.connect_activate({
        let state = state.clone();
        move |_, parameter| {
            let Some(uuid) = parameter.and_then(|value| value.str()).map(str::to_owned) else {
                return;
            };
            let mut s = state.borrow_mut();
            let Some(index) = crate::tab_actions::workspace_of(&s, &uuid) else {
                return;
            };
            let Some(engine) = s.split_engines.get_mut(index) else {
                return;
            };
            let Some((pane_id, _)) = engine.surface_location(&uuid) else {
                return;
            };
            engine.focus_surface(&uuid);
            engine.toggle_zoom(pane_id);
        }
    });
    window.add_action(&action);

    // GtkNotebook pointer reordering mutates its local surface model before asking the
    // application to persist the resulting workspace topology.
    let action = gio::SimpleAction::new("surface-tabs-changed", None);
    action.connect_activate({
        let state = state.clone();
        move |_, _| state.borrow().trigger_session_save()
    });
    window.add_action(&action);

    // Native tab drag destination. Payload is internal and contains only a stable UUID,
    // session-local pane id and a fixed direction selected by the pane drop target.
    let action = gio::SimpleAction::new("surface-drop", Some(&String::static_variant_type()));
    action.connect_activate({
        let state = state.clone();
        move |_, parameter| {
            let Some(payload) = parameter.and_then(|value| value.str()) else {
                return;
            };
            let mut fields = payload.split('|');
            let (Some(surface), Some(pane), Some(direction), Some(position), None) = (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            ) else {
                return;
            };
            let (Ok(surface), Ok(pane)) = (uuid::Uuid::parse_str(surface), pane.parse::<u64>())
            else {
                return;
            };
            let mut state = state.borrow_mut();
            let Some(source_index) = state
                .split_engines
                .iter()
                .position(|engine| engine.find_pane_id_by_uuid(&surface.to_string()).is_some())
            else {
                return;
            };
            let Some(destination_index) = state
                .split_engines
                .iter()
                .position(|engine| engine.contains_pane(pane))
            else {
                return;
            };
            let destination_workspace = state.workspaces[destination_index].uuid;
            let result = if direction == "center" {
                let Ok(position) = position.parse::<usize>() else {
                    return;
                };
                state
                    .move_surface_between_workspaces(
                        surface,
                        destination_workspace,
                        Some(pane),
                        Some(position),
                        true,
                    )
                    .map(|_| ())
            } else {
                if source_index != destination_index
                    && state
                        .move_surface_between_workspaces(
                            surface,
                            destination_workspace,
                            Some(pane),
                            None,
                            false,
                        )
                        .is_err()
                {
                    return;
                }
                let Some(index) = state
                    .workspaces
                    .iter()
                    .position(|workspace| workspace.uuid == destination_workspace)
                else {
                    return;
                };
                let direction = match direction {
                    "left" => crate::split_engine::FocusDirection::Left,
                    "right" => crate::split_engine::FocusDirection::Right,
                    "up" => crate::split_engine::FocusDirection::Up,
                    "down" => crate::split_engine::FocusDirection::Down,
                    _ => return,
                };
                state.split_engines[index]
                    .drag_surface_to_split(surface, pane, direction)
                    .map(|_| {
                        state.switch_to_index(index);
                        state.trigger_session_save();
                    })
            };
            if let Err(message) = result {
                crate::diagnostics::record(
                    "surface.drop_rejected",
                    serde_json::json!({"reason": message}),
                );
            }
        }
    });
    window.add_action(&action);

    // Sidebar workspace rows accept the same tab drag currency and resolve the target's
    // focused pane at drop time.
    let action = gio::SimpleAction::new(
        "surface-workspace-drop",
        Some(&String::static_variant_type()),
    );
    action.connect_activate({
        let state = state.clone();
        move |_, parameter| {
            let Some(payload) = parameter.and_then(|value| value.str()) else {
                return;
            };
            let Some((surface, workspace)) = payload.split_once('|') else {
                return;
            };
            let (Ok(surface), Ok(workspace)) = (
                uuid::Uuid::parse_str(surface),
                uuid::Uuid::parse_str(workspace),
            ) else {
                return;
            };
            if let Err(message) = state
                .borrow_mut()
                .move_surface_between_workspaces(surface, workspace, None, None, true)
            {
                crate::diagnostics::record(
                    "surface.workspace_drop_rejected",
                    serde_json::json!({"reason": message}),
                );
            }
        }
    });
    window.add_action(&action);

    // win.close-pane
    let action = gio::SimpleAction::new("close-pane", None);
    action.connect_activate({
        let state = state.clone();
        let app = app.clone();
        move |_, _| {
            crate::shortcuts::handle_close_pane(&state, &app);
        }
    });
    window.add_action(&action);

    let action = gio::SimpleAction::new(
        "context-menu-refresh",
        Some(gtk4::glib::VariantTy::BOOLEAN),
    );
    action.connect_activate({
        let window = window.downgrade();
        move |_, parameter| {
            // The one-line paste is offered only for text the action would actually join;
            // the caller reads the clipboard and passes whether it spans several lines.
            let Some(window) = window.upgrade() else {
                return;
            };
            let Some(paste_one_line) = window
                .lookup_action("paste-one-line")
                .and_then(|action| action.downcast::<gio::SimpleAction>().ok())
            else {
                return;
            };
            paste_one_line.set_enabled(parameter.and_then(|p| p.get::<bool>()).unwrap_or(false));
        }
    });
    window.add_action(&action);

    // win.close-workspace
    let action = gio::SimpleAction::new("close-workspace", None);
    action.connect_activate({
        let state = state.clone();
        let app = app.clone();
        move |_, _| {
            crate::shortcuts::handle_close_workspace(&state, &app);
        }
    });
    window.add_action(&action);

    // --- Edit section actions ---

    // win.copy (Ctrl+Shift+C) -- invoke Ghostty's copy_to_clipboard binding action
    let action = gio::SimpleAction::new("copy", None);
    action.connect_activate({
        let state = state.clone();
        move |_, _| {
            let s = state.borrow();
            if let Some(engine) = s.active_split_engine() {
                if let Some(pane_id) = engine.root.find_active_pane_id() {
                    if let Some(surface) = engine.root.find_surface_for_pane(pane_id) {
                        let action_str = b"copy_to_clipboard";
                        unsafe {
                            crate::ghostty::ffi::ghostty_surface_binding_action(
                                surface,
                                action_str.as_ptr() as *const _,
                                action_str.len(),
                            );
                        }
                    }
                }
            }
        }
    });
    window.add_action(&action);

    // win.paste (Ctrl+Shift+V) -- invoke Ghostty's paste_from_clipboard binding action
    let action = gio::SimpleAction::new("paste", None);
    action.connect_activate({
        let state = state.clone();
        move |_, _| {
            let s = state.borrow();
            if let Some(engine) = s.active_split_engine() {
                if let Some(pane_id) = engine.root.find_active_pane_id() {
                    if let Some(surface) = engine.root.find_surface_for_pane(pane_id) {
                        let action_str = b"paste_from_clipboard";
                        unsafe {
                            crate::ghostty::ffi::ghostty_surface_binding_action(
                                surface,
                                action_str.as_ptr() as *const _,
                                action_str.len(),
                            );
                        }
                    }
                }
            }
        }
    });
    window.add_action(&action);

    // win.paste-one-line -- join a multi-line clipboard into one literal line, without Enter.
    let action = gio::SimpleAction::new("paste-one-line", None);
    action.set_enabled(false);
    action.connect_activate({
        let state = state.clone();
        let window = window.downgrade();
        move |_, _| {
            let Some(window) = window.upgrade() else {
                return;
            };
            let clipboard = window.clipboard();
            let state = state.clone();
            gtk4::glib::MainContext::default().spawn_local(async move {
                let Ok(Some(text)) = clipboard.read_text_future().await else {
                    return;
                };
                let text = text.to_string();
                if !crate::paste_line_join::spans_multiple_lines(&text) {
                    return;
                }
                let joined = crate::paste_line_join::joined(&text);
                let surface = {
                    let s = state.borrow();
                    s.active_split_engine().and_then(|engine| {
                        engine.root.find_active_pane_id().and_then(|pane_id| {
                            engine.root.find_surface_for_pane(pane_id)
                        })
                    })
                };
                if let Some(surface) = surface {
                    // SAFETY: GTK main thread, surface read from the live model just above,
                    // and the model borrow released before native input.
                    let _ = unsafe { crate::ghostty::text::send_literal(surface, &joined) };
                }
            });
        }
    });
    window.add_action(&action);

    // win.reset-terminal -- Ghostty's reset binding on the active pane.
    let action = gio::SimpleAction::new("reset-terminal", None);
    action.connect_activate({
        let state = state.clone();
        move |_, _| {
            let s = state.borrow();
            if let Some(engine) = s.active_split_engine() {
                if let Some(pane_id) = engine.root.find_active_pane_id() {
                    if let Some(surface) = engine.root.find_surface_for_pane(pane_id) {
                        let action_str = b"reset";
                        unsafe {
                            crate::ghostty::ffi::ghostty_surface_binding_action(
                                surface,
                                action_str.as_ptr() as *const _,
                                action_str.len(),
                            );
                        }
                    }
                }
            }
        }
    });
    window.add_action(&action);

    // win.copy-ids -- copy the active workspace, pane and surface identities for socket clients.
    let action = gio::SimpleAction::new("copy-ids", None);
    action.connect_activate({
        let state = state.clone();
        let window = window.downgrade();
        move |_, _| {
            let Some(window) = window.upgrade() else {
                return;
            };
            let text = {
                let s = state.borrow();
                let Some(workspace) = s.workspaces.get(s.active_index) else {
                    return;
                };
                let Some(engine) = s.active_split_engine() else {
                    return;
                };
                let pane_id = engine.active_pane_id;
                let Some(surface_id) = engine.active_pane_uuid() else {
                    return;
                };
                let workspace_id = workspace.uuid.to_string();
                let workspace_ref = s.handles.ensure_ref(
                    crate::socket::handles::HandleKind::Workspace,
                    &workspace_id.to_lowercase(),
                );
                let surface_ref = s.handles.ensure_ref(
                    crate::socket::handles::HandleKind::Surface,
                    &surface_id.to_lowercase(),
                );
                format!(
                    "workspace_ref={workspace_ref}\nworkspace_id={}\npane_ref=pane:{pane_id}\npane_id=pane:{pane_id}\nsurface_ref={surface_ref}\nsurface_id={}",
                    workspace_id.to_uppercase(),
                    surface_id.to_uppercase(),
                )
            };
            window.clipboard().set_text(&text);
        }
    });
    window.add_action(&action);

    // Edit › Find on the focused terminal (`terminal_search`): Find… opens the bar, the others
    // drive Ghostty's search of that terminal.
    for (name, binding) in [
        ("find", "start_search"),
        ("find-next", "navigate_search:next"),
        ("find-previous", "navigate_search:previous"),
        ("hide-find-bar", "end_search"),
        ("use-selection-for-find", "search_selection"),
    ] {
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate({
            let state = state.clone();
            move |_, _| {
                let Some(area) = state.borrow().active_terminal_area() else {
                    return;
                };
                if binding == "start_search" {
                    crate::terminal_search::open(&area, None);
                }
                crate::terminal_search::binding(&area, binding);
            }
        });
        window.add_action(&action);
    }

    // win.preferences — terminal appearance settings.
    let action = gio::SimpleAction::new("preferences", None);
    action.connect_activate({
        let window = window.downgrade();
        let state = state.clone();
        move |_, _| {
            if let Some(window) = window.upgrade() {
                crate::preferences::show(&window, &state);
            }
        }
    });
    window.add_action(&action);

    let action = gio::SimpleAction::new("notifications", None);
    action.connect_activate({
        let window = window.downgrade();
        let state = state.clone();
        move |_, _| {
            if let Some(window) = window.upgrade() {
                crate::inbox_view::show(&window, &state);
            }
        }
    });
    window.add_action(&action);

    // --- View section actions ---

    // win.toggle-sidebar
    let action = gio::SimpleAction::new("toggle-sidebar", None);
    action.connect_activate({
        let state = state.clone();
        let sidebar = sidebar.clone();
        move |_, _| {
            let visible = sidebar.is_visible();
            sidebar.set_visible(!visible);
            if let Some(engine) = state.borrow_mut().active_split_engine_mut() {
                engine.focus_active_surface();
            }
        }
    });
    window.add_action(&action);

    // win.toggle-right-sidebar — the right sidebar (Files panel), upstream ⌘⌥B.
    let action = gio::SimpleAction::new("toggle-right-sidebar", None);
    action.connect_activate({
        let state = state.clone();
        move |_, _| {
            state.borrow().toggle_right_sidebar();
            if let Some(engine) = state.borrow_mut().active_split_engine_mut() {
                engine.focus_active_surface();
            }
        }
    });
    window.add_action(&action);

    // win.focus-right-sidebar — focus toggle into the panel, upstream ⌘⇧E.
    let action = gio::SimpleAction::new("focus-right-sidebar", None);
    action.connect_activate({
        let state = state.clone();
        move |_, _| {
            if state.borrow().right_sidebar_tree_focused() {
                if let Some(engine) = state.borrow_mut().active_split_engine_mut() {
                    engine.focus_active_surface();
                }
            } else {
                state.borrow().focus_right_sidebar();
            }
        }
    });
    window.add_action(&action);

    // win.split-right
    let action = gio::SimpleAction::new("split-right", None);
    action.connect_activate({
        let state = state.clone();
        move |_, _| {
            crate::shortcuts::handle_split(&state, false);
        }
    });
    window.add_action(&action);

    // win.split-down
    let action = gio::SimpleAction::new("split-down", None);
    action.connect_activate({
        let state = state.clone();
        move |_, _| {
            crate::shortcuts::handle_split(&state, true);
        }
    });
    window.add_action(&action);

    // win.rename-workspace
    let action = gio::SimpleAction::new("rename-workspace", None);
    action.connect_activate({
        let state = state.clone();
        move |_, _| {
            let (active_index, sidebar_list) = {
                let s = state.borrow();
                (s.active_index, s.sidebar_list.clone())
            };
            crate::sidebar::start_inline_rename(&sidebar_list, active_index, state.clone());
        }
    });
    window.add_action(&action);

    // win.focus-back / win.focus-forward — the titlebar arrows (upstream Focus Back / Forward),
    // enabled only while their direction leads somewhere.
    let back = gio::SimpleAction::new("focus-back", None);
    let forward = gio::SimpleAction::new("focus-forward", None);
    for (action, direction) in [
        (&back, crate::focus_history::Direction::Back),
        (&forward, crate::focus_history::Direction::Forward),
    ] {
        action.set_enabled(false);
        action.connect_activate({
            let state = state.clone();
            move |_, _| {
                let item = state.borrow().focus_history_items(direction).into_iter().next();
                if let Some(item) = item {
                    go_focus_history(&state, &item);
                }
            }
        });
        window.add_action(action);
    }
    state.borrow_mut().focus_history_actions = Some((back, forward));
    // The launch terminal took focus before `win.terminal-focused` existed: record it now, so the
    // first move away can come back to it.
    {
        let mut s = state.borrow_mut();
        let index = s.active_index;
        let surface = s
            .split_engines
            .get(index)
            .and_then(|engine| engine.active_pane_uuid())
            .and_then(|uuid| uuid::Uuid::parse_str(&uuid).ok());
        if let Some(surface) = surface {
            s.record_focus(index, surface);
        }
    }

    // win.focus-history-go(u64) — a row of an arrow's right-click menu, by history position.
    let action = gio::SimpleAction::new("focus-history-go", Some(&u64::static_variant_type()));
    action.connect_activate({
        let state = state.clone();
        move |_, parameter| {
            let Some(position) = parameter.and_then(|value| value.get::<u64>()) else {
                return;
            };
            let item = {
                let s = state.borrow();
                [
                    crate::focus_history::Direction::Back,
                    crate::focus_history::Direction::Forward,
                ]
                .into_iter()
                .flat_map(|direction| s.focus_history_items(direction))
                .find(|item| item.index as u64 == position)
            };
            if let Some(item) = item {
                go_focus_history(&state, &item);
            }
        }
    });
    window.add_action(&action);

    // win.edit-workspace-description — the in-app editor for the sidebar description
    // (upstream's "Edit workspace description").
    let action = gio::SimpleAction::new("edit-workspace-description", None);
    action.connect_activate({
        let state = state.clone();
        let window = window.downgrade();
        move |_, _| {
            let (Some(window), Some(uuid)) = (
                window.upgrade(),
                state.borrow().active_workspace().map(|ws| ws.uuid),
            ) else {
                return;
            };
            crate::workspace_description::show(&window, &state, uuid);
        }
    });
    window.add_action(&action);

    // win.clear-workspace-description
    let action = gio::SimpleAction::new("clear-workspace-description", None);
    action.connect_activate({
        let state = state.clone();
        move |_, _| {
            let index = state.borrow().active_index;
            state.borrow_mut().set_workspace_description(index, None);
        }
    });
    window.add_action(&action);

    // --- Help section actions ---

    // win.keyboard-shortcuts (D-14)
    let action = gio::SimpleAction::new("keyboard-shortcuts", None);
    action.connect_activate({
        let window_weak = window.downgrade();
        move |_, _| {
            if let Some(win) = window_weak.upgrade() {
                let sw = build_shortcuts_window();
                sw.set_transient_for(Some(&win));
                sw.present();
            }
        }
    });
    window.add_action(&action);

    // win.about (D-15)
    let action = gio::SimpleAction::new("about", None);
    action.connect_activate({
        let window_weak = window.downgrade();
        move |_, _| {
            if let Some(win) = window_weak.upgrade() {
                let about = gtk4::AboutDialog::builder()
                    .program_name("cmux")
                    .version(env!("CARGO_PKG_VERSION"))
                    .comments("GPU-accelerated terminal multiplexer for Linux")
                    .website("https://github.com/manaflow-ai/cmux")
                    .license_type(gtk4::License::MitX11)
                    .transient_for(&win)
                    .modal(true)
                    .build();
                about.present();
            }
        }
    });
    window.add_action(&action);

    // app.quit
    let quit_action = gio::SimpleAction::new("quit", None);
    quit_action.connect_activate({
        let app = app.clone();
        move |_, _| {
            app.quit();
        }
    });
    app.add_action(&quit_action);

    // --- Browser-specific actions (D-09) ---

    // win.open-external-browser -- opens current browser pane URL in xdg-open
    // Disabled until BrowserManager exposes current_url() (wired in Plan 03)
    let action = gio::SimpleAction::new("open-external-browser", None);
    action.set_enabled(false); // TODO: enable when BrowserManager.current_url() is available
    window.add_action(&action);

    // win.copy-url -- copies current browser pane URL to clipboard
    // Disabled until BrowserManager exposes current_url() (wired in Plan 03)
    let action = gio::SimpleAction::new("copy-url", None);
    action.set_enabled(false); // TODO: enable when BrowserManager.current_url() is available
    window.add_action(&action);
}

/// Move to a history position, then record again once GTK has delivered the focus changes the
/// move caused (they arrive deferred while the state is borrowed here).
/// The terminal identity carried by `win.focus-terminal` and `win.terminal-focused`.
fn terminal_parameter(parameter: Option<&gtk4::glib::Variant>) -> Option<uuid::Uuid> {
    uuid::Uuid::parse_str(&parameter?.get::<String>()?).ok()
}

fn go_focus_history(state: &crate::app_state::AppStateRef, item: &crate::focus_history::Item) {
    if state.borrow_mut().navigate_focus_history(item) {
        let state = state.clone();
        gtk4::glib::idle_add_local_once(move || {
            let mut s = state.borrow_mut();
            s.focus_history.suppressed = false;
            s.refresh_focus_history_actions();
        });
    }
}

/// Register menu accelerators from the resolved shortcut map plus fixed nonconfigurable actions.
/// GIO accelerators can activate commands, so they must agree with capture-phase dispatch.
pub fn register_accels(app: &gtk4::Application, shortcuts: &crate::config::ShortcutMap) {
    use crate::config::ShortcutAction;
    for (action, name) in [
        (ShortcutAction::NewWorkspace, "win.new-workspace"),
        (ShortcutAction::CloseWorkspace, "win.close-workspace"),
        (ShortcutAction::NewSshWorkspace, "win.new-ssh-workspace"),
        (ShortcutAction::BrowserOpen, "win.browser-open"),
        (ShortcutAction::ClosePane, "win.close-pane"),
        (ShortcutAction::ToggleSidebar, "win.toggle-sidebar"),
        (ShortcutAction::ToggleRightSidebar, "win.toggle-right-sidebar"),
        (ShortcutAction::FocusRightSidebar, "win.focus-right-sidebar"),
        (ShortcutAction::SplitRight, "win.split-right"),
        (ShortcutAction::SplitDown, "win.split-down"),
        (ShortcutAction::RenameWorkspace, "win.rename-workspace"),
        (ShortcutAction::FocusBack, "win.focus-back"),
        (ShortcutAction::FocusForward, "win.focus-forward"),
    ] {
        let accelerator = shortcuts.accelerator_for(action);
        let accelerators: Vec<&str> = accelerator.as_deref().into_iter().collect();
        app.set_accels_for_action(name, &accelerators);
    }
    for (name, action) in crate::shortcuts::MENU_ACTIONS {
        let accelerator = shortcuts.accelerator_for(*action);
        let accelerators: Vec<&str> = accelerator.as_deref().into_iter().collect();
        app.set_accels_for_action(&format!("win.{name}"), &accelerators);
    }
    app.set_accels_for_action("win.new-terminal-tab", &["<Ctrl>t"]);
    app.set_accels_for_action("win.new-browser-tab", &["<Ctrl><Shift>l"]);
    app.set_accels_for_action("win.copy", &["<Ctrl><Shift>c"]);
    app.set_accels_for_action("win.paste", &["<Ctrl><Shift>v"]);
    app.set_accels_for_action("win.find", &["<Ctrl>f"]);
    app.set_accels_for_action("win.preferences", &["<Ctrl>comma"]);
    app.set_accels_for_action("win.notifications", &["<Ctrl><Shift>i"]);
    app.set_accels_for_action("app.quit", &["<Ctrl>q"]);
    app.set_accels_for_action("win.jump-to-unread", &["<Ctrl><Shift>u"]);
    app.set_accels_for_action("win.toggle-unread", &["<Ctrl><Alt>u"]);
}

/// The menu bar's Notifications menu, whose entries `notification_menu::refresh` rebuilds.
pub fn notifications_submenu(bar: &gio::Menu) -> Option<gio::Menu> {
    (0..bar.n_items())
        .find(|&index| {
            bar.item_attribute_value(index, "label", None)
                .and_then(|label| label.get::<String>())
                .as_deref()
                == Some("Notifications")
        })
        .and_then(|index| bar.item_link(index, "submenu"))
        .and_then(|menu| menu.downcast::<gio::Menu>().ok())
}

/// Build the menu bar model: upstream's menus in their order (cmux, File, Edit, View,
/// Notifications, History, Window, Help; Cloud left out), upstream labels, our existing actions.
pub fn build_menu_bar() -> gio::Menu {
    fn submenu(bar: &gio::Menu, label: &str, sections: &[&[(&str, &str)]]) {
        let menu = gio::Menu::new();
        for rows in sections {
            let section = gio::Menu::new();
            for (row, action) in *rows {
                section.append(Some(row), Some(action));
            }
            menu.append_section(None, &section);
        }
        bar.append_submenu(Some(label), &menu);
    }
    let bar = gio::Menu::new();
    submenu(&bar, "cmux", &[
        &[("About cmux", "win.about")],
        &[("Settings…", "win.preferences")],
        &[("Quit cmux", "app.quit")],
    ]);
    submenu(&bar, "File", &[
        &[
            ("New Workspace", "win.new-workspace"),
            ("New SSH Workspace", "win.new-ssh-workspace"),
            ("New Tab (Terminal)", "win.new-terminal-tab"),
            ("New Tab (Browser)", "win.new-browser-tab"),
        ],
        &[("Command Palette…", "win.project-palette")],
        &[
            ("Close Tab", "win.close-tab"),
            ("Close Other Tabs in Pane", "win.close-other-tabs-in-pane"),
            ("Close Pane", "win.close-pane"),
            ("Close Workspace", "win.close-workspace"),
        ],
    ]);
    submenu(&bar, "Edit", &[&[("Copy", "win.copy"), ("Paste", "win.paste")]]);
    // Find ▸: Ctrl+G, Ctrl+Alt+G, Ctrl+Alt+Shift+F and Ctrl+E reach cmux only while they apply
    // (`shortcuts::find_key`); otherwise the terminal keeps them, so they are shown, not bound.
    let find = gio::Menu::new();
    for (label, action, accel) in [
        ("Find…", "win.find", None),
        ("Find Next", "win.find-next", Some("<Ctrl>g")),
        ("Find Previous", "win.find-previous", Some("<Ctrl><Alt>g")),
        ("Hide Find Bar", "win.hide-find-bar", Some("<Ctrl><Alt><Shift>f")),
        ("Use Selection for Find", "win.use-selection-for-find", Some("<Ctrl>e")),
    ] {
        let item = gio::MenuItem::new(Some(label), Some(action));
        if let Some(accel) = accel {
            item.set_attribute_value("accel", Some(&accel.to_variant()));
        }
        find.append_item(&item);
    }
    if let Some(edit) = bar
        .item_link(bar.n_items() - 1, "submenu")
        .and_then(|menu| menu.downcast::<gio::Menu>().ok())
    {
        edit.append_submenu(Some("Find"), &find);
    }
    submenu(&bar, "View", &[
        &[
            ("Toggle Left Sidebar", "win.toggle-sidebar"),
            ("Toggle Right Sidebar", "win.toggle-right-sidebar"),
            ("Toggle Right Sidebar Focus", "win.focus-right-sidebar"),
        ],
        &[
            ("Next Surface", "win.next-surface"),
            ("Previous Surface", "win.previous-surface"),
            ("Reorder Surface Left", "win.reorder-surface-left"),
            ("Reorder Surface Right", "win.reorder-surface-right"),
        ],
        &[
            ("Next Workspace", "win.next-workspace"),
            ("Previous Workspace", "win.previous-workspace"),
            ("Move Workspace Up", "win.move-workspace-up"),
            ("Move Workspace Down", "win.move-workspace-down"),
            ("Rename Tab…", "win.rename-tab"),
            ("Rename Workspace…", "win.rename-workspace"),
            ("Edit Workspace Description…", "win.edit-workspace-description"),
            ("Toggle Full Screen", "win.toggle-full-screen"),
            ("Toggle Focused Workspace's Group Collapse", "win.toggle-workspace-group"),
        ],
        &[
            ("Split Right", "win.split-right"),
            ("Split Down", "win.split-down"),
        ],
        &[
            ("Equalize Splits", "win.equalize-splits"),
            ("Resize Pane Left", "win.resize-pane-left"),
            ("Resize Pane Right", "win.resize-pane-right"),
            ("Resize Pane Up", "win.resize-pane-up"),
            ("Resize Pane Down", "win.resize-pane-down"),
        ],
        &[
            ("Workspace 1", "win.workspace-1"),
            ("Workspace 2", "win.workspace-2"),
            ("Workspace 3", "win.workspace-3"),
            ("Workspace 4", "win.workspace-4"),
            ("Workspace 5", "win.workspace-5"),
            ("Workspace 6", "win.workspace-6"),
            ("Workspace 7", "win.workspace-7"),
            ("Workspace 8", "win.workspace-8"),
            ("Workspace 9", "win.workspace-9"),
        ],
        &[
            ("Jump to Latest Unread", "win.jump-to-unread"),
            ("Show Notifications", "win.notifications"),
        ],
    ]);
    // Filled from the inbox by `notification_menu::refresh`.
    submenu(&bar, "Notifications", &[]);
    submenu(&bar, "History", &[&[
        ("Focus Back", "win.focus-back"),
        ("Focus Forward", "win.focus-forward"),
    ]]);
    // GtkWindow's own actions: upstream's Window menu holds the standard macOS window items.
    submenu(&bar, "Window", &[&[
        ("Minimize", "window.minimize"),
        ("Zoom", "window.toggle-maximized"),
    ]]);
    submenu(&bar, "Help", &[&[
        ("Keyboard Shortcuts", "win.keyboard-shortcuts"),
    ]]);
    bar
}

/// Row menu entries that act on the selected workspace: their `workspace.<name>` row action
/// selects the row's workspace, then runs `win.<name>` (`sidebar::attach_sidebar_context_menu`).
pub const SIDEBAR_SELECTING_ACTIONS: &[(&str, &str)] = &[
    ("Rename Workspace…", "rename-workspace"),
    ("Edit Workspace Description…", "edit-workspace-description"),
    ("Clear Workspace Description", "clear-workspace-description"),
    ("Close Workspace", "close-workspace"),
];

/// Build terminal pane context menu (D-08).
pub fn build_terminal_context_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let edit_section = gio::Menu::new();
    edit_section.append(Some("Copy"), Some("win.copy"));
    edit_section.append(Some("Paste"), Some("win.paste"));
    let paste_one_line = gio::MenuItem::new(Some("Paste as One Line"), Some("win.paste-one-line"));
    paste_one_line.set_attribute_value("hidden-when", Some(&"action-disabled".to_variant()));
    edit_section.append_item(&paste_one_line);
    menu.append_section(None, &edit_section);

    // Upstream labels: Split Horizontally stacks the new pane below, Split Vertically beside.
    let pane_section = gio::Menu::new();
    pane_section.append(Some("Split Horizontally"), Some("win.split-down"));
    pane_section.append(Some("Split Vertically"), Some("win.split-right"));
    pane_section.append(Some("Reset Terminal"), Some("win.reset-terminal"));
    menu.append_section(None, &pane_section);

    let identity_section = gio::Menu::new();
    identity_section.append(Some("Copy IDs"), Some("win.copy-ids"));
    menu.append_section(None, &identity_section);

    menu
}

/// Build browser preview pane context menu (D-09).
/// Includes Open in External Browser and Copy URL actions.
pub fn build_browser_context_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    menu.append(
        Some("Open in External Browser"),
        Some("win.open-external-browser"),
    );
    menu.append(Some("Copy URL"), Some("win.copy-url"));
    menu.append(Some("Close Pane"), Some("win.close-pane"));
    menu
}

/// Shortcuts listed in the GtkShortcutsWindow (D-14), by group.
const SHORTCUT_GROUPS: &[(&str, &[(&str, &str)])] = &[
    (
        "Workspaces",
        &[
            ("<Ctrl>n", "New Workspace"),
            ("<Ctrl><Shift>w", "Close Workspace"),
            ("<Ctrl>bracketright", "Next Workspace"),
            ("<Ctrl>bracketleft", "Previous Workspace"),
            ("<Ctrl><Shift>r", "Rename Workspace"),
            ("<Ctrl>1...9", "Switch to Workspace 1-9"),
        ],
    ),
    (
        "Panes",
        &[
            ("<Ctrl>d", "Split Right"),
            ("<Ctrl><Shift>d", "Split Down"),
            ("<Ctrl><Shift>x", "Close Pane"),
            ("<Ctrl><Shift>Left", "Focus Left"),
            ("<Ctrl><Shift>Right", "Focus Right"),
            ("<Ctrl><Shift>Up", "Focus Up"),
            ("<Ctrl><Shift>Down", "Focus Down"),
        ],
    ),
    (
        "Edit",
        &[
            ("<Ctrl><Shift>c", "Copy"),
            ("<Ctrl><Shift>v", "Paste"),
            ("<Ctrl>f", "Find"),
        ],
    ),
    (
        "View",
        &[
            ("<Ctrl>b", "Toggle Sidebar"),
            ("<Ctrl><Shift>b", "Open Browser"),
            ("<Ctrl><Shift>s", "New SSH Workspace"),
        ],
    ),
    ("General", &[("<Ctrl>q", "Quit")]),
];

/// Build GtkShortcutsWindow (D-14) with all shortcuts grouped by category.
/// The window keeps its search bar and section stack as its own child: replacing that child
/// (set_child) frees them, and GTK then crashes when the window is closed. Before GTK 4.14,
/// sections can only be added through GtkBuildable, hence the UI description.
fn build_shortcuts_window() -> gtk4::ShortcutsWindow {
    let escape = |text: &str| gtk4::glib::markup_escape_text(text).to_string();
    let mut ui = String::from(
        "<interface><object class=\"GtkShortcutsWindow\" id=\"window\"><child>\
         <object class=\"GtkShortcutsSection\">\
         <property name=\"section-name\">shortcuts</property>",
    );
    for (group, shortcuts) in SHORTCUT_GROUPS {
        ui += &format!(
            "<child><object class=\"GtkShortcutsGroup\"><property name=\"title\">{}</property>",
            escape(group)
        );
        for (accel, title) in *shortcuts {
            ui += &format!(
                "<child><object class=\"GtkShortcutsShortcut\">\
                 <property name=\"accelerator\">{}</property>\
                 <property name=\"title\">{}</property></object></child>",
                escape(accel),
                escape(title)
            );
        }
        ui += "</object></child>";
    }
    ui += "</object></child></object></interface>";

    gtk4::Builder::from_string(&ui)
        .object("window")
        .expect("shortcuts window UI defines \"window\"")
}
