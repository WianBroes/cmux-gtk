//! In-app editor for a workspace description, the counterpart of upstream's
//! "Edit workspace description" (⌥⌘E). Markdown, shown under the workspace name in the sidebar.

use crate::app_state::AppState;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Open the editor for the workspace with `uuid`, prefilled with its current description.
pub fn show(
    parent: &gtk4::ApplicationWindow,
    state: &Rc<RefCell<AppState>>,
    uuid: uuid::Uuid,
) {
    let current = state
        .borrow()
        .workspaces
        .iter()
        .find(|workspace| workspace.uuid == uuid)
        .and_then(|workspace| workspace.custom_description.clone());

    let window = gtk4::Window::builder()
        .title("Workspace Description")
        .modal(true)
        .default_width(460)
        .default_height(320)
        .build();
    window.add_css_class("workspace-description-dialog");
    window.set_transient_for(Some(parent));

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    vbox.set_margin_start(16);
    vbox.set_margin_end(16);
    vbox.set_margin_top(16);
    vbox.set_margin_bottom(16);

    let help = gtk4::Label::new(Some(
        "Shown under the workspace name. Markdown (links, code), 12 lines at most.",
    ));
    help.set_xalign(0.0);
    help.set_wrap(true);
    help.add_css_class("dim-label");
    vbox.append(&help);

    let editor = gtk4::TextView::new();
    editor.set_wrap_mode(gtk4::WrapMode::WordChar);
    editor.buffer().set_text(current.as_deref().unwrap_or(""));
    let scrolled = gtk4::ScrolledWindow::builder()
        .vexpand(true)
        .child(&editor)
        .build();
    vbox.append(&scrolled);

    let buttons = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    buttons.set_halign(gtk4::Align::End);
    let clear = gtk4::Button::with_label("Clear");
    clear.set_tooltip_text(Some("Remove the description"));
    let cancel = gtk4::Button::with_label("Cancel");
    let apply = gtk4::Button::with_label("Apply");
    apply.add_css_class("suggested-action");
    buttons.append(&clear);
    buttons.append(&cancel);
    buttons.append(&apply);
    vbox.append(&buttons);
    window.set_child(Some(&vbox));

    // One commit path for all three buttons; an empty value clears the description.
    let commit: Rc<dyn Fn(Option<String>)> = Rc::new({
        let state = state.clone();
        let window = window.clone();
        let editor = editor.clone();
        move |value| {
            let value = value.or_else(|| {
                let buffer = editor.buffer();
                Some(buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string())
            });
            let index = state
                .borrow()
                .workspaces
                .iter()
                .position(|workspace| workspace.uuid == uuid);
            if let Some(index) = index {
                state
                    .borrow_mut()
                    .set_workspace_description(index, value);
            }
            window.close();
        }
    });
    apply.connect_clicked({
        let commit = commit.clone();
        move |_| commit(None)
    });
    clear.connect_clicked({
        let commit = commit.clone();
        move |_| commit(Some(String::new()))
    });
    cancel.connect_clicked({
        let window = window.clone();
        move |_| window.close()
    });
    // Escape closes without applying, like any other dialog.
    let keys = gtk4::EventControllerKey::new();
    keys.connect_key_pressed({
        let window = window.clone();
        move |_, key, _, _| {
            if key == gtk4::gdk::Key::Escape {
                window.close();
                return gtk4::glib::Propagation::Stop;
            }
            gtk4::glib::Propagation::Proceed
        }
    });
    window.add_controller(keys);
    window.present();
    editor.grab_focus();
}
