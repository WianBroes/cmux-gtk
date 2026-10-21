//! Application shortcut installation and workspace/pane action entry points.

pub use crate::browser::ui::{handle_browser_close, handle_browser_open, restore_browser_tabs};

use crate::app_state::AppState;
use crate::config::ShortcutAction;
use crate::split_engine::{FocusDirection, ResizeDirection};
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Install all cmux keyboard shortcuts on the application window.
///
/// Uses PropagationPhase::Capture (parent -> child) so the window controller fires
/// BEFORE Ghostty's per-GLArea EventControllerKey. Without capture phase, Ghostty
/// eats Ctrl+D, Ctrl+N, etc. (per RESEARCH.md Pattern 4 and Anti-patterns).
///
/// Shortcut bindings are driven by ShortcutMap (config-driven, D-06).
pub fn install_shortcuts(
    window: &gtk4::ApplicationWindow,
    state: Rc<RefCell<AppState>>,
    sidebar: &gtk4::Box,
    app: &gtk4::Application,
    shortcut_map: std::rc::Rc<std::cell::RefCell<crate::config::ShortcutMap>>,
) {
    let key_ctrl = gtk4::EventControllerKey::new();
    // CRITICAL: Capture phase -- fires before GLArea key handlers.
    key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);

    let sidebar_clone = sidebar.clone();
    let app_clone = app.clone();

    // One entry point per shortcut action: the key handler below and the menu bar's actions
    // (`win.<action>`, see `add_menu_actions`) run the same code.
    let run: Rc<dyn Fn(ShortcutAction)> = Rc::new({
        let state = state.clone();
        let sidebar_clone = sidebar_clone.clone();
        let app_clone = app_clone.clone();
        move |action: ShortcutAction| {
            match action {
                // -- Workspace shortcuts --
                ShortcutAction::NewWorkspace => {
                    handle_new_workspace(&state, &app_clone);
                }
                ShortcutAction::CloseWorkspace => {
                    handle_close_workspace(&state, &app_clone);
                }
                ShortcutAction::NextWorkspace => {
                    state.borrow_mut().switch_next();
                }
                ShortcutAction::PrevWorkspace => {
                    state.borrow_mut().switch_prev();
                }
                action @ (ShortcutAction::MoveWorkspaceUp | ShortcutAction::MoveWorkspaceDown) => {
                    let (from, to) = {
                        let state = state.borrow();
                        let from = state.active_index;
                        let offset = if action == ShortcutAction::MoveWorkspaceUp {
                            -1
                        } else {
                            1
                        };
                        (from, state.adjacent_workspace_in_group(from, offset))
                    };
                    let changed =
                        to.is_some_and(|to| state.borrow_mut().reorder_workspace(from, to));
                    if changed {
                        crate::sidebar::rebuild_grouped_sidebar(&state);
                    }
                }
                ShortcutAction::ToggleWorkspaceGroup => {
                    let group = {
                        let state = state.borrow();
                        state
                            .active_workspace()
                            .and_then(|workspace| workspace.group_id)
                            .and_then(|id| {
                                state
                                    .workspace_groups
                                    .iter()
                                    .find(|group| group.id == id)
                                    .map(|group| (id, !group.collapsed))
                            })
                    };
                    if let Some((id, collapsed)) = group {
                        let _ = state.borrow_mut().update_workspace_group(
                            id,
                            None,
                            None,
                            Some(collapsed),
                            None,
                        );
                        crate::sidebar::rebuild_grouped_sidebar(&state);
                    }
                }
                ShortcutAction::RenameWorkspace => {
                    let (active_index, sidebar_list) = {
                        let s = state.borrow();
                        let idx = s.active_index;
                        let list = s.sidebar_list.clone();
                        (idx, list)
                    };
                    crate::sidebar::start_inline_rename(&sidebar_list, active_index, state.clone());
                }
                ShortcutAction::ToggleSidebar => {
                    let visible = sidebar_clone.is_visible();
                    sidebar_clone.set_visible(!visible);
                    if let Some(engine) = state.borrow_mut().active_split_engine_mut() {
                        engine.focus_active_surface();
                    }
                }
                ShortcutAction::ToggleRightSidebar => {
                    // Upstream ⌘⌥B: toggling never moves focus into the panel; ⌘⇧E below
                    // owns the focus hand-over.
                    state.borrow().toggle_right_sidebar();
                    if let Some(engine) = state.borrow_mut().active_split_engine_mut() {
                        engine.focus_active_surface();
                    }
                }
                ShortcutAction::FocusRightSidebar => {
                    // Upstream ⌘⇧E: toggle focus between the panel's tree and the terminal.
                    if state.borrow().right_sidebar_tree_focused() {
                        if let Some(engine) = state.borrow_mut().active_split_engine_mut() {
                            engine.focus_active_surface();
                        }
                    } else {
                        state.borrow().focus_right_sidebar();
                    }
                }
                // -- Pane split shortcuts --
                ShortcutAction::SplitRight => {
                    handle_split(&state, false);
                }
                ShortcutAction::SplitDown => {
                    handle_split(&state, true);
                }
                ShortcutAction::EqualizeSplits => {
                    let changed = {
                        let mut s = state.borrow_mut();
                        s.active_split_engine_mut()
                            .is_some_and(|engine| engine.equalize_splits())
                    };
                    if changed {
                        state.borrow().trigger_session_save();
                    }
                }
                action @ (ShortcutAction::ResizePaneLeft
                | ShortcutAction::ResizePaneRight
                | ShortcutAction::ResizePaneUp
                | ShortcutAction::ResizePaneDown) => {
                    let direction = match action {
                        ShortcutAction::ResizePaneLeft => ResizeDirection::Left,
                        ShortcutAction::ResizePaneRight => ResizeDirection::Right,
                        ShortcutAction::ResizePaneUp => ResizeDirection::Up,
                        ShortcutAction::ResizePaneDown => ResizeDirection::Down,
                        _ => unreachable!(),
                    };
                    let changed = {
                        let mut s = state.borrow_mut();
                        s.active_split_engine_mut()
                            .is_some_and(|engine| engine.resize_active_pane(direction))
                    };
                    if changed {
                        state.borrow().trigger_session_save();
                    }
                }
                ShortcutAction::ToggleSplitZoom => {
                    let mut s = state.borrow_mut();
                    if let Some(engine) = s.active_split_engine_mut() {
                        let pane_id = engine.active_pane_id;
                        engine.toggle_zoom(pane_id);
                    }
                }
                ShortcutAction::ClosePane => {
                    handle_close_pane(&state, &app_clone);
                }
                // -- Tab shortcuts: the focused pane's selected tab --
                action @ (ShortcutAction::CloseTab
                    | ShortcutAction::CloseOtherTabsInPane
                    | ShortcutAction::RenameTab) => {
                    let target = {
                        let s = state.borrow();
                        let Some(engine) = s.active_split_engine() else {
                            return;
                        };
                        let pane_id = engine.active_pane_id;
                        let Some(surface) = engine
                            .pane_info()
                            .into_iter()
                            .find(|pane| pane.id == pane_id)
                            .and_then(|pane| pane.selected_surface)
                        else {
                            return;
                        };
                        (s.active_index, surface)
                    };
                    let (active_index, surface) = target;
                    let uuid = surface.to_string();
                    match action {
                        ShortcutAction::CloseTab => {
                            handle_close_surface_tab(&state, &app_clone, surface);
                        }
                        ShortcutAction::CloseOtherTabsInPane => {
                            let _ = crate::tab_actions::close_around(
                                &mut state.borrow_mut(),
                                active_index,
                                &uuid,
                                crate::tab_actions::CloseScope::Others,
                            );
                        }
                        _ => {
                            let s = state.borrow();
                            if let Some(engine) = s.split_engines.get(active_index) {
                                let _ = engine.begin_tab_rename(&uuid);
                            }
                        }
                    }
                }
                // Upstream Next/Previous Surface wrap around the focused pane's tabs (bonsplit
                // `selectNextTab`); Reorder Surface Left/Right stop at the ends (`reorderSurface`).
                action @ (ShortcutAction::NextSurface
                    | ShortcutAction::PrevSurface
                    | ShortcutAction::MoveSurfaceLeft
                    | ShortcutAction::MoveSurfaceRight) => {
                    let mut s = state.borrow_mut();
                    let Some(engine) = s.active_split_engine_mut() else {
                        return;
                    };
                    let pane_id = engine.active_pane_id;
                    let Some(pane) = engine.pane_info().into_iter().find(|pane| pane.id == pane_id)
                    else {
                        return;
                    };
                    let Some(slot) = pane
                        .selected_surface
                        .and_then(|selected| pane.surface_ids.iter().position(|id| *id == selected))
                    else {
                        return;
                    };
                    let (count, last) = (pane.surface_ids.len(), pane.surface_ids.len() - 1);
                    match action {
                        ShortcutAction::NextSurface | ShortcutAction::PrevSurface => {
                            let next = if action == ShortcutAction::NextSurface {
                                (slot + 1) % count
                            } else {
                                (slot + count - 1) % count
                            };
                            engine.focus_surface(&pane.surface_ids[next].to_string());
                        }
                        _ => {
                            let target = if action == ShortcutAction::MoveSurfaceLeft {
                                slot.saturating_sub(1)
                            } else {
                                (slot + 1).min(last)
                            };
                            if target != slot
                                && engine.reorder_surface(pane.surface_ids[slot], target).is_ok()
                            {
                                s.trigger_session_save();
                            }
                        }
                    }
                }
                // -- SSH workspace shortcut --
                ShortcutAction::NewSshWorkspace => {
                    handle_new_ssh_workspace(&state, &app_clone);
                }
                // -- Focus direction shortcuts --
                ShortcutAction::FocusLeft => {
                    handle_focus_direction(&state, FocusDirection::Left);
                }
                ShortcutAction::FocusRight => {
                    handle_focus_direction(&state, FocusDirection::Right);
                }
                ShortcutAction::FocusUp => {
                    handle_focus_direction(&state, FocusDirection::Up);
                }
                ShortcutAction::FocusDown => {
                    handle_focus_direction(&state, FocusDirection::Down);
                }
                // -- Workspace number shortcuts --
                action @ (ShortcutAction::Workspace1
                    | ShortcutAction::Workspace2
                    | ShortcutAction::Workspace3
                    | ShortcutAction::Workspace4
                    | ShortcutAction::Workspace5
                    | ShortcutAction::Workspace6
                    | ShortcutAction::Workspace7
                    | ShortcutAction::Workspace8
                    | ShortcutAction::Workspace9) => {
                    let idx = match action {
                        ShortcutAction::Workspace1 => 0,
                        ShortcutAction::Workspace2 => 1,
                        ShortcutAction::Workspace3 => 2,
                        ShortcutAction::Workspace4 => 3,
                        ShortcutAction::Workspace5 => 4,
                        ShortcutAction::Workspace6 => 5,
                        ShortcutAction::Workspace7 => 6,
                        ShortcutAction::Workspace8 => 7,
                        ShortcutAction::Workspace9 => 8,
                        _ => unreachable!(),
                    };
                    state.borrow_mut().switch_to_index(idx);
                }
                // -- Browser shortcuts --
                ShortcutAction::FocusBack => {
                    let _ = sidebar_clone.activate_action("win.focus-back", None);
                }
                ShortcutAction::FocusForward => {
                    let _ = sidebar_clone.activate_action("win.focus-forward", None);
                }
                ShortcutAction::BrowserOpen => {
                    handle_browser_open(&state);
                }
                ShortcutAction::BrowserClose => {
                    handle_browser_close(&state);
                }
            }
        }
    });

    add_menu_actions(window, &state, &run);

    let dispatch: Rc<KeyDispatch> = Rc::new({
        let state = state.clone();
        let window = window.downgrade();
        let run = run.clone();
        move |keyval: gtk4::gdk::Key, mods: gtk4::gdk::ModifierType| {
            let action = shortcut_map.borrow().lookup(mods, keyval);
            match action {
                Some(action) => {
                    run(action);
                    gtk4::glib::Propagation::Stop
                }
                None if keyval == gtk4::gdk::Key::comma
                    && mods.intersection(
                        gtk4::gdk::ModifierType::CONTROL_MASK
                            | gtk4::gdk::ModifierType::SHIFT_MASK
                            | gtk4::gdk::ModifierType::ALT_MASK
                            | gtk4::gdk::ModifierType::SUPER_MASK,
                    ) == gtk4::gdk::ModifierType::CONTROL_MASK =>
                {
                    if let Some(window) = window.upgrade() {
                        crate::preferences::show(&window, &state);
                    }
                    gtk4::glib::Propagation::Stop
                }
                None if (keyval.to_lower() == gtk4::gdk::Key::i
                    || keyval.to_lower() == gtk4::gdk::Key::u)
                    && mods.intersection(
                        gtk4::gdk::ModifierType::CONTROL_MASK
                            | gtk4::gdk::ModifierType::SHIFT_MASK
                            | gtk4::gdk::ModifierType::ALT_MASK
                            | gtk4::gdk::ModifierType::SUPER_MASK,
                    ) == (gtk4::gdk::ModifierType::CONTROL_MASK
                        | gtk4::gdk::ModifierType::SHIFT_MASK) =>
                {
                    if keyval.to_lower() == gtk4::gdk::Key::i {
                        if let Some(window) = window.upgrade() {
                            crate::inbox_view::show(&window, &state);
                        }
                    } else if let Err((code, _)) = crate::inbox_actions::handle(
                        &mut state.borrow_mut(),
                        crate::inbox::Action::JumpToUnread,
                    ) {
                        crate::diagnostics::event(format_args!(
                            "notification.navigation outcome={code}"
                        ));
                    }
                    gtk4::glib::Propagation::Stop
                }
                None if keyval.to_lower() == gtk4::gdk::Key::u
                    && mods.intersection(
                        gtk4::gdk::ModifierType::CONTROL_MASK
                            | gtk4::gdk::ModifierType::SHIFT_MASK
                            | gtk4::gdk::ModifierType::ALT_MASK
                            | gtk4::gdk::ModifierType::SUPER_MASK,
                    ) == (gtk4::gdk::ModifierType::CONTROL_MASK
                        | gtk4::gdk::ModifierType::ALT_MASK) =>
                {
                    crate::inbox_actions::toggle_unread(&mut state.borrow_mut());
                    gtk4::glib::Propagation::Stop
                }
                None if keyval.to_lower() == gtk4::gdk::Key::p
                    && mods.intersection(
                        gtk4::gdk::ModifierType::CONTROL_MASK
                            | gtk4::gdk::ModifierType::SHIFT_MASK
                            | gtk4::gdk::ModifierType::ALT_MASK
                            | gtk4::gdk::ModifierType::SUPER_MASK,
                    ) == (gtk4::gdk::ModifierType::CONTROL_MASK
                        | gtk4::gdk::ModifierType::SHIFT_MASK) =>
                {
                    if let Some(window) = window.upgrade() {
                        crate::project_palette::show(&window, &state);
                    }
                    gtk4::glib::Propagation::Stop
                }
                // Everything else passes through to Ghostty.
                _ => find_key(&state, keyval, mods),
            }
        }
    });
    key_ctrl.connect_key_pressed({
        let dispatch = dispatch.clone();
        move |_ctrl, keyval, keycode, mods| {
            // Bracket and = shortcuts follow the key's place on a US keyboard (Wian, 2026-10-21):
            // on a Belgian layout those keys type ^, $ or -, and Shift turns ] into } everywhere.
            let result = dispatch(keyval, mods);
            match us_bracket_key(keycode) {
                Some(bracket) if result == gtk4::glib::Propagation::Proceed && bracket != keyval => {
                    dispatch(bracket, mods)
                }
                _ => result,
            }
        }
    });
    KEY_DISPATCH.with(|slot| *slot.borrow_mut() = Some(dispatch));

    window.add_controller(key_ctrl);
}

