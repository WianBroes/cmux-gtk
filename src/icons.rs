//! Icons cmux ships. The titlebar and pane bars use one pack, Tabler (MIT, Wian's choice,
//! 2026-10-20), under `cmux-*` names no theme defines, so they look the same on any desktop.
//! Other icons cmux names come from the theme; the ones some themes lack (Breeze has no
//! `thunderbolt-symbolic`, so the sidebar's Running status drew "image missing") have Adwaita
//! copies (CC-BY-SA-3.0 OR LGPL-3.0) searched last. Both are written to the cache and added
//! to the icon search path; see THIRD_PARTY_LICENSES.md.

/// Tabler icons by role; stroke-drawn SVGs that GTK recolors like symbolic icons.
const PACK: &[(&str, &[u8])] = &[
    ("cmux-sidebar-symbolic", include_bytes!("../resources/icons/tabler/cmux-sidebar-symbolic.svg")),
    ("cmux-bell-symbolic", include_bytes!("../resources/icons/tabler/cmux-bell-symbolic.svg")),
    ("cmux-bell-off-symbolic", include_bytes!("../resources/icons/tabler/cmux-bell-off-symbolic.svg")),
    ("cmux-pin-symbolic", include_bytes!("../resources/icons/tabler/cmux-pin-symbolic.svg")),
    ("cmux-plus-symbolic", include_bytes!("../resources/icons/tabler/cmux-plus-symbolic.svg")),
    ("cmux-chevron-down-symbolic", include_bytes!("../resources/icons/tabler/cmux-chevron-down-symbolic.svg")),
    ("cmux-back-symbolic", include_bytes!("../resources/icons/tabler/cmux-back-symbolic.svg")),
    ("cmux-forward-symbolic", include_bytes!("../resources/icons/tabler/cmux-forward-symbolic.svg")),
    ("cmux-folder-symbolic", include_bytes!("../resources/icons/tabler/cmux-folder-symbolic.svg")),
    ("cmux-terminal-symbolic", include_bytes!("../resources/icons/tabler/cmux-terminal-symbolic.svg")),
    ("cmux-browser-symbolic", include_bytes!("../resources/icons/tabler/cmux-browser-symbolic.svg")),
    ("cmux-split-right-symbolic", include_bytes!("../resources/icons/tabler/cmux-split-right-symbolic.svg")),
    ("cmux-split-down-symbolic", include_bytes!("../resources/icons/tabler/cmux-split-down-symbolic.svg")),
    ("cmux-minimize-symbolic", include_bytes!("../resources/icons/tabler/cmux-minimize-symbolic.svg")),
    ("cmux-maximize-symbolic", include_bytes!("../resources/icons/tabler/cmux-maximize-symbolic.svg")),
    ("cmux-close-symbolic", include_bytes!("../resources/icons/tabler/cmux-close-symbolic.svg")),
];

const ICONS: &[(&str, &[u8])] = &[
    (
        "preferences-system-notifications-symbolic",
        include_bytes!("../resources/icons/preferences-system-notifications-symbolic.svg"),
    ),
    (
        "text-x-generic-symbolic",
        include_bytes!("../resources/icons/text-x-generic-symbolic.svg"),
    ),
    (
        "thunderbolt-symbolic",
        include_bytes!("../resources/icons/thunderbolt-symbolic.svg"),
    ),
    (
        "web-browser-symbolic",
        include_bytes!("../resources/icons/web-browser-symbolic.svg"),
    ),
];

/// Add the icon pack, and the bundled copies of the icons the display's theme lacks.
/// GTK main thread only.
pub fn install(display: &gtk4::gdk::Display) {
    let theme = gtk4::IconTheme::for_display(display);
    let missing = ICONS.iter().filter(|(name, _)| !theme.has_icon(name));
    let dir = gtk4::glib::user_cache_dir().join("cmux").join("icons");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    for (name, svg) in PACK.iter().chain(missing) {
        let path = dir.join(format!("{name}.svg"));
        if std::fs::read(&path).ok().as_deref() != Some(*svg) && std::fs::write(&path, svg).is_err() {
            return;
        }
    }
    theme.add_search_path(&dir);
}
