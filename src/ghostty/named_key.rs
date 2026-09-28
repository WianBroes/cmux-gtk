//! Named key presses (`enter`, `ctrl-c`) synthesized without a physical keyboard.
//!
//! Names map to US-layout XKB hardware keycodes so Ghostty resolves them through
//! its own keycode table, exactly as it does for a press delivered by GTK.

use super::ffi;

/// A key press synthesized from a name like "enter" or "ctrl-c".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NamedKey {
    /// XKB hardware keycode (evdev + 8), US layout position.
    pub keycode: u32,
    /// Modifier mask applied to both halves of the press.
    pub mods: ffi::ghostty_input_mods_e,
    /// Unshifted codepoint, 0 for non-text keys.
    pub unshifted: u32,
}

/// Parse a key name, case-insensitively; `None` when it is not a known name.
///
/// Modifiers are a `-`/`+` separated prefix in any order (`ctrl-c`, `Ctrl+C`,
/// `shift-tab`, `alt-left`); the key is the trailing token. A name that reduces
/// to a bare letter stays a literal character and is rejected here.
pub(crate) fn parse(name: &str) -> Option<NamedKey> {
    let lower = name.to_ascii_lowercase();
    let tokens: Vec<&str> = lower.split(['-', '+']).collect();
    // Longest key first, so `page-up` resolves as a key instead of `up` behind `page`.
    for prefix_len in 0..tokens.len() {
        let key = tokens[prefix_len..].join("-");
        let Some((keycode, unshifted)) = key_code(&key) else {
            continue;
        };
        if prefix_len == 0 && is_bare_letter(&lower) {
            // `c` on its own is still a literal character, not a key press.
            continue;
        }
        let mut mods = ffi::ghostty_input_mods_e_GHOSTTY_MODS_NONE;
        let mut known = true;
        for token in &tokens[..prefix_len] {
            match modifier(token) {
                Some(flag) => mods |= flag,
                None => {
                    known = false;
                    break;
                }
            }
        }
        if !known {
            continue;
        }
        return Some(NamedKey {
            keycode,
            mods,
            unshifted,
        });
    }
    None
}

/// Press then release the key on a live surface (same fields as the keyboard
/// handler in src/ghostty/surface.rs lines 633-676: keycode, mods, action,
/// text = null, unshifted_codepoint, consumed_mods = 0).
///
/// # Safety
/// Same contract as `crate::ghostty::text::send_character`: the caller must keep
/// the surface live on its owning GTK thread, with model borrows released and no
/// teardown or event-loop iteration during this call.
pub(crate) unsafe fn send(surface: ffi::ghostty_surface_t, key: NamedKey) {
    for action in [
        ffi::ghostty_input_action_e_GHOSTTY_ACTION_PRESS,
        ffi::ghostty_input_action_e_GHOSTTY_ACTION_RELEASE,
    ] {
        let mut input = unsafe { std::mem::zeroed::<ffi::ghostty_input_key_s>() };
        input.keycode = key.keycode;
        input.mods = key.mods;
        input.action = action;
        input.text = std::ptr::null();
        input.unshifted_codepoint = key.unshifted;
        input.consumed_mods = ffi::ghostty_input_mods_e_GHOSTTY_MODS_NONE;
        // SAFETY: the caller guarantees a live surface; the initialized input
        // struct is borrowed by Ghostty only for this synchronous call.
        unsafe {
            ffi::ghostty_surface_key(surface, input);
        }
    }
}

/// Map one modifier token to its Ghostty modifier bit.
fn modifier(token: &str) -> Option<ffi::ghostty_input_mods_e> {
    match token {
        "ctrl" | "control" => Some(ffi::ghostty_input_mods_e_GHOSTTY_MODS_CTRL),
        "shift" => Some(ffi::ghostty_input_mods_e_GHOSTTY_MODS_SHIFT),
        "alt" | "option" | "meta" => Some(ffi::ghostty_input_mods_e_GHOSTTY_MODS_ALT),
        "super" | "cmd" => Some(ffi::ghostty_input_mods_e_GHOSTTY_MODS_SUPER),
        _ => None,
    }
}

/// Whether a name is a single letter, which stays a literal character on its own.
fn is_bare_letter(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(
        (bytes.next(), bytes.next()),
        (Some(byte), None) if byte.is_ascii_alphabetic()
    )
}