/// Window actions for the menu bar entries that only had a shortcut (`MENU_ACTIONS`), running
/// the same code as the key; plus Jump to Latest Unread, a fixed key outside the shortcut map.
fn add_menu_actions(
    window: &gtk4::ApplicationWindow,
    state: &Rc<RefCell<AppState>>,
    run: &Rc<dyn Fn(ShortcutAction)>,
) {
    for (name, action) in MENU_ACTIONS {
        let entry = gtk4::gio::SimpleAction::new(name, None);
        entry.connect_activate({
            let run = run.clone();
            move |_, _| run(*action)
        });
        window.add_action(&entry);
    }
    let entry = gtk4::gio::SimpleAction::new("jump-to-unread", None);
    entry.connect_activate({
        let state = state.clone();
        move |_, _| {
            if let Err((code, _)) = crate::inbox_actions::handle(
                &mut state.borrow_mut(),
                crate::inbox::Action::JumpToUnread,
            ) {
                crate::diagnostics::event(format_args!("notification.navigation outcome={code}"));
            }
        }
    });
    window.add_action(&entry);
}

/// Menu bar actions (`win.<name>`) backed by a shortcut action; `menus::register_accels` shows
/// each one's live shortcut.
pub const MENU_ACTIONS: &[(&str, ShortcutAction)] = &[
    ("next-workspace", ShortcutAction::NextWorkspace),
    ("previous-workspace", ShortcutAction::PrevWorkspace),
    ("move-workspace-up", ShortcutAction::MoveWorkspaceUp),
    ("move-workspace-down", ShortcutAction::MoveWorkspaceDown),
    ("toggle-workspace-group", ShortcutAction::ToggleWorkspaceGroup),
    ("workspace-1", ShortcutAction::Workspace1),
    ("workspace-2", ShortcutAction::Workspace2),
    ("workspace-3", ShortcutAction::Workspace3),
    ("workspace-4", ShortcutAction::Workspace4),
    ("workspace-5", ShortcutAction::Workspace5),
    ("workspace-6", ShortcutAction::Workspace6),
    ("workspace-7", ShortcutAction::Workspace7),
    ("workspace-8", ShortcutAction::Workspace8),
    ("workspace-9", ShortcutAction::Workspace9),
    ("close-tab", ShortcutAction::CloseTab),
    (
        "close-other-tabs-in-pane",
        ShortcutAction::CloseOtherTabsInPane,
    ),
    ("rename-tab", ShortcutAction::RenameTab),
    ("next-surface", ShortcutAction::NextSurface),
    ("previous-surface", ShortcutAction::PrevSurface),
    ("reorder-surface-left", ShortcutAction::MoveSurfaceLeft),
    ("reorder-surface-right", ShortcutAction::MoveSurfaceRight),
    ("equalize-splits", ShortcutAction::EqualizeSplits),
    ("resize-pane-left", ShortcutAction::ResizePaneLeft),
    ("resize-pane-right", ShortcutAction::ResizePaneRight),
    ("resize-pane-up", ShortcutAction::ResizePaneUp),
    ("resize-pane-down", ShortcutAction::ResizePaneDown),
    ("toggle-split-zoom", ShortcutAction::ToggleSplitZoom),
];

