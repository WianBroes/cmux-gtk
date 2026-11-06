//! Desktop decisions for Codex hooks, which wait before their native approval UI can open.
use gtk4::{glib, prelude::*};
use serde_json::{json, Value};
use std::{cell::Cell, collections::HashMap, rc::Rc};

/// Show each live Codex permission once; expire dialogs when mobile replies or the hook stops.
/// All widgets stay on GTK's thread; the timer ends when the application window is destroyed.
pub fn start(window: &gtk4::ApplicationWindow) {
    let parent = window.downgrade();
    let mut revision = None;
    let mut dialogs = HashMap::<String, gtk4::Dialog>::new();
    let stopped = Rc::new(Cell::new(false));
    let stop = stopped.clone();
    window.connect_destroy(move |_| stop.set(true));
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        if stopped.get() { return glib::ControlFlow::Break; }
        let Some(parent) = parent.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let pending = match crate::feed::global().try_lock() {
            Ok(store) => {
                if revision == Some(store.revision()) {
                    return glib::ControlFlow::Continue;
                }
                revision = Some(store.revision());
                store.desktop_permissions()
            }
            Err(_) => return glib::ControlFlow::Continue,
        };
        dialogs.retain(|request, dialog| {
            if pending.iter().any(|item| item["id"].as_str() == Some(request)) {
                true
            } else {
                dialog.close();
                false
            }
        });
        for item in pending {
            let Some(request) = item["id"].as_str() else { continue };
            if !dialogs.contains_key(request) {
                dialogs.insert(request.to_owned(), show(&parent, &item, request));
            }
        }
        glib::ControlFlow::Continue
    });
}

/// Present the exact input as plain, selectable text. Closing or Escape refuses this request.
fn show(parent: &gtk4::ApplicationWindow, item: &Value, request: &str) -> gtk4::Dialog {
    let dialog = gtk4::Dialog::builder()
        .title(format!("Codex — autorisation requise · {}", item["tool_name"].as_str().unwrap_or("Codex")))
        .transient_for(parent)
        .default_width(620)
        .default_height(360)
        .destroy_with_parent(true)
        .build();
    dialog.add_css_class("codex-permission");
    dialog.add_button("_Refuser", gtk4::ResponseType::Reject);
    dialog.add_button("_Accepter une fois", gtk4::ResponseType::Accept);
    dialog.set_default_response(gtk4::ResponseType::Reject);
    let content = dialog.content_area();
    content.set_spacing(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    let input = &item["desktop_input"];
    let reason = input["description"].as_str().unwrap_or("Autoriser cette action ?");
    let label = gtk4::Label::new(Some(reason));
    label.add_css_class("approval-reason");
    label.set_xalign(0.0);
    label.set_wrap(true);
    content.append(&label);
    let context = gtk4::Label::new(Some(&format!("{} · {}",
        item["tool_name"].as_str().unwrap_or("Codex"), item["cwd"].as_str().unwrap_or(""))));
    context.add_css_class("approval-context");
    context.set_xalign(0.0);
    context.set_selectable(true);
    context.set_wrap(true);
    content.append(&context);
    let text = gtk4::TextView::builder().editable(false).monospace(true)
        .wrap_mode(gtk4::WrapMode::WordChar).build();
    let disclosure = serde_json::to_string_pretty(input).unwrap_or_default();
    text.buffer().set_text(&disclosure);
    let scroll = gtk4::ScrolledWindow::builder().vexpand(true).child(&text).build();
    content.append(&scroll);
    let request = request.to_owned();
    dialog.connect_response(move |dialog, response| {
        let mode = if response == gtk4::ResponseType::Accept { "once" } else { "deny" };
        // The store rejects an expired/already resolved request: a stale click cannot approve it.
        if let Ok(mut store) = crate::feed::global().lock() {
            let _ = store.reply_item(&request, json!({"kind": "permission", "mode": mode}), crate::feed::now_ms());
        }
        dialog.close();
    });
    dialog.present();
    dialog
}
