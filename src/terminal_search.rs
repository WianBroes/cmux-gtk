//! Terminal find bar (upstream Edit › Find): a popover at the top right of a terminal that
//! drives Ghostty's own search with binding actions; Ghostty reports matches back through
//! START_SEARCH / END_SEARCH / SEARCH_TOTAL / SEARCH_SELECTED (`ghostty::events`).

use gtk4::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// Private key on a terminal's GLArea holding its find bar.
const BAR: &str = "cmux-search-bar";

#[derive(Clone)]
struct Bar {
    popover: gtk4::Popover,
    entry: gtk4::SearchEntry,
    count: gtk4::Label,
    /// Shown, or hidden only because its tab is (re-shown when the terminal maps again).
    active: Rc<Cell<bool>>,
    total: Rc<Cell<Option<isize>>>,
    selected: Rc<Cell<Option<isize>>>,
}

/// Send a Ghostty binding action (`search:<text>`, `navigate_search:next`…) to a terminal.
pub fn binding(area: &gtk4::GLArea, action: &str) {
    if let Some(surface) = crate::split_engine::surface_for_area(area) {
        unsafe {
            crate::ghostty::ffi::ghostty_surface_binding_action(
                surface,
                action.as_ptr().cast(),
                action.len(),
            );
        }
    }
}

/// Whether the terminal has a selection, for Use Selection for Find.
pub fn has_selection(area: &gtk4::GLArea) -> bool {
    crate::split_engine::surface_for_area(area)
        .is_some_and(|surface| unsafe { crate::ghostty::ffi::ghostty_surface_has_selection(surface) })
}

/// Whether this terminal's find bar is open.
pub fn is_active(area: &gtk4::GLArea) -> bool {
    bar(area).is_some_and(|bar| bar.active.get())
}

fn bar(area: &gtk4::GLArea) -> Option<Bar> {
    // SAFETY: this private key only ever stores a `Bar`, set by `bar_or_create`.
    unsafe { area.data::<Bar>(BAR).map(|bar| bar.as_ref().clone()) }
}

/// Show the bar (creating it on first use), put `needle` in it when given, focus the entry.
pub fn open(area: &gtk4::GLArea, needle: Option<&str>) {
    let bar = bar_or_create(area);
    if let Some(needle) = needle.filter(|needle| !needle.is_empty()) {
        if bar.entry.text() != needle {
            bar.entry.set_text(needle);
        }
    }
    bar.active.set(true);
    crate::diagnostics::record("terminal.search", serde_json::json!({"bar": "open"}));
    show(area, &bar);
    bar.entry.grab_focus();
    bar.entry.select_region(0, -1);
}

/// Hide the bar after Ghostty ended the search, and give the keyboard back to the terminal.
pub fn close(area: &gtk4::GLArea) {
    let Some(bar) = bar(area) else {
        return;
    };
    bar.active.set(false);
    bar.total.set(None);
    bar.selected.set(None);
    update_count(&bar);
    crate::diagnostics::record("terminal.search", serde_json::json!({"bar": "closed"}));
    bar.popover.popdown();
    area.grab_focus();
}

/// Ghostty's match count for the current needle (negative: unknown).
pub fn set_total(area: &gtk4::GLArea, total: isize) {
    // Ghostty reports a last count after END_SEARCH; a closed bar keeps its reset.
    if let Some(bar) = bar(area).filter(|bar| bar.active.get()) {
        bar.total.set((total >= 0).then_some(total));
        update_count(&bar);
    }
}

/// Ghostty's selected match, 0-based (negative: none).
pub fn set_selected(area: &gtk4::GLArea, selected: isize) {
    // Ghostty reports a last count after END_SEARCH; a closed bar keeps its reset.
    if let Some(bar) = bar(area).filter(|bar| bar.active.get()) {
        bar.selected.set((selected >= 0).then_some(selected));
        update_count(&bar);
    }
}

/// Ghostty's label: "selected/total", "0/0" without matches.
fn update_count(bar: &Bar) {
    let text = match bar.total.get() {
        Some(total) if total > 0 => {
            format!("{}/{}", bar.selected.get().map_or(0, |selected| selected + 1), total)
        }
        _ => "0/0".to_owned(),
    };
    // Metadata only (never the needle): lets a scripted run read what the bar shows.
    crate::diagnostics::record("terminal.search", serde_json::json!({"count": text}));
    bar.count.set_text(&text);
}

