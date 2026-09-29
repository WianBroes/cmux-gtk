//! "Shortcuts" page of the Preferences dialog: change, unbind or reset the shortcuts listed in
//! [`crate::config::EDITABLE_SHORTCUTS`]. Each change is written to `cmux.json` at once (the
//! page has no Apply step) and applied live through `config::reload_shortcuts`. Combinations
//! that would steal keys from the terminal or from agents are refused, with the reason shown
//! (see `shortcut_rules`).

use crate::config::{self, ShortcutAction, EDITABLE_SHORTCUTS, FIXED_SHORTCUTS};
use gtk4::gdk::{Key, ModifierType};
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

const MODS: ModifierType = ModifierType::from_bits_truncate(
    ModifierType::CONTROL_MASK.bits()
        | ModifierType::SHIFT_MASK.bits()
        | ModifierType::ALT_MASK.bits()
        | ModifierType::SUPER_MASK.bits()
        | ModifierType::META_MASK.bits()
        | ModifierType::HYPER_MASK.bits(),
);

struct Row {
    id: &'static str,
    action: ShortcutAction,
    value: gtk4::Label,
    change: gtk4::Button,
}

/// Human spelling of the live binding, or "—" when unbound.
fn shown(action: ShortcutAction) -> String {
    config::live_accelerator(action)
        .and_then(|accel| gtk4::accelerator_parse(&accel))
        .map(|(key, mods)| gtk4::accelerator_get_label(key, mods).to_string())
        .unwrap_or_else(|| "—".to_string())
}

/// Why `key`+`mods` cannot be given to `action`: a reserved key, a fixed cmux shortcut, or a
/// combination another action already uses.
fn refusal(action: ShortcutAction, mods: ModifierType, key: Key) -> Option<String> {
    if let Some(reason) = crate::shortcut_rules::refusal(mods, key) {
        return Some(reason);
    }
    for (accel, name) in FIXED_SHORTCUTS {
        if let Some((fixed_key, fixed_mods)) = gtk4::accelerator_parse(*accel) {
            if fixed_key == key && fixed_mods == mods {
                return Some(format!("Already used by \"{name}\", which is not editable."));
            }
        }
    }
    if let Some(other) = config::live_lookup(mods, key).filter(|other| *other != action) {
        let name = EDITABLE_SHORTCUTS
            .iter()
            .find(|(_, _, _, a)| *a == other)
            .map(|(_, label, _, _)| (*label).to_string())
            .unwrap_or_else(|| format!("{other:?}"));
        return Some(format!(
            "Already used by \"{name}\". Unbind or change that one first."
        ));
    }
    None
}

/// Write (`Some`) or remove (`None`) `shortcuts.bindings.<id>` in the global `cmux.json`, then
/// reload the live map. The rest of the file is preserved by `settings_json::apply`.
fn store(id: &str, value: Option<&str>) -> Result<(), String> {
    let path = crate::settings_json::global_path().ok_or("no cmux.json location")?;
    let json = value.map(|text| serde_json::Value::String(text.to_owned()));
    crate::settings_json::apply(
        &path,
        &format!("shortcuts.bindings.{id}"),
        json.as_ref(),
        crate::settings_json::Scope::Global,
    )
    .map_err(|error| error.to_string())?;
    config::reload_shortcuts();
    Ok(())
}

