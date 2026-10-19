use crate::ghostty::ffi;
use gtk4::prelude::*;

/// Maps GDK modifier state (gdk4::ModifierType bits) to ghostty_input_mods_e.
/// Returns 0 if no modifiers.
pub fn map_mods(state: gtk4::gdk::ModifierType) -> ffi::ghostty_input_mods_e {
    let mut mods: ffi::ghostty_input_mods_e = 0;
    use gtk4::gdk::ModifierType;
    if state.contains(ModifierType::SHIFT_MASK) {
        mods |= ffi::ghostty_input_mods_e_GHOSTTY_MODS_SHIFT;
    }
    if state.contains(ModifierType::CONTROL_MASK) {
        mods |= ffi::ghostty_input_mods_e_GHOSTTY_MODS_CTRL;
    }
    if state.contains(ModifierType::ALT_MASK) {
        mods |= ffi::ghostty_input_mods_e_GHOSTTY_MODS_ALT;
    }
    if state.contains(ModifierType::SUPER_MASK) {
        mods |= ffi::ghostty_input_mods_e_GHOSTTY_MODS_SUPER;
    }
    mods
}

/// Codepoint of the key without any modifier (level 0 of the active layout),
/// as Ghostty's own GTK runtime computes it (`keyvalUnicodeUnshifted` in
/// ghostty/src/apprt/gtk/key.zig). The Kitty keyboard protocol needs it: when
/// it is 0, Ghostty sends the text as-is, so Ctrl+C reaches the app as `c`.
///
/// `entries` = (group, level, unicode codepoint) for every keyval the physical
/// key can produce (from `gdk::Display::map_keycode`); `layout` = the event's
/// active layout (group). Returns 0 when nothing matches.
pub fn unshifted_codepoint<I>(entries: I, layout: u32) -> u32
where
    I: IntoIterator<Item = (i32, i32, u32)>,
{
    entries
        .into_iter()
        .find(|&(group, level, _)| group as u32 == layout && level == 0)
        .map_or(0, |(_, _, cp)| cp)
}

/// Text to send with a key press. Control characters (< 0x20) are not text:
/// Ghostty's encoder handles them from the key itself (same rule as
/// ghostty/src/apprt/gtk/class/surface.zig, `keyEvent`).
pub fn key_text_char(unicode: Option<char>) -> Option<char> {
    unicode.filter(|&ch| ch as u32 >= 0x20)
}

/// Where a key event stands relative to the input method, as `in_keyevent` in
/// Ghostty's GTK runtime (ghostty/src/apprt/gtk/class/surface.zig).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyPhase {
    /// No key event in progress: a commit comes from elsewhere (on-screen keyboard...).
    Outside,
    /// In a key event that started while the input method was composing (dead key pending).
    Composing,
    /// In a key event with nothing being composed: a commit is this key's own text.
    NotComposing,
}

/// Input method state of one terminal surface. Dead keys (^ ¨ ~ ` ´ on AZERTY),
/// compose sequences and IMEs go through a GtkIMMulticontext before Ghostty, as
/// in Ghostty's GTK runtime and in macOS's `interpretKeyEvents`.
pub struct ImeState {
    pub composing: std::cell::Cell<bool>,
    pub phase: std::cell::Cell<KeyPhase>,
    buf: std::cell::RefCell<[u8; 129]>, // 128 bytes of UTF-8 + NUL
    len: std::cell::Cell<usize>,
}

impl Default for ImeState {
    fn default() -> Self {
        Self {
            composing: std::cell::Cell::new(false),
            phase: std::cell::Cell::new(KeyPhase::Outside),
            buf: std::cell::RefCell::new([0; 129]),
            len: std::cell::Cell::new(0),
        }
    }
}

impl ImeState {
    /// Keep a commit made during a non-composing key event: it is the key's text.
    /// Returns false when it does not fit (the caller then sends it on its own).
    pub fn store(&self, text: &str) -> bool {
        let bytes = text.as_bytes();
        let mut buf = self.buf.borrow_mut();
        if bytes.len() >= buf.len() {
            return false;
        }
        buf[..bytes.len()].copy_from_slice(bytes);
        buf[bytes.len()] = 0;
        self.len.set(bytes.len());
        true
    }

    pub fn committed_len(&self) -> usize {
        self.len.get()
    }

    /// NUL-terminated committed text, or null when the key committed nothing.
    pub fn committed_ptr(&self) -> *const std::os::raw::c_char {
        if self.len.get() == 0 {
            std::ptr::null()
        } else {
            self.buf.as_ptr().cast()
        }
    }

    pub fn clear(&self) {
        self.len.set(0);
    }
}

/// Whether the input method took the key, so it must not reach Ghostty's encoder.
/// Same rules as Ghostty's GTK `keyEvent`: composing (dead key pending), a key that
/// ended a composition (its result was committed separately), or handled without text.
pub fn ime_takes_key(handled: bool, composing: bool, phase: KeyPhase, committed_len: usize) -> bool {
    handled && (composing || phase == KeyPhase::Composing || committed_len == 0)
}