/// Find Next (Ctrl+G), Find Previous (Ctrl+Alt+G), Hide Find Bar (Ctrl+Alt+Shift+F) while the
/// focused terminal's find bar is open, Use Selection for Find (Ctrl+E) while it has a
/// selection (Wian, 2026-10-20: upstream's ⌘ keys as Ctrl). Otherwise the key stays the
/// terminal's (Ctrl+E end of line, Ctrl+G abort in the shell).
fn find_key(
    state: &Rc<RefCell<AppState>>,
    keyval: gtk4::gdk::Key,
    mods: gtk4::gdk::ModifierType,
) -> gtk4::glib::Propagation {
    use gtk4::gdk::{Key, ModifierType as M};
    let mods = mods & (M::CONTROL_MASK | M::SHIFT_MASK | M::ALT_MASK | M::SUPER_MASK);
    let binding = match (keyval.to_lower(), mods) {
        (Key::g, m) if m == M::CONTROL_MASK => "navigate_search:next",
        (Key::g, m) if m == M::CONTROL_MASK | M::ALT_MASK => "navigate_search:previous",
        (Key::f, m) if m == M::CONTROL_MASK | M::ALT_MASK | M::SHIFT_MASK => "end_search",
        (Key::e, m) if m == M::CONTROL_MASK => "search_selection",
        _ => return gtk4::glib::Propagation::Proceed,
    };
    let Some(area) = state.borrow().active_terminal_area() else {
        return gtk4::glib::Propagation::Proceed;
    };
    let applies = if binding == "search_selection" {
        crate::terminal_search::has_selection(&area)
    } else {
        crate::terminal_search::is_active(&area)
    };
    if !applies {
        return gtk4::glib::Propagation::Proceed;
    }
    crate::terminal_search::binding(&area, binding);
    gtk4::glib::Propagation::Stop
}

