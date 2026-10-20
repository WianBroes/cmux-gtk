//! Icons cmux names that some themes lack (Breeze has no `thunderbolt-symbolic`, so the
//! sidebar's Running status drew "image missing"). Copies from Adwaita (CC-BY-SA-3.0 OR
//! LGPL-3.0, see THIRD_PARTY_LICENSES.md) are written to the cache and searched last: the
//! user's theme still wins for every icon it has, and symbolic recoloring still applies.

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

/// Add the bundled copies of the icons the display's theme lacks. GTK main thread only.
pub fn install(display: &gtk4::gdk::Display) {
    let theme = gtk4::IconTheme::for_display(display);
    let missing: Vec<_> = ICONS.iter().filter(|(name, _)| !theme.has_icon(name)).collect();
    if missing.is_empty() {
        return;
    }
    let dir = gtk4::glib::user_cache_dir().join("cmux").join("icons");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    for (name, svg) in missing {
        let path = dir.join(format!("{name}.svg"));
        if std::fs::read(&path).ok().as_deref() != Some(*svg) && std::fs::write(&path, svg).is_err() {
            return;
        }
    }
    theme.add_search_path(&dir);
}
