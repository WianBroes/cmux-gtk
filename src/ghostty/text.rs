//! Literal terminal input and bounded, non-selecting text snapshots through the native Ghostty API.

use super::ffi;

// Worst-case JSON escaping remains below the four-MiB socket response budget.
const MAX_BYTES: usize = 256 * 1024;

/// Native text allocation tied to its live owning surface until the snapshot is copied.
struct NativeText {
    surface: ffi::ghostty_surface_t,
    text: ffi::ghostty_text_s,
}

impl Drop for NativeText {
    /// Return the native allocation even when UTF-8 validation or copying fails.
    fn drop(&mut self) {
        // SAFETY: the capture function owns this native result and keeps the
        // surface alive on GTK until this guard is dropped.
        unsafe { ffi::ghostty_surface_free_text(self.surface, &mut self.text) };
    }
}

/// Paste literal UTF-8 text, rejecting embedded NUL bytes before native input.
/// Bracketed paste mode can keep a trailing newline from submitting a shell command.
/// The native call borrows the converted string only until it returns.
///
/// # Safety
/// The caller must provide a live surface on its owning GTK thread and prevent
/// teardown throughout this call. Release model borrows before calling Ghostty.
pub(crate) unsafe fn send_literal(
    surface: ffi::ghostty_surface_t,
    text: &str,
) -> Result<(), &'static str> {
    let text = std::ffi::CString::new(text).map_err(|_| "terminal text contains a NUL byte")?;
    // SAFETY: the caller guarantees surface lifetime; the CString remains live
    // throughout the synchronous input call, including its trailing NUL.
    unsafe { ffi::ghostty_surface_text(surface, text.as_ptr(), text.as_bytes().len()) };
    Ok(())
}

/// Backslash-escape shell metacharacters in a dropped path, matching Ghostty's
/// ShellEscapeWriter so agents and shells read it as one literal argument.
pub(crate) fn shell_escape(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len());
    for character in path.chars() {
        if "\\ ()[]{}<>\"'`!#$&;|*?\t".contains(character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// Type one Unicode scalar without bracketed paste; newline becomes carriage return.
/// Reject NUL before delivery. This does not translate named keys or modifiers.
///
/// # Safety
/// The caller must keep the surface live on its owning GTK thread, with model
/// borrows released and no teardown or event-loop iteration during this call.
pub(crate) unsafe fn send_character(
    surface: ffi::ghostty_surface_t,
    character: char,
) -> Result<(), &'static str> {
    if character == '\0' {
        return Err("terminal key contains a NUL byte");
    }
    let mut buffer = [0; 4];
    let text = character.encode_utf8(&mut buffer);
    // SAFETY: the caller guarantees a live surface; native typed input borrows
    // the explicit-length UTF-8 buffer only for this synchronous call.
    unsafe { ffi::ghostty_surface_text_input(surface, text.as_ptr().cast(), text.len()) };
    Ok(())
}

/// Copy up to 256 KiB of clipboard-formatted text from the current viewport.
/// Does not focus, scroll or alter selection. Concurrent terminal output may move
/// rows between the scrollbar snapshot and extraction; this is a best-effort read.
///
/// # Safety
/// The caller must hold a live native surface on its owning GTK thread throughout
/// this call. No teardown or event-loop iteration may occur until it returns.
pub(crate) unsafe fn read_visible(surface: ffi::ghostty_surface_t) -> Result<String, &'static str> {
    let viewport = unsafe { scrollbar(surface) }?;
    if viewport.len == 0 {
        return Ok(String::new());
    }
    let bottom = viewport
        .offset
        .checked_add(viewport.len - 1)
        .ok_or("terminal row exceeds native range")?;
    unsafe { read_rows(surface, viewport.offset, bottom) }
}

/// Rows of scrollback above the current viewport (0 on the alternate screen).
///
/// # Safety
/// Same contract as [`read_visible`].
pub(crate) unsafe fn rows_above_viewport(surface: ffi::ghostty_surface_t) -> Result<u64, &'static str> {
    Ok(unsafe { scrollbar(surface) }?.offset)
}