/// `=` / `[` / `]` for the X11 keycodes of their keys on a US keyboard (evdev 13/26/27 + 8).
fn us_bracket_key(keycode: u32) -> Option<gtk4::gdk::Key> {
    match keycode {
        21 => Some(gtk4::gdk::Key::equal),
        34 => Some(gtk4::gdk::Key::bracketleft),
        35 => Some(gtk4::gdk::Key::bracketright),
        _ => None,
    }
}

type KeyDispatch = dyn Fn(gtk4::gdk::Key, gtk4::gdk::ModifierType) -> gtk4::glib::Propagation;

thread_local! {
    /// The window's shortcut matcher, shared with `debug.shortcut.simulate` so a simulated
    /// combo takes the same matcher and dispatch order as a real key press (upstream:
    /// `AppDelegate.debugHandleCustomShortcut`).
    static KEY_DISPATCH: RefCell<Option<Rc<KeyDispatch>>> = const { RefCell::new(None) };
}

/// Run a `cmux.json`-style combo ("cmd+[", "ctrl+shift+t") through the shortcut matcher.
/// `Ok(true)` when a shortcut consumed it, `Ok(false)` when it would pass through to the
/// terminal, `Err` when the combo is not a key GTK knows.
pub fn simulate_shortcut(combo: &str) -> Result<bool, ()> {
    let (key, mods) = crate::config::parse_combo(combo).ok_or(())?;
    let dispatch = KEY_DISPATCH.with(|slot| slot.borrow().clone()).ok_or(())?;
    Ok(dispatch(key, mods) == gtk4::glib::Propagation::Stop)
}

