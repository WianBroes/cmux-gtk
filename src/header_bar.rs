//! GTK header controls backed by the same window actions as menus and shortcuts.

use gtk4::prelude::*;

/// Build the titlebar controls in upstream's order — sidebar, notifications, new workspace with
/// its menu, Focus Back / Forward — on the GTK thread, unless the header is hidden. The window
/// buttons and the ≡ menu stay: Linux has no global menu bar to hold Preferences and Help.
/// Returns the arrows so their right-click history menus can be attached once state exists.
pub fn build_header_bar(
    config: &crate::config::Config,
    bell: &gtk4::Overlay,
) -> Option<(gtk4::HeaderBar, [gtk4::Button; 2])> {
    if config.ui.header_bar.style == "none" {
        return None;
    }
    let header = gtk4::HeaderBar::new();
    header.add_css_class("cmux-headerbar");
    header.pack_start(&action_button(
        "sidebar-show-symbolic",
        "Toggle Sidebar (Ctrl+B)",
        "win.toggle-sidebar",
    ));
    header.pack_start(bell);
    header.pack_start(&new_workspace_split_button());
    let back = action_button(
        "go-previous-symbolic",
        "Focus Back (Ctrl+Alt+Left)",
        "win.focus-back",
    );
    let forward = action_button(
        "go-next-symbolic",
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
    let menu = gtk4::MenuButton::new();
    menu.set_icon_name("open-menu-symbolic");
    menu.set_tooltip_text(Some("Menu"));
    menu.set_menu_model(Some(&crate::menus::build_hamburger_menu()));
    menu.add_css_class("headerbar-btn");
    header.pack_end(&menu);
    Some((header, [back, forward]))
}

/// Upstream's "+" split button: the button makes a workspace, the caret (or a right-click on
/// either part) opens the New Workspace menu — its default rows, less the Cloud ones.
fn new_workspace_split_button() -> gtk4::Box {
    let model = gtk4::gio::Menu::new();
    model.append(Some("New Workspace"), Some("win.new-workspace"));
    model.append(Some("New Terminal Tab"), Some("win.new-terminal-tab"));
    model.append(Some("New Browser Tab"), Some("win.new-browser-tab"));
    let plus = action_button("list-add-symbolic", "New Workspace (Ctrl+N)", "win.new-workspace");
    let caret = gtk4::MenuButton::new();
    caret.set_icon_name("pan-down-symbolic");
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
    let icon = gtk4::gio::ThemedIcon::from_names(&[
        "notifications-symbolic",
        "preferences-system-notifications-symbolic",
    ]);
    button.set_child(Some(&gtk4::Image::from_gicon(&icon)));
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
