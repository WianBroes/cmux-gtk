//! GTK header controls backed by the same window actions as menus and shortcuts.

use gtk4::prelude::*;

/// The window's single titlebar row, upstream's on macOS (Wian, 2026-10-20): its controls —
/// sidebar, notifications, new workspace with its menu, Focus Back / Forward — then the menus
/// (Linux has no screen-wide menu bar), the selected workspace's name in the middle, the file
/// explorer and the window buttons on the right; nothing moves when a sidebar hides. Built on
/// the GTK thread, unless the header is hidden. Returns the arrows (their right-click history
/// menus are attached once state exists) and the title label (`set_title`).
pub fn build_header_bar(
    config: &crate::config::Config,
    bell: &gtk4::Overlay,
    menu_bar: &gtk4::gio::Menu,
) -> Option<(gtk4::HeaderBar, [gtk4::Button; 2], gtk4::Label)> {
    if config.ui.header_bar.style == "none" {
        return None;
    }
    let header = gtk4::HeaderBar::new();
    header.add_css_class("cmux-headerbar");
    // Window buttons are cmux's own, in the titlebar's icon pack (`crate::icons`), on the right;
    // the theme's would differ from the rest of the row (and some layouts add an app icon).
    header.set_show_title_buttons(false);
    for (icon, tooltip, action) in [
        ("cmux-close-symbolic", "Close", "window.close"),
        ("cmux-maximize-symbolic", "Maximize", "window.toggle-maximized"),
        ("cmux-minimize-symbolic", "Minimize", "window.minimize"),
    ] {
        header.pack_end(&action_button(icon, tooltip, action));
    }
    header.pack_start(&action_button(
        "cmux-sidebar-symbolic",
        "Toggle Sidebar (Ctrl+B)",
        "win.toggle-sidebar",
    ));
    header.pack_start(bell);
    header.pack_start(&new_workspace_split_button());
    let back = action_button(
        "cmux-back-symbolic",
        "Focus Back (Ctrl+Alt+Left)",
        "win.focus-back",
    );
    let forward = action_button(
        "cmux-forward-symbolic",
        "Focus Forward (Ctrl+Alt+Right)",
        "win.focus-forward",
    );
    // A disabled arrow no longer takes clicks, so GTK hands them to the titlebar, where the
    // window manager reads them as a titlebar double-click or drag and moves the window. The
    // arrows' own box claims presses on a disabled arrow only; an enabled one keeps its click.
    let arrows = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    arrows.append(&back);
    arrows.append(&forward);
    let absorb = gtk4::GestureClick::new();
    absorb.set_button(0);
    absorb.connect_pressed(|gesture, _, x, y| {
        let disabled = gesture
            .widget()
            .and_then(|arrows| arrows.pick(x, y, gtk4::PickFlags::INSENSITIVE))
            .and_then(|widget| {
                if widget.is::<gtk4::Button>() {
                    Some(widget)
                } else {
                    widget.ancestor(gtk4::Button::static_type())
                }
            })
            .is_some_and(|button| !button.is_sensitive());
        if disabled {
            gesture.set_state(gtk4::EventSequenceState::Claimed);
        }
    });
    arrows.add_controller(absorb);
    header.pack_start(&arrows);
    header.pack_start(&gtk4::PopoverMenuBar::from_model(Some(menu_bar)));
    let title = gtk4::Label::new(None);
    title.add_css_class("title");
    title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    let center = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    let folder = gtk4::Image::from_icon_name("cmux-folder-symbolic");
    folder.add_css_class("dim-label");
    center.append(&folder);
    center.append(&title);
    header.set_title_widget(Some(&center));
    // Files panel toggle. Icon is folder-symbolic: breeze (the active theme here) has no sidebar-show-right-symbolic, GTK showed image-missing.
    header.pack_end(&action_button(
        "cmux-folder-symbolic",
        "Toggle File Explorer (Ctrl+Alt+B)",
        "win.toggle-right-sidebar",
    ));
    Some((header, [back, forward], title))
}

/// Show the selected workspace's name in the titlebar; no-op when it is already shown.
pub fn set_title(label: &gtk4::Label, name: &str) {
    if label.text() != name {
        label.set_text(name);
    }
}