/// Create a new workspace with an initial GLArea pane and add it to AppState + GtkStack.
pub fn handle_new_workspace(state: &Rc<RefCell<AppState>>, app: &gtk4::Application) {
    crate::workspace_dialog::show_workspace_dialog(app, state.clone());
}

/// Show close-workspace confirmation dialog. If confirmed, closes the active workspace.
pub fn handle_close_workspace(state: &Rc<RefCell<AppState>>, app: &gtk4::Application) {
    // Cannot close the last workspace.
    let (workspace_id, workspace_count) = {
        let s = state.borrow();
        (s.active_workspace().map(|ws| ws.id), s.workspaces.len())
    };
    if workspace_count <= 1 {
        return; // No-op: cannot close the last workspace
    }

    let dialog = gtk4::MessageDialog::builder()
        .text("Close Workspace?")
        .secondary_text("All panes in this workspace will be closed. This cannot be undone.")
        .modal(true)
        .build();
    if let Some(window) = app.windows().into_iter().next() {
        dialog.set_transient_for(Some(&window));
    }
    dialog.add_button("Keep Workspace", gtk4::ResponseType::Cancel);
    dialog.add_button("Close Workspace", gtk4::ResponseType::Accept);
    dialog.set_default_response(gtk4::ResponseType::Cancel);
    dialog.connect_response({
        let state = state.clone();
        move |dialog, response| {
            if response == gtk4::ResponseType::Accept {
                let mut s = state.borrow_mut();
                if let Some(index) = s
                    .workspaces
                    .iter()
                    .position(|ws| Some(ws.id) == workspace_id)
                {
                    s.close_workspace(index);
                }
            }
            dialog.close();
        }
    });
    dialog.present();
}