/// Copy up to `max_lines` rows just above the viewport, for a phone that scrolls its
/// own copy of the history. Returns the text and the row where it ends (exclusive),
/// which equals [`rows_above_viewport`] at the time of the read.
///
/// # Safety
/// Same contract as [`read_visible`].
pub(crate) unsafe fn read_history(
    surface: ffi::ghostty_surface_t,
    max_lines: u64,
) -> Result<(String, u64), &'static str> {
    let end = unsafe { scrollbar(surface) }?.offset;
    if end == 0 || max_lines == 0 {
        return Ok((String::new(), end));
    }
    let top = end.saturating_sub(max_lines);
    Ok((unsafe { read_rows(surface, top, end - 1) }?, end))
}

/// Viewport position and size in rows.
///
/// # Safety
/// Same contract as [`read_visible`].
unsafe fn scrollbar(
    surface: ffi::ghostty_surface_t,
) -> Result<ffi::ghostty_surface_scrollbar_s, &'static str> {
    let mut viewport: ffi::ghostty_surface_scrollbar_s = unsafe { std::mem::zeroed() };
    if !unsafe { ffi::ghostty_surface_scrollbar(surface, &mut viewport) } {
        return Err("terminal viewport unavailable");
    }
    Ok(viewport)
}

/// Clipboard-formatted text of rows `top..=bottom`, at most 256 KiB.
///
/// # Safety
/// Same contract as [`read_visible`].
unsafe fn read_rows(
    surface: ffi::ghostty_surface_t,
    top: u64,
    bottom: u64,
) -> Result<String, &'static str> {
    let top = u32::try_from(top).map_err(|_| "terminal row exceeds native range")?;
    let bottom = u32::try_from(bottom).map_err(|_| "terminal row exceeds native range")?;
    let mut text: ffi::ghostty_text_s = unsafe { std::mem::zeroed() };
    if !unsafe {
        ffi::ghostty_surface_read_screen_clipboard_text(surface, top, bottom, MAX_BYTES, &mut text)
    } {
        return Err("terminal read failed or exceeded the text limit");
    }
    let native = NativeText { surface, text };
    if native.text.text_len == 0 {
        return Ok(String::new());
    }
    if native.text.text.is_null() || native.text.text_len > MAX_BYTES {
        return Err("invalid native text result");
    }
    // SAFETY: successful native capture owns text_len readable bytes until the
    // guard releases them. Validate UTF-8 before copying into Rust ownership.
    let bytes =
        unsafe { std::slice::from_raw_parts(native.text.text.cast::<u8>(), native.text.text_len) };
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| "invalid terminal UTF-8")
}

/// Capture up to 2,000 recent rows as replayable VT within 256 KiB, without selection or viewport changes.
/// Native formatting shrinks the suffix when necessary, preserving complete styles and graphemes.
///
/// # Safety
/// The caller must keep the native surface alive on GTK without event-loop iteration until return.
pub(crate) unsafe fn read_scrollback(
    surface: ffi::ghostty_surface_t,
) -> Result<String, &'static str> {
    let mut text: ffi::ghostty_text_s = unsafe { std::mem::zeroed() };
    // SAFETY: caller owns the surface; native output is bounded and returned as an owned allocation.
    if !unsafe { ffi::ghostty_surface_read_screen_tail_vt(surface, 2000, MAX_BYTES, &mut text) } {
        return Err("terminal scrollback capture failed");
    }
    let native = NativeText { surface, text };
    if native.text.text_len == 0 {
        return Ok(String::new());
    }
    if native.text.text.is_null() || native.text.text_len > MAX_BYTES {
        return Err("invalid native scrollback result");
    }
    // SAFETY: native capture owns text_len readable bytes until the guard releases them.
    let bytes =
        unsafe { std::slice::from_raw_parts(native.text.text.cast::<u8>(), native.text.text_len) };
    let text = std::str::from_utf8(bytes).map_err(|_| "invalid scrollback UTF-8")?;
    crate::scrollback::replay_text(text).ok_or("scrollback exceeds replay limit")
}

#[cfg(test)]
mod tests {
    use super::shell_escape;

    #[test]
    fn shell_escape_keeps_plain_paths_and_escapes_metacharacters() {
        assert_eq!(shell_escape("/home/a/b.png"), "/home/a/b.png");
        assert_eq!(shell_escape("/tmp/my file (1).png"), "/tmp/my\\ file\\ \\(1\\).png");
        assert_eq!(shell_escape("it's$x"), "it\\'s\\$x");
    }
}