/// Unshifted codepoint and consumed modifiers of a GDK key event.
/// `consumed_mods` = modifiers GDK used to pick the keyval (e.g. Shift for
/// 'C'); Ghostty ignores them when encoding the text, as in its GTK runtime.
pub fn key_event_details(
    ctrl: &gtk4::EventControllerKey,
    keycode: u32,
) -> (u32, ffi::ghostty_input_mods_e) {
    use gtk4::gdk::prelude::DisplayExtManual;
    let Some(event) = ctrl
        .current_event()
        .and_then(|e| e.downcast::<gtk4::gdk::KeyEvent>().ok())
    else {
        return (0, 0);
    };
    let entries = event.display().and_then(|d| d.map_keycode(keycode));
    let unshifted = entries.map_or(0, |entries| {
        unshifted_codepoint(
            entries
                .iter()
                .map(|(k, kv)| (k.group(), k.level(), kv.to_unicode().map_or(0, |c| c as u32))),
            event.layout(),
        )
    });
    (unshifted, map_mods(event.consumed_modifiers()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // US layout: the C key produces 'c' (level 0) and 'C' (level 1).
    #[test]
    fn test_unshifted_codepoint_ctrl_c_us_layout() {
        let entries = [(0, 0, 'c' as u32), (0, 1, 'C' as u32)];
        assert_eq!(unshifted_codepoint(entries, 0), 'c' as u32);
    }

    // Two layouts (US + Russian): level 0 of the active layout wins.
    #[test]
    fn test_unshifted_codepoint_uses_active_layout() {
        let entries = [(0, 0, 'c' as u32), (0, 1, 'C' as u32), (1, 0, 0x0441), (1, 1, 0x0421)];
        assert_eq!(unshifted_codepoint(entries, 0), 'c' as u32);
        assert_eq!(unshifted_codepoint(entries, 1), 0x0441);
    }

    #[test]
    fn test_unshifted_codepoint_no_match_is_zero() {
        assert_eq!(unshifted_codepoint([(0, 1, 'C' as u32)], 0), 0);
        assert_eq!(unshifted_codepoint([], 0), 0);
    }

    // Belgian AZERTY: ^ is a dead key. Its press is taken (composing), the next key
    // ends the composition (committed on its own), a plain key goes to Ghostty with its text.
    #[test]
    fn test_ime_takes_key_dead_key_sequence() {
        assert!(ime_takes_key(true, true, KeyPhase::NotComposing, 0), "dead key press");
        assert!(ime_takes_key(true, false, KeyPhase::Composing, 0), "e after ^ -> ê committed");
        assert!(!ime_takes_key(true, false, KeyPhase::NotComposing, 1), "plain a");
        assert!(ime_takes_key(true, false, KeyPhase::NotComposing, 0), "handled, no text");
        assert!(!ime_takes_key(false, false, KeyPhase::NotComposing, 0), "Ctrl+C, arrows");
    }

    #[test]
    fn test_ime_state_store_and_clear() {
        let ime = ImeState::default();
        assert!(ime.committed_ptr().is_null());
        assert!(ime.store("ê"));
        assert_eq!(ime.committed_len(), "ê".len());
        let text = unsafe { std::ffi::CStr::from_ptr(ime.committed_ptr()) };
        assert_eq!(text.to_str().unwrap(), "ê");
        ime.clear();
        assert!(ime.committed_ptr().is_null());
        assert!(!ime.store(&"x".repeat(129)), "too long for the buffer");
    }

    #[test]
    fn test_key_text_char_skips_control_characters() {
        assert_eq!(key_text_char(Some('c')), Some('c'));
        assert_eq!(key_text_char(Some('é')), Some('é'));
        assert_eq!(key_text_char(Some('\r')), None);
        assert_eq!(key_text_char(Some('\u{1b}')), None);
        assert_eq!(key_text_char(None), None);
    }

    /// Modifier combinations preserve exact native flags without unrelated bits.
    #[test]
    fn test_map_mods() {
        use gtk4::gdk::ModifierType;

        assert_eq!(map_mods(ModifierType::empty()), 0);
        assert_eq!(
            map_mods(ModifierType::ALT_MASK),
            ffi::ghostty_input_mods_e_GHOSTTY_MODS_ALT
        );
        assert_eq!(
            map_mods(ModifierType::SUPER_MASK),
            ffi::ghostty_input_mods_e_GHOSTTY_MODS_SUPER
        );
        // Test shift
        let shift = map_mods(ModifierType::SHIFT_MASK);
        assert_eq!(shift, ffi::ghostty_input_mods_e_GHOSTTY_MODS_SHIFT);

        // Test control
        let ctrl = map_mods(ModifierType::CONTROL_MASK);
        assert_eq!(ctrl, ffi::ghostty_input_mods_e_GHOSTTY_MODS_CTRL);

        // Test combined
        let combined = map_mods(ModifierType::SHIFT_MASK | ModifierType::CONTROL_MASK);
        assert_eq!(
            combined,
            ffi::ghostty_input_mods_e_GHOSTTY_MODS_SHIFT
                | ffi::ghostty_input_mods_e_GHOSTTY_MODS_CTRL,
            "Combined modifiers must have both bits set"
        );
    }
}