/// Split the active pane. `vertical=false` -> split right (Ctrl+D), `vertical=true` -> split down.
pub fn handle_split(state: &Rc<RefCell<AppState>>, vertical: bool) {
    let mut s = state.borrow_mut();
    if let Some(engine) = s.active_split_engine_mut() {
        let _new_pane_id = if vertical {
            engine.split_down()
        } else {
            engine.split_right()
        };
        // The new GLArea is already added to the widget tree inside SplitEngine.
        // CSS active-pane class is updated inside SplitEngine.
    }
}

/// Close the active pane (Ctrl+Shift+X).
pub fn handle_close_pane(state: &Rc<RefCell<AppState>>, app: &gtk4::Application) {
    let close_workspace = {
        let mut s = state.borrow_mut();
        if let Some(engine) = s.active_split_engine_mut() {
            let last = engine.close_active().is_none(); // Last pane: close the workspace.
            crate::inbox_actions::purge_closed_surfaces(&mut s);
            last
        } else {
            false
        }
    };
    if close_workspace {
        handle_close_workspace(state, app);
    }
}

/// Close one terminal/browser tab. The final tab follows the pane-close flow.
pub fn handle_close_surface_tab(
    state: &Rc<RefCell<AppState>>,
    app: &gtk4::Application,
    uuid: uuid::Uuid,
) {
    crate::diagnostics::event(format_args!("surface-tab close requested uuid={uuid}"));
    let result = state
        .borrow_mut()
        .active_split_engine_mut()
        .map(|engine| engine.close_surface_and_empty_pane(uuid));
    crate::diagnostics::event(format_args!(
        "surface-tab close result uuid={uuid} result={result:?}"
    ));
    match result {
        Some(crate::split_engine::CloseSurfaceResult::Closed) => {
            crate::inbox_actions::purge_closed_surfaces(&mut state.borrow_mut());
            state.borrow().trigger_session_save();
        }
        Some(crate::split_engine::CloseSurfaceResult::LastSurfaceInPane) => {
            handle_close_workspace(state, app);
        }
        Some(crate::split_engine::CloseSurfaceResult::NotFound) | None => {}
    }
}

