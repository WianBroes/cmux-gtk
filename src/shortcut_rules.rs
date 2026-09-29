//! Which key combinations a user may assign to a cmux shortcut.
//!
//! cmux shortcuts fire in the capture phase, before the terminal sees the key. A combination
//! taken by cmux therefore never reaches the program running in the terminal, and agent TUIs
//! (Claude Code, pi, vim, readline, tmux) rely on many of them. The settings page refuses
//! those combinations and says why. Defaults are not checked: only user assignments are.

use gtk4::gdk::{Key, ModifierType};

/// Why a combination is refused, phrased for the person choosing it.
pub type Refusal = String;

fn has(mods: ModifierType, flag: ModifierType) -> bool {
    mods.contains(flag)
}

/// Keys that carry meaning by themselves in a terminal program (submit, cancel, complete…).
fn is_editing_key(key: Key) -> bool {
    matches!(
        key,
        Key::Escape
            | Key::Return
            | Key::KP_Enter
            | Key::Tab
            | Key::ISO_Left_Tab
            | Key::BackSpace
            | Key::Delete
            | Key::space
            | Key::Insert
    )
}

/// Cursor and paging keys: single-modifier forms are word/line motions and scrollback.
fn is_navigation_key(key: Key) -> bool {
    matches!(
        key,
        Key::Left
            | Key::Right
            | Key::Up
            | Key::Down
            | Key::Home
            | Key::End
            | Key::Page_Up
            | Key::Page_Down
    )
}

fn is_function_key(key: Key) -> bool {
    key.name().is_some_and(|name| {
        name.strip_prefix('F')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    })
}

fn is_letter(key: Key) -> bool {
    key.to_unicode().is_some_and(|c| c.is_ascii_alphabetic())
}

/// `None` when the combination may be assigned; otherwise the reason it is refused.
/// `key` is the key as GTK reports it, `mods` the Ctrl/Shift/Alt/Super/Meta state.
pub fn refusal(mods: ModifierType, key: Key) -> Option<Refusal> {
    let ctrl = has(mods, ModifierType::CONTROL_MASK);
    let alt = has(mods, ModifierType::ALT_MASK);
    let shift = has(mods, ModifierType::SHIFT_MASK);
    if mods.intersects(ModifierType::SUPER_MASK | ModifierType::META_MASK | ModifierType::HYPER_MASK)
    {
        return Some(
            "Super/Meta/Hyper is not supported: the desktop usually grabs it, and cmux \
             ignores it when matching keys."
                .into(),
        );
    }
    if !(ctrl || alt || shift) {
        return Some(
            "A shortcut needs a modifier (Ctrl, Alt or Shift): a bare key is typed text and \
             would never reach the terminal."
                .into(),
        );
    }
    if is_function_key(key) {
        return None;
    }
    if shift
        && key
            .to_unicode()
            .is_some_and(|c| !c.is_alphanumeric() && !c.is_whitespace())
    {
        return Some(
            "Shift plus a punctuation key depends on the keyboard layout (Shift+[ is reported \
             as {), so cmux could never match it. Use Ctrl+Alt or a letter instead."
                .into(),
        );
    }
    if is_editing_key(key) && !(ctrl && alt) {
        return Some(
            "Enter, Tab, Escape, Backspace, Delete, Space and Insert (alone or with one \
             modifier) are used by agents and shells: submit, newline, cancel, mode switch, \
             completion. cmux would swallow them."
                .into(),
        );
    }
    if is_navigation_key(key) && !(u8::from(ctrl) + u8::from(alt) + u8::from(shift) >= 2) {
        return Some(
            "Arrow, Home, End and Page keys with a single modifier are word/line motions and \
             scrollback in shells and editors. Use two modifiers (for example Ctrl+Shift)."
                .into(),
        );
    }
    if is_letter(key) && !ctrl && !alt {
        return Some(
            "Shift plus a letter is just a capital letter typed in the terminal.".into(),
        );
    }
    if is_letter(key) && ctrl && !alt && !shift {
        return Some(
            "Ctrl+letter is a control character for the program in the terminal (Ctrl+C \
             interrupts, Ctrl+D ends input, Ctrl+R searches history, Ctrl+B is the tmux \
             prefix…). Add Shift or Alt: Ctrl+Shift+letter or Ctrl+Alt+letter."
                .into(),
        );
    }
    if is_letter(key) && alt && !ctrl && !shift {
        return Some(
            "Alt+letter is used by readline and agents (Alt+B/F word motions, Alt+P/T in \
             Claude Code). Add Ctrl or Shift."
                .into(),
        );
    }
    if ctrl && !alt && !shift {
        let control_char = matches!(
            key,
            Key::bracketleft
                | Key::bracketright
                | Key::backslash
                | Key::underscore
                | Key::asciicircum
                | Key::at
                | Key::slash
        );
        if control_char {
            return Some(
                "Ctrl+[ is Escape, Ctrl+] / Ctrl+\\ / Ctrl+_ / Ctrl+^ / Ctrl+@ are control \
                 characters or signals (Ctrl+\\ quits a process). Add Shift or Alt."
                    .into(),
            );
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: ModifierType = ModifierType::CONTROL_MASK;
    const S: ModifierType = ModifierType::SHIFT_MASK;
    const A: ModifierType = ModifierType::ALT_MASK;

    fn refused(mods: ModifierType, key: Key) -> bool {
        refusal(mods, key).is_some()
    }

    #[test]
    fn bare_keys_and_shift_letters_are_refused() {
        assert!(refused(ModifierType::empty(), Key::a));
        assert!(refused(ModifierType::empty(), Key::F5));
        assert!(refused(S, Key::a));
    }

    #[test]
    fn terminal_control_characters_are_refused_with_a_reason() {
        for key in [Key::c, Key::d, Key::r, Key::l, Key::z, Key::b, Key::w] {
            let reason = refusal(C, key).expect("Ctrl+letter must be refused");
            assert!(reason.contains("control character"), "{reason}");
        }
        assert!(refused(C, Key::bracketleft));
        assert!(refused(C, Key::backslash));
    }

    #[test]
    fn alt_letters_and_agent_keys_are_refused() {
        assert!(refused(A, Key::p));
        assert!(refused(A, Key::b));
        assert!(refused(S, Key::Return));
        assert!(refused(A, Key::Return));
        assert!(refused(C, Key::Tab));
        assert!(refused(S, Key::ISO_Left_Tab));
        assert!(refused(ModifierType::empty(), Key::Escape));
        assert!(refused(C | S, Key::space));
    }

    #[test]
    fn single_modifier_navigation_is_refused_but_two_modifiers_pass() {
        assert!(refused(C, Key::Left));
        assert!(refused(A, Key::Right));
        assert!(refused(S, Key::Page_Up));
        assert!(!refused(C | S, Key::Left));
        assert!(!refused(C | A, Key::Right));
    }

    #[test]
    fn super_is_refused() {
        assert!(refused(ModifierType::SUPER_MASK | C, Key::k));
    }

    #[test]
    fn accepted_combinations() {
        assert!(!refused(C | S, Key::k));
        assert!(!refused(C | A, Key::k));
        assert!(refused(C | S, Key::braceleft));
        assert!(!refused(C | A, Key::bracketleft));
        assert!(!refused(C, Key::F5));
        assert!(!refused(A, Key::F5));
        assert!(!refused(C | A, Key::Return));
        assert!(!refused(C, Key::_1));
    }
}
