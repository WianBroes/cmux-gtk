//! Terminal text zoom feedback and Ctrl+wheel zoom (Wian, 2026-10-20; upstream zooms with
//! ⌘+/⌘−/⌘0 only, a point per step, and shows no size). Ghostty reports each font change
//! (cmux fork callback); a bubble then shows the size against the configured one, "120 %",
//! for a second. 100 % is the size Ghostty calls unadjusted, the one Ctrl+0 returns to.

use gtk4::prelude::*;

/// Private keys on a terminal's GLArea.
const BASE: &str = "cmux-font-base";
const BUBBLE: &str = "cmux-font-bubble";

/// Wire a new terminal: remember its unadjusted size and follow its font changes.
///
/// # Safety
/// `surface` is the live surface of `area`, freed (`destroy_terminal_area`) while `area` lives.
pub unsafe fn install(area: &gtk4::GLArea, surface: crate::ghostty::ffi::ghostty_surface_t) {
    unsafe {
        if !crate::ghostty::ffi::ghostty_surface_font_size_adjusted(surface) {
            area.set_data(BASE, crate::ghostty::ffi::ghostty_surface_font_size(surface));
        }
        crate::ghostty::ffi::ghostty_surface_set_font_size_action_callback(
            surface,
            Some(font_size_changed),
            area.as_ptr().cast(),
        );
    }
}

/// Ghostty changed a terminal's font size (synchronously, on GTK): update the base, show the
/// bubble after the callback returns.
unsafe extern "C" fn font_size_changed(
    userdata: *mut std::ffi::c_void,
    _action: crate::ghostty::ffi::ghostty_font_size_action_e,
    previous: f32,
    current: f32,
    previous_adjusted: bool,
    current_adjusted: bool,
) {
    // SAFETY: the userdata is the GLArea set by `install`, alive until the surface is freed.
    let area: gtk4::glib::translate::Borrowed<gtk4::GLArea> =
        unsafe { gtk4::glib::translate::from_glib_borrow(userdata as *mut gtk4::ffi::GtkGLArea) };
    let base = if !current_adjusted {
        Some(current)
    } else if !previous_adjusted {
        Some(previous)
    } else {
        // SAFETY: this key only ever stores an f32.
        unsafe { area.data::<f32>(BASE).map(|base| *base.as_ref()) }
    };
    if let Some(base) = base {
        // SAFETY: as above.
        unsafe { area.set_data(BASE, base) };
    }
    let text = match base {
        Some(base) if base > 0.0 => format!("{} %", (current / base * 100.0).round() as i32),
        _ => format!("{current:.0} pt"),
    };
    crate::diagnostics::record("terminal.font_zoom", serde_json::json!({"shown": text}));
    let area = area.downgrade();
    gtk4::glib::idle_add_local_once(move || {
        if let Some(area) = area.upgrade() {
            show_bubble(&area, &text);
        }
    });
}

/// Ctrl+wheel: a notch up enlarges by a point, down shrinks; `notches` accumulates touchpad
/// deltas until a whole notch.
pub fn scroll(area: &gtk4::GLArea, dy: f64, notches: &std::cell::Cell<f64>) {
    let total = notches.get() + dy;
    let steps = total.trunc();
    notches.set(total - steps);
    let action = if steps < 0.0 { "increase_font_size:1" } else { "decrease_font_size:1" };
    for _ in 0..steps.abs() as u32 {
        crate::terminal_search::binding(area, action);
    }
}

/// "120 %" in the middle of the terminal for a second; a newer change restarts the second.
fn show_bubble(area: &gtk4::GLArea, text: &str) {
    // SAFETY: this key only ever stores a (Popover, Label, generation) tuple, set below.
    let existing = unsafe {
        area.data::<(gtk4::Popover, gtk4::Label, std::rc::Rc<std::cell::Cell<u64>>)>(BUBBLE)
            .map(|bubble| bubble.as_ref().clone())
    };
    let (popover, label, generation) = existing.unwrap_or_else(|| {
        let label = gtk4::Label::new(None);
        label.add_css_class("font-zoom-bubble");
        let popover = gtk4::Popover::new();
        popover.set_child(Some(&label));
        popover.set_has_arrow(false);
        popover.set_autohide(false);
        popover.set_can_focus(false);
        popover.set_parent(area);
        area.connect_destroy({
            let popover = popover.clone();
            move |_| popover.unparent()
        });
        let bubble = (popover, label, std::rc::Rc::new(std::cell::Cell::new(0)));
        // SAFETY: as above.
        unsafe { area.set_data(BUBBLE, bubble.clone()) };
        bubble
    });
    label.set_text(text);
    let (width, height) = (area.width(), area.height());
    popover.set_pointing_to(Some(&gtk4::gdk::Rectangle::new(width / 2, height / 2, 1, 1)));
    popover.popup();
    let mine = generation.get() + 1;
    generation.set(mine);
    let popover = popover.downgrade();
    gtk4::glib::timeout_add_local_once(std::time::Duration::from_secs(1), move || {
        if generation.get() == mine {
            if let Some(popover) = popover.upgrade() {
                popover.popdown();
            }
        }
    });
}