/// Open the SSH connect dialog (Ctrl+Shift+S).
pub fn handle_new_ssh_workspace(state: &Rc<RefCell<AppState>>, app: &gtk4::Application) {
    crate::ssh_dialog::show_ssh_dialog(app, state.clone());
}

/// Move focus to adjacent pane in `direction`.
pub fn handle_focus_direction(state: &Rc<RefCell<AppState>>, direction: FocusDirection) {
    let mut s = state.borrow_mut();
    if let Some(engine) = s.active_split_engine_mut() {
        engine.focus_next_in_direction(direction);
    }
}

/// Synchronize cmux's active-pane model with pointer-driven GTK focus changes.
pub fn handle_focus_pane(state: &Rc<RefCell<AppState>>, pane_id: u64) {
    let Ok(mut app_state) = state.try_borrow_mut() else {
        let state = state.clone();
        glib::idle_add_local_once(move || handle_focus_pane(&state, pane_id));
        return;
    };
    if let Some(engine) = app_state.active_split_engine_mut() {
        if engine.activate_pane(pane_id) {
            crate::diagnostics::event(format_args!("pane activated by pointer pane={pane_id}"));
        }
    }
}

/// Activate the pane that holds a clicked terminal now, found by its tab identity.
pub fn handle_focus_terminal(state: &Rc<RefCell<AppState>>, surface: uuid::Uuid) {
    let Ok(app_state) = state.try_borrow() else {
        let state = state.clone();
        glib::idle_add_local_once(move || handle_focus_terminal(&state, surface));
        return;
    };
    let pane_id = app_state
        .active_split_engine()
        .and_then(|engine| engine.find_pane_id_by_uuid(&surface.to_string()));
    drop(app_state);
    if let Some(pane_id) = pane_id {
        handle_focus_pane(state, pane_id);
    }
}

/// Create a sibling terminal tab inside the currently focused pane.
pub fn handle_new_terminal_tab(state: &Rc<RefCell<AppState>>) {
    let created = state
        .borrow_mut()
        .active_split_engine_mut()
        .and_then(|engine| engine.new_terminal_tab())
        .is_some();
    if created {
        state.borrow().trigger_session_save();
    }
}