/// Upstream's "+" split button: the button makes a workspace, the caret (or a right-click on
/// either part) opens the New Workspace menu — its default rows, less the Cloud ones.
fn new_workspace_split_button() -> gtk4::Box {
    let model = gtk4::gio::Menu::new();
    model.append(Some("New Workspace"), Some("win.new-workspace"));
    model.append(Some("New Terminal Tab"), Some("win.new-terminal-tab"));
    model.append(Some("New Browser Tab"), Some("win.new-browser-tab"));
    let plus = action_button("cmux-plus-symbolic", "New Workspace (Ctrl+N)", "win.new-workspace");
    let caret = gtk4::MenuButton::new();
    caret.set_icon_name("cmux-chevron-down-symbolic");
    caret.set_tooltip_text(Some("New workspace options"));
    caret.set_menu_model(Some(&model));
    caret.add_css_class("headerbar-btn");
    let group = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    group.add_css_class("linked");
    group.append(&plus);
    group.append(&caret);
    let right_click = gtk4::GestureClick::new();
    right_click.set_button(3);
    right_click.connect_pressed({
        let caret = caret.downgrade();
        move |gesture, _, _, _| {
            gesture.set_state(gtk4::EventSequenceState::Claimed);
            if let Some(caret) = caret.upgrade() {
                caret.popup();
            }
        }
    });
    group.add_controller(right_click);
    group
}

/// Right-click on an arrow lists the positions it leads to (upstream shows 12), newest first.
pub fn attach_focus_history_menus(state: &crate::app_state::AppStateRef, arrows: &[gtk4::Button; 2]) {
    use crate::focus_history::Direction;
    for (button, direction) in arrows.iter().zip([Direction::Back, Direction::Forward]) {
        let right_click = gtk4::GestureClick::new();
        right_click.set_button(3);
        right_click.connect_pressed({
            let state = std::rc::Rc::downgrade(state);
            let button = button.downgrade();
            move |gesture, _, _, _| {
                gesture.set_state(gtk4::EventSequenceState::Claimed);
                let (Some(state), Some(button)) = (state.upgrade(), button.upgrade()) else {
                    return;
                };
                let model = gtk4::gio::Menu::new();
                {
                    let s = state.borrow();
                    let items = s.focus_history_items(direction);
                    if items.is_empty() {
                        let empty = gtk4::gio::MenuItem::new(Some("No Focus History"), None);
                        empty.set_action_and_target_value(Some("win.focus-history-none"), None);
                        model.append_item(&empty);
                    }
                    for item in items.iter().take(crate::focus_history::MENU_LIMIT) {
                        let row = gtk4::gio::MenuItem::new(
                            Some(&s.focus_history_label(item, direction)),
                            None,
                        );
                        row.set_action_and_target_value(
                            Some("win.focus-history-go"),
                            Some(&(item.index as u64).to_variant()),
                        );
                        model.append_item(&row);
                    }
                }
                let popover = gtk4::PopoverMenu::from_model(Some(&model));
                popover.set_parent(&button);
                popover.set_has_arrow(false);
                popover.connect_closed(|popover| {
                    let popover = popover.clone();
                    gtk4::glib::idle_add_local_once(move || popover.unparent());
                });
                popover.popup();
            }
        });
        button.add_controller(right_click);
    }
}

/// Upstream's titlebar notification bell: opens the notification list, with an unread badge.
pub fn notification_bell() -> (gtk4::Overlay, gtk4::Label) {
    let button = gtk4::Button::new();
    button.set_child(Some(&gtk4::Image::from_icon_name("cmux-bell-symbolic")));
    button.set_tooltip_text(Some("Notifications (Ctrl+Shift+I)"));
    button.set_action_name(Some("win.notifications"));
    button.add_css_class("headerbar-btn");
    let badge = gtk4::Label::new(None);
    badge.add_css_class("header-badge");
    badge.set_halign(gtk4::Align::End);
    badge.set_valign(gtk4::Align::Start);
    badge.set_can_target(false);
    badge.set_visible(false);
    let overlay = gtk4::Overlay::new();
    overlay.set_child(Some(&button));
    overlay.add_overlay(&badge);
    (overlay, badge)
}

/// Create a consistently styled header button bound to an existing GIO action.
fn action_button(icon: &str, tooltip: &str, action: &str) -> gtk4::Button {
    let button = gtk4::Button::from_icon_name(icon);
    button.set_tooltip_text(Some(tooltip));
    button.set_action_name(Some(action));
    button.add_css_class("headerbar-btn");
    button
}