/// Point the popover at the terminal's top right corner and show it.
fn show(area: &gtk4::GLArea, bar: &Bar) {
    let width = area.width().max(1);
    bar.popover
        .set_pointing_to(Some(&gtk4::gdk::Rectangle::new(width - 1, 0, 1, 1)));
    bar.popover.popup();
}

fn bar_or_create(area: &gtk4::GLArea) -> Bar {
    if let Some(bar) = bar(area) {
        return bar;
    }
    let entry = gtk4::SearchEntry::new();
    entry.set_placeholder_text(Some("Find…"));
    entry.set_width_chars(20);
    entry.set_hexpand(true);
    let count = gtk4::Label::new(Some("0/0"));
    count.add_css_class("dim-label");
    count.set_width_chars(6);
    count.set_xalign(1.0);
    let button = |icon: &str, tooltip: &str| {
        let button = gtk4::Button::from_icon_name(icon);
        button.set_tooltip_text(Some(tooltip));
        button
    };
    // Ghostty's own bar: "up" navigates to the next match, "down" to the previous one.
    let up = button("go-up-symbolic", "Find Next");
    let down = button("go-down-symbolic", "Find Previous");
    let close = button("window-close-symbolic", "Hide Find Bar");
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    row.append(&entry);
    row.append(&count);
    row.append(&up);
    row.append(&down);
    row.append(&close);
    let popover = gtk4::Popover::new();
    popover.set_child(Some(&row));
    popover.set_has_arrow(false);
    popover.set_autohide(false);
    popover.set_position(gtk4::PositionType::Bottom);
    popover.set_parent(area);
    let bar = Bar {
        popover,
        entry,
        count,
        active: Rc::new(Cell::new(false)),
        total: Rc::new(Cell::new(None)),
        selected: Rc::new(Cell::new(None)),
    };
    let weak = area.downgrade();
    let send = move |action: &str| {
        if let Some(area) = weak.upgrade() {
            binding(&area, action);
        }
    };
    bar.entry.connect_search_changed({
        let send = send.clone();
        move |entry| send(&format!("search:{}", entry.text()))
    });
    bar.entry.connect_activate({
        let send = send.clone();
        move |_| send("navigate_search:next")
    });
    // Shift+Enter, like Ghostty's bar; the entry's own Ctrl+G / Ctrl+Shift+G signals too.
    let keys = gtk4::EventControllerKey::new();
    keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let send = send.clone();
        move |_, key, _, mods| {
            let enter = key == gtk4::gdk::Key::Return || key == gtk4::gdk::Key::KP_Enter;
            if enter && mods.contains(gtk4::gdk::ModifierType::SHIFT_MASK) {
                send("navigate_search:previous");
                return gtk4::glib::Propagation::Stop;
            }
            gtk4::glib::Propagation::Proceed
        }
    });
    bar.entry.add_controller(keys);
    bar.entry.connect_next_match({
        let send = send.clone();
        move |_| send("navigate_search:next")
    });
    bar.entry.connect_previous_match({
        let send = send.clone();
        move |_| send("navigate_search:previous")
    });
    bar.entry.connect_stop_search({
        let send = send.clone();
        move |_| send("end_search")
    });
    up.connect_clicked({
        let send = send.clone();
        move |_| send("navigate_search:next")
    });
    down.connect_clicked({
        let send = send.clone();
        move |_| send("navigate_search:previous")
    });
    close.connect_clicked(move |_| send("end_search"));
    // A popover hides with its terminal's tab; bring it back when the tab shows again.
    area.connect_map({
        let bar = bar.clone();
        move |area| {
            if bar.active.get() {
                show(area, &bar);
            }
        }
    });
    area.connect_destroy({
        let popover = bar.popover.clone();
        move |_| popover.unparent()
    });
    // SAFETY: this private key only ever stores a `Bar`.
    unsafe { area.set_data(BAR, bar.clone()) };
    bar
}