/// US-layout keycode and unshifted codepoint of a key without modifiers.
fn key_code(name: &str) -> Option<(u32, u32)> {
    match name {
        "enter" | "return" => Some((36, 0)),
        "tab" => Some((23, 0)),
        "escape" | "esc" => Some((9, 0)),
        "backspace" => Some((22, 0)),
        "delete" | "del" => Some((119, 0)),
        "space" => Some((65, ' ' as u32)),
        "insert" => Some((118, 0)),
        "up" => Some((111, 0)),
        "down" => Some((116, 0)),
        "left" => Some((113, 0)),
        "right" => Some((114, 0)),
        "home" => Some((110, 0)),
        "end" => Some((115, 0)),
        "pageup" | "page-up" | "pgup" => Some((112, 0)),
        "pagedown" | "page-down" | "pgdn" => Some((117, 0)),
        "f1" => Some((67, 0)),
        "f2" => Some((68, 0)),
        "f3" => Some((69, 0)),
        "f4" => Some((70, 0)),
        "f5" => Some((71, 0)),
        "f6" => Some((72, 0)),
        "f7" => Some((73, 0)),
        "f8" => Some((74, 0)),
        "f9" => Some((75, 0)),
        "f10" => Some((76, 0)),
        "f11" => Some((95, 0)),
        "f12" => Some((96, 0)),
        _ => letter_code(name),
    }
}

/// US-layout keycode of one lowercase letter, paired with its own codepoint.
fn letter_code(name: &str) -> Option<(u32, u32)> {
    let mut chars = name.chars();
    let letter = chars.next()?;
    if chars.next().is_some() || !letter.is_ascii_lowercase() {
        return None;
    }
    let keycode = match letter {
        'a' => 38,
        'b' => 56,
        'c' => 54,
        'd' => 40,
        'e' => 26,
        'f' => 41,
        'g' => 42,
        'h' => 43,
        'i' => 31,
        'j' => 44,
        'k' => 45,
        'l' => 46,
        'm' => 58,
        'n' => 57,
        'o' => 32,
        'p' => 33,
        'q' => 24,
        'r' => 27,
        's' => 39,
        't' => 28,
        'u' => 30,
        'v' => 55,
        'w' => 25,
        'x' => 53,
        'y' => 29,
        'z' => 52,
        _ => return None,
    };
    Some((keycode, letter as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: ffi::ghostty_input_mods_e = ffi::ghostty_input_mods_e_GHOSTTY_MODS_NONE;

    /// Resolve a name or report the name that failed to parse.
    fn key(name: &str) -> NamedKey {
        parse(name).unwrap_or_else(|| panic!("{name} should parse"))
    }

    /// The table keys resolve case-insensitively to their documented keycodes.
    #[test]
    fn table_names_resolve_to_keycodes() {
        assert_eq!(
            key("enter"),
            NamedKey {
                keycode: 36,
                mods: NONE,
                unshifted: 0
            }
        );
        assert_eq!(
            key("Return"),
            NamedKey {
                keycode: 36,
                mods: NONE,
                unshifted: 0
            }
        );
        assert_eq!(
            key("esc"),
            NamedKey {
                keycode: 9,
                mods: NONE,
                unshifted: 0
            }
        );
        assert_eq!(
            key("pgup"),
            NamedKey {
                keycode: 112,
                mods: NONE,
                unshifted: 0
            }
        );
        assert_eq!(
            key("f12"),
            NamedKey {
                keycode: 96,
                mods: NONE,
                unshifted: 0
            }
        );
        assert_eq!(
            key("shift-tab"),
            NamedKey {
                keycode: 23,
                mods: ffi::ghostty_input_mods_e_GHOSTTY_MODS_SHIFT,
                unshifted: 0
            }
        );
        assert_eq!(
            key("alt-left"),
            NamedKey {
                keycode: 113,
                mods: ffi::ghostty_input_mods_e_GHOSTTY_MODS_ALT,
                unshifted: 0
            }
        );
        assert_eq!(
            key("space"),
            NamedKey {
                keycode: 65,
                mods: NONE,
                unshifted: ' ' as u32
            }
        );
    }

    /// Either separator spelling yields the same control combination.
    #[test]
    fn control_combinations_keep_the_unshifted_letter() {
        let expected = NamedKey {
            keycode: 54,
            mods: ffi::ghostty_input_mods_e_GHOSTTY_MODS_CTRL,
            unshifted: 'c' as u32,
        };
        assert_eq!(key("ctrl-c"), expected);
        assert_eq!(key("Ctrl+C"), expected);
    }

    /// A bare letter stays literal and unknown modifiers or names are rejected.
    #[test]
    fn unknown_names_are_not_keys() {
        for name in ["c", "hyper-c", "ctrl-", ""] {
            assert_eq!(parse(name), None, "{name} must not parse");
        }
    }
}