/// Fill `page` with the shortcut editor. `dialog` supplies the key events while a row waits for
/// its new combination.
pub fn append(page: &gtk4::Box, dialog: &gtk4::Dialog) {
    let intro = gtk4::Label::new(Some(
        "Click Change, then press the new combination. Changes apply at once and are saved in \
         cmux.json. Combinations that terminals and agents rely on (Ctrl+C, Ctrl+D, Enter, \
         Alt+letters…) are refused so cmux never swallows them.",
    ));
    intro.set_xalign(0.0);
    intro.set_wrap(true);
    page.append(&intro);
    let status = gtk4::Label::new(None);
    status.set_xalign(0.0);
    status.set_wrap(true);
    page.append(&status);

    let rows: Rc<RefCell<Vec<Row>>> = Rc::new(RefCell::new(Vec::new()));
    // Index of the row waiting for a key press.
    let waiting: Rc<RefCell<Option<usize>>> = Rc::new(RefCell::new(None));

    let refresh = {
        let rows = rows.clone();
        move || {
            for row in rows.borrow().iter() {
                row.value.set_text(&shown(row.action));
                row.change.set_label("Change");
            }
        }
    };

    let mut section = "";
    for (index, (id, label, category, action)) in EDITABLE_SHORTCUTS.iter().enumerate() {
        if *category != section {
            section = category;
            let heading = gtk4::Label::new(Some(category));
            heading.set_xalign(0.0);
            heading.add_css_class("heading");
            heading.set_margin_top(8);
            page.append(&heading);
        }
        let line = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let name = gtk4::Label::new(Some(label));
        name.set_xalign(0.0);
        name.set_hexpand(true);
        let value = gtk4::Label::new(None);
        value.set_width_chars(16);
        let change = gtk4::Button::with_label("Change");
        let unbind = gtk4::Button::with_label("Unbind");
        let reset = gtk4::Button::with_label("Reset");
        for widget in [&name.clone().upcast::<gtk4::Widget>(), value.upcast_ref()] {
            line.append(widget);
        }
        line.append(&change);
        line.append(&unbind);
        line.append(&reset);
        page.append(&line);
        rows.borrow_mut().push(Row {
            id,
            action: *action,
            value: value.clone(),
            change: change.clone(),
        });

        change.connect_clicked({
            let waiting = waiting.clone();
            let status = status.clone();
            let rows = rows.clone();
            move |_| {
                for row in rows.borrow().iter() {
                    row.change.set_label("Change");
                }
                *waiting.borrow_mut() = Some(index);
                rows.borrow()[index].change.set_label("Press keys…  (Esc cancels)");
                status.set_text("");
            }
        });
        for (button, unbinding) in [(unbind, true), (reset, false)] {
            button.connect_clicked({
                let status = status.clone();
                let refresh = refresh.clone();
                let waiting = waiting.clone();
                move |_| {
                    *waiting.borrow_mut() = None;
                    let result = store(id, unbinding.then_some("none"));
                    status.set_text(&result.err().map(|e| format!("Not saved: {e}")).unwrap_or_default());
                    refresh();
                }
            });
        }
    }
    refresh();

    let keys = gtk4::EventControllerKey::new();
    keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let rows = rows.clone();
        let waiting = waiting.clone();
        let status = status.clone();
        let refresh = refresh.clone();
        move |_, key, _, state| {
            let Some(index) = *waiting.borrow() else {
                return gtk4::glib::Propagation::Proceed;
            };
            // A lone modifier press is the start of a combination, not the combination.
            if matches!(
                key,
                Key::Control_L | Key::Control_R | Key::Shift_L | Key::Shift_R | Key::Alt_L
                    | Key::Alt_R | Key::Super_L | Key::Super_R | Key::Meta_L | Key::Meta_R
                    | Key::ISO_Level3_Shift
            ) {
                return gtk4::glib::Propagation::Stop;
            }
            if key == Key::Escape && (state & MODS).is_empty() {
                *waiting.borrow_mut() = None;
                refresh();
                return gtk4::glib::Propagation::Stop;
            }
            let mods = state & MODS;
            let key = key.to_lower();
            let (id, action) = {
                let rows = rows.borrow();
                (rows[index].id, rows[index].action)
            };
            if let Some(reason) = refusal(action, mods, key) {
                let spelled = gtk4::accelerator_get_label(key, mods);
                status.set_text(&format!("{spelled} refused: {reason}"));
                return gtk4::glib::Propagation::Stop;
            }
            *waiting.borrow_mut() = None;
            match store(id, Some(&config::accelerator_to_text(mods, key))) {
                Ok(()) => status.set_text(""),
                Err(error) => status.set_text(&format!("Not saved: {error}")),
            }
            refresh();
            gtk4::glib::Propagation::Stop
        }
    });
    dialog.add_controller(keys);
}
