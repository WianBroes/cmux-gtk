//! Configure GDK before it creates the compositor's shared GL context.

/// Apply version-scoped GL workarounds before GTK initialization and worker startup.
pub fn configure() {
    let version = (
        gtk4::major_version(),
        gtk4::minor_version(),
        gtk4::micro_version(),
    );
    append_flags("GDK_DEBUG", &["gl-prefer-gl"]);
    // Match Ghostty's native GTK runtime: its renderer requires desktop GL.
    if version >= (4, 16, 0) {
        append_flags("GDK_DISABLE", &["gles-api", "vulkan"]);
    } else if version >= (4, 14, 0) {
        append_flags("GDK_DEBUG", &["gl-disable-gles", "vulkan-disable"]);
    }
    // These GTK versions can lose the reference to a GL texture's downloaded
    // backing when a dmabuf outlives it. Avoid the affected ownership path.
    // https://github.com/GNOME/gtk/commit/7ff233c7ff2a9949ffd28c9ff55500e1b7578e5e
    // Limit the workaround to the versions audited here; keep desktop GL.
    if ((4, 16, 0)..=(4, 22, 4)).contains(&version) {
        append_flags("GDK_DISABLE", &["dmabuf"]);
    }
    // Since GTK 4.19.2 the Wayland input method no longer composes dead keys and
    // compose sequences itself: it leaves them to the compositor's input method, so
    // without ibus or fcitx (KDE Plasma by default) ^ then e gives nothing, in Ghostty
    // too. GTK's documented way back: GTK_IM_MODULE=simple. Never set over the user's
    // own choice or a running input method (it would break their CJK input).
    // https://download.gnome.org/sources/gtk/4.19/gtk-4.19.2.news
    let im_module_set = std::env::var_os("GTK_IM_MODULE").is_some_and(|v| !v.is_empty());
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
    if needs_simple_im(version, im_module_set, wayland, input_method_running) {
        std::env::set_var("GTK_IM_MODULE", "simple");
    }
    crate::diagnostics::event(format_args!(
        "GTK {}.{}.{} GDK_DEBUG={} GDK_DISABLE={} GSK_RENDERER={} GTK_IM_MODULE={}",
        version.0,
        version.1,
        version.2,
        std::env::var("GDK_DEBUG").unwrap_or_default(),
        std::env::var("GDK_DISABLE").unwrap_or_default(),
        std::env::var("GSK_RENDERER").unwrap_or_else(|_| "auto".into()),
        std::env::var("GTK_IM_MODULE").unwrap_or_default(),
    ));
}

/// Whether to fall back to GTK's own composition: GTK without client-side compose on
/// Wayland, no module chosen by the user, no input method daemon running.
fn needs_simple_im(
    version: (u32, u32, u32),
    im_module_set: bool,
    wayland: bool,
    input_method_running: fn() -> bool,
) -> bool {
    version >= (4, 19, 2) && !im_module_set && wayland && !input_method_running()
}

/// Input method daemons that compose dead keys for GTK through the compositor.
const INPUT_METHODS: &[&str] = &["ibus-daemon", "fcitx5", "fcitx"];

fn input_method_running() -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    entries.flatten().any(|entry| {
        std::fs::read_to_string(entry.path().join("comm"))
            .is_ok_and(|comm| INPUT_METHODS.contains(&comm.trim_end()))
    })
}

/// Add missing GDK flags without replacing explicit user settings or duplicating tokens.
fn append_flags(name: &str, flags: &[&str]) {
    let mut value = std::env::var(name).unwrap_or_default();
    for flag in flags {
        if !value
            .split([':', ' ', ','])
            .any(|existing| existing == *flag)
        {
            if !value.is_empty() {
                value.push(',');
            }
            value.push_str(flag);
        }
    }
    std::env::set_var(name, value);
}

#[cfg(test)]
mod tests {
    use super::needs_simple_im;

    fn running() -> bool {
        true
    }
    fn absent() -> bool {
        false
    }

    #[test]
    fn simple_im_only_without_any_input_method() {
        // KDE Plasma Wayland, GTK 4.22.5, no ibus/fcitx: dead keys need the fallback.
        assert!(needs_simple_im((4, 22, 5), false, true, absent));
        assert!(
            needs_simple_im((4, 19, 2), false, true, absent),
            "first GTK without it"
        );
        assert!(
            !needs_simple_im((4, 19, 1), false, true, absent),
            "GTK still composes"
        );
        assert!(
            !needs_simple_im((4, 22, 5), true, true, absent),
            "user's GTK_IM_MODULE"
        );
        assert!(!needs_simple_im((4, 22, 5), false, false, absent), "X11");
        assert!(
            !needs_simple_im((4, 22, 5), false, true, running),
            "ibus or fcitx"
        );
    }
}
