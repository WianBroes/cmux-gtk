use gtk4::prelude::*;
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::LazyLock;

#[derive(serde::Serialize, serde::Deserialize)]
struct Preferences {
    font_size: f32,
    #[serde(default)]
    invert_scroll: bool,
    #[serde(default = "enabled")]
    auto_resume_agents: bool,
    #[serde(default = "enabled")]
    desktop_notifications: bool,
    #[serde(default = "default_sidebar_width")]
    sidebar_width: f64,
    #[serde(default)]
    right_sidebar_visible: bool,
    #[serde(default = "default_sidebar_width")]
    right_sidebar_width: f64,
}

fn enabled() -> bool {
    true
}

/// Sidebar width bounds, from upstream's `SessionPersistencePolicy`: 240 points by default and as
/// its own minimum, 600 at most (the divider also stops at a third of the window).
const SIDEBAR_WIDTH_MIN: f64 = 240.0;
const SIDEBAR_WIDTH_MAX: f64 = 600.0;

fn default_sidebar_width() -> f64 {
    SIDEBAR_WIDTH_MIN
}

/// Clamp a stored or dragged width, falling back to the default for a broken value.
fn clamp_sidebar_width(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX)
    } else {
        SIDEBAR_WIDTH_MIN
    }
}

/// Scroll inversion read by every terminal's scroll handler; loaded once, updated on Apply.
static INVERT_SCROLL: LazyLock<AtomicBool> =
    LazyLock::new(|| AtomicBool::new(read(&path()).is_some_and(|prefs| prefs.invert_scroll)));

/// Whether mouse-wheel and touchpad scrolling are inverted.
pub fn invert_scroll() -> bool {
    INVERT_SCROLL.load(Ordering::Relaxed)
}

/// Automatic resume of hook-written agent sessions, on by default like upstream; updated on Apply.
static AUTO_RESUME_AGENTS: LazyLock<AtomicBool> =
    LazyLock::new(|| AtomicBool::new(read(&path()).is_none_or(|prefs| prefs.auto_resume_agents)));

/// Whether agent sessions recorded by the built-in hooks resume without a manual approval.
pub fn auto_resume_agents() -> bool {
    AUTO_RESUME_AGENTS.load(Ordering::Relaxed)
}

/// Desktop (notify-send) notifications, on by default; updated on Apply.
static DESKTOP_NOTIFICATIONS: LazyLock<AtomicBool> = LazyLock::new(|| {
    AtomicBool::new(read(&path()).is_none_or(|prefs| prefs.desktop_notifications))
});

/// Whether bells and agent messages also raise a desktop notification; in-app attention is unaffected.
pub fn desktop_notifications() -> bool {
    DESKTOP_NOTIFICATIONS.load(Ordering::Relaxed)
}

/// Sidebar width in points; dragged on the divider, remembered in `preferences.json`.
static SIDEBAR_WIDTH: LazyLock<AtomicU64> = LazyLock::new(|| {
    let stored = read(&path()).map_or(SIDEBAR_WIDTH_MIN, |prefs| prefs.sidebar_width);
    AtomicU64::new(clamp_sidebar_width(stored).to_bits())
});

/// Current sidebar width in points, already clamped.
pub fn sidebar_width() -> f64 {
    f64::from_bits(SIDEBAR_WIDTH.load(Ordering::Relaxed))
}

/// Remember a dragged width: live at once, on disk for the next launch.
pub fn save_sidebar_width(width: f64) -> Result<(), String> {
    SIDEBAR_WIDTH.store(clamp_sidebar_width(width).to_bits(), Ordering::Relaxed);
    save_current()
}

/// Whether the right sidebar (Files) is open; loaded once, saved on every toggle.
static RIGHT_SIDEBAR_VISIBLE: LazyLock<AtomicBool> = LazyLock::new(|| {
    AtomicBool::new(read(&path()).is_some_and(|prefs| prefs.right_sidebar_visible))
});

/// Whether the right sidebar is open.
pub fn right_sidebar_visible() -> bool {
    RIGHT_SIDEBAR_VISIBLE.load(Ordering::Relaxed)
}

/// Show or hide the right sidebar, remembering the choice for the next launch.
pub fn set_right_sidebar_visible(visible: bool) {
    if right_sidebar_visible() == visible {
        return;
    }
    RIGHT_SIDEBAR_VISIBLE.store(visible, Ordering::Relaxed);
    let _ = save_current();
}

/// Right sidebar width in points; dragged on its divider, persisted like the left one.
static RIGHT_SIDEBAR_WIDTH: LazyLock<AtomicU64> = LazyLock::new(|| {
    let stored = read(&path()).map_or(SIDEBAR_WIDTH_MIN, |prefs| prefs.right_sidebar_width);
    AtomicU64::new(clamp_sidebar_width(stored).to_bits())
});

/// Current right sidebar width in points, already clamped.
pub fn right_sidebar_width() -> f64 {
    f64::from_bits(RIGHT_SIDEBAR_WIDTH.load(Ordering::Relaxed))
}

/// Remember a dragged right sidebar width: live at once, on disk for the next launch.
pub fn save_right_sidebar_width(width: f64) -> Result<(), String> {
    RIGHT_SIDEBAR_WIDTH.store(clamp_sidebar_width(width).to_bits(), Ordering::Relaxed);
    save_current()
}

/// Persist every current preference value; the single write path shared by Apply, the two
/// dividers and the right sidebar toggle, so one writer never drops another writer's fields.
fn save_current() -> Result<(), String> {
    save(
        &path(),
        saved_font_size().unwrap_or(12.0),
        invert_scroll(),
        auto_resume_agents(),
        desktop_notifications(),
    )
}

/// Make the sidebar divider behave like upstream's: drag to resize, width remembered, never
/// narrower than the minimum and never wider than a third of the window (`ContentView.swift`).
pub fn attach_sidebar_resize(paned: &gtk4::Paned) {
    if let Some(sidebar) = paned.start_child() {
        sidebar.set_size_request(SIDEBAR_WIDTH_MIN as i32, -1);
    }
    // Upstream keeps the sidebar width when the window resizes. GTK's defaults scale it with the
    // window and let it fall below its minimum, where it is allocated anyway and clipped.
    paned.set_resize_start_child(false);
    paned.set_shrink_start_child(false);
    paned.set_position(sidebar_width() as i32);
    // The window shrinking lowers the third-of-the-window cap without moving the divider.
    // `max-position` changes during allocation, so the divider is moved afterwards.
    paned.connect_max_position_notify(|paned| {
        let paned = paned.clone();
        gtk4::glib::idle_add_local_once(move || paned.notify("position"));
    });
    let scheduled = Rc::new(Cell::new(false));
    paned.connect_position_notify(move |paned| {
        let position = paned.position() as f64;
        let available = paned.width() as f64;
        if available > 1.0 {
            let cap = (available / 3.0).clamp(SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX);
            if position > cap {
                paned.set_position(cap.round() as i32);
                return;
            }
        }
        if position.to_bits() == SIDEBAR_WIDTH.load(Ordering::Relaxed) {
            return;
        }
        SIDEBAR_WIDTH.store(clamp_sidebar_width(position).to_bits(), Ordering::Relaxed);
        // Write once the drag settles, not on every pixel. The flag replaces a previous timer
        // instead of removing it: `SourceId::remove` panics on a source that already fired.
        if scheduled.replace(true) {
            return;
        }
        let paned = paned.clone();
        let scheduled = scheduled.clone();
        gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
            scheduled.set(false);
            let _ = save_sidebar_width(paned.position() as f64);
        });
    });
}

/// Measured minimum width of the right sidebar. `GtkPaned` clamps `max-position` against
/// exactly this value when `shrink_end_child` is false, which is what keeps the panel from
/// ever being allocated less than it asks for (`gtkpaned.c`, `gtk_paned_compute_position`).
fn right_sidebar_minimum(paned: &gtk4::Paned) -> i32 {
    paned
        .end_child()
        .map(|sidebar| sidebar.measure(gtk4::Orientation::Horizontal, -1).0)
        .unwrap_or(0)
}

/// End child width in points, derived from GTK's own clamp:
/// `max_position = width - handle - end_min`, so `width - handle - position = max - position + end_min`.
/// None before the first allocation, when `max-position` is still unset.
fn right_sidebar_width_of(paned: &gtk4::Paned) -> Option<i32> {
    let available = paned.width();
    if available <= 1 || i64::from(paned.max_position()) > i64::from(available) {
        return None;
    }
    Some(right_sidebar_minimum(paned) + paned.max_position() - paned.position())
}

/// Cap for the right sidebar, the same bound the left sidebar gets: at most a third of the
/// **window** (the content paned only spans the region left of the panel, so its own width
/// would cap the panel far below a third of the window).
fn right_sidebar_cap(paned: &gtk4::Paned) -> f64 {
    let window_width = paned.root().map(|root| root.width()).unwrap_or(0);
    let available = if window_width > 1 {
        window_width
    } else {
        paned.width()
    };
    (f64::from(available) / 3.0).clamp(SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX)
}

/// Move the divider so the right sidebar is `width` points wide, applying the same bounds as
/// the left sidebar (240…600, at most a third of the window). GTK clamps the position itself.
fn set_right_sidebar_width(paned: &gtk4::Paned, width: f64) {
    let Some(available) = (paned.width() > 1).then(|| paned.width()) else {
        return;
    };
    if i64::from(paned.max_position()) > i64::from(available) {
        return;
    }
    let target = clamp_sidebar_width(width)
        .min(right_sidebar_cap(paned))
        .round() as i32;
    let position = (paned.max_position() - (target - right_sidebar_minimum(paned))).max(0);
    if paned.position() != position {
        paned.set_position(position);
    }
}

/// Right sidebar divider, mirrored from `attach_sidebar_resize` onto the end child: it keeps
/// the width it was given when the window resizes, follows the same bounds, and its persisted
/// width survives a relaunch. Dragging writes through the same debounced path.
pub fn attach_right_sidebar_resize(paned: &gtk4::Paned) {
    if let Some(sidebar) = paned.end_child() {
        sidebar.set_size_request(SIDEBAR_WIDTH_MIN as i32, -1);
    }
    // The workspace content (start child) absorbs window resizes; the sidebar keeps its width.
    paned.set_resize_end_child(false);
    paned.set_shrink_end_child(false);
    // The stored width can only land after GTK computed max-position for this window, which
    // also happens on every window resize (the third-of-the-window cap must follow it).
    paned.connect_max_position_notify(|paned| {
        let paned = paned.clone();
        gtk4::glib::idle_add_local_once(move || {
            set_right_sidebar_width(&paned, right_sidebar_width())
        });
    });
    let scheduled = Rc::new(Cell::new(false));
    paned.connect_position_notify(move |paned| {
        // The first allocation places the divider by GTK's own default while `position-set`
        // is still false; storing that value would clobber the persisted width before the
        // max-position handler below has applied it.
        if !paned.property::<bool>("position-set") {
            return;
        }
        let Some(width) = right_sidebar_width_of(paned) else {
            return;
        };
        let cap = right_sidebar_cap(paned);
        if f64::from(width) > cap {
            set_right_sidebar_width(paned, cap);
            return;
        }
        let width = clamp_sidebar_width(f64::from(width));
        if width.to_bits() == RIGHT_SIDEBAR_WIDTH.load(Ordering::Relaxed) {
            return;
        }
        RIGHT_SIDEBAR_WIDTH.store(width.to_bits(), Ordering::Relaxed);
        // Write once the drag settles, not on every pixel (same debounce as the left divider).
        if scheduled.replace(true) {
            return;
        }
        let scheduled = scheduled.clone();
        gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
            scheduled.set(false);
            let _ = save_right_sidebar_width(right_sidebar_width());
        });
    });
}

/// Test-only: pin the gate, so a test of the delivery itself never reads the user's preferences file.
#[cfg(test)]
pub(crate) fn set_desktop_notifications(value: bool) {
    DESKTOP_NOTIFICATIONS.store(value, Ordering::Relaxed);
}

/// Locate terminal preferences beside the application configuration.
fn path() -> PathBuf {
    crate::config::config_path().with_file_name("preferences.json")
}

/// Accept finite font sizes within the supported point-size range.
fn valid(size: f32) -> bool {
    size.is_finite() && (6.0..=72.0).contains(&size)
}

/// Load stored preferences, ignoring missing or malformed files.
fn read(path: &Path) -> Option<Preferences> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Load a valid stored font size, ignoring missing or malformed preferences.
fn read_size(path: &Path) -> Option<f32> {
    read(path)
        .map(|prefs| prefs.font_size)
        .filter(|size| valid(*size))
}

/// Return the optional user-selected terminal size without changing native configuration.
pub fn saved_font_size() -> Option<f32> {
    read_size(&path())
}

/// Validate and atomically persist the preferences, returning user-readable errors.
fn save(
    path: &Path,
    size: f32,
    invert_scroll: bool,
    auto_resume_agents: bool,
    desktop_notifications: bool,
) -> Result<(), String> {
    if !valid(size) {
        return Err("Font size must be between 6 and 72 points.".into());
    }
    let contents = serde_json::to_vec_pretty(&Preferences {
        font_size: size,
        invert_scroll,
        auto_resume_agents,
        desktop_notifications,
        sidebar_width: sidebar_width(),
        right_sidebar_visible: right_sidebar_visible(),
        right_sidebar_width: right_sidebar_width(),
    })
    .map_err(|error| error.to_string())?;
    cmux_platform::filesystem::atomic_write(path, &contents).map_err(|error| error.to_string())
}

/// Snapshot registered native terminal handles for use on the GTK thread.
fn surfaces() -> Vec<usize> {
    crate::ghostty::callbacks::GL_TO_SURFACE
        .lock()
        .map(|registry| registry.values().copied().collect())
        .unwrap_or_default()
}

/// Add a scrollable tab to the Preferences notebook and return its content box.
fn page(notebook: &gtk4::Notebook, title: &str) -> gtk4::Box {
    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    content.set_margin_top(16);
    content.set_margin_bottom(16);
    content.set_margin_start(16);
    content.set_margin_end(16);
    let scrolled = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&content)
        .build();
    notebook.append_page(&scrolled, Some(&gtk4::Label::new(Some(title))));
    content
}

thread_local! {
    /// The live Preferences dialog, so a second request brings the open one forward.
    static OPEN_PREFERENCES: std::cell::RefCell<Option<glib::WeakRef<gtk4::Dialog>>> =
        const { std::cell::RefCell::new(None) };
}

/// Display the font-size editor and apply successful changes to live terminal surfaces.
///
/// One Preferences dialog exists at a time: when it is already open, a second request brings it
/// forward instead of stacking another one (upstream shows a single settings window).
pub fn show(parent: &gtk4::ApplicationWindow, state: &crate::app_state::AppStateRef) {
    // A closed dialog can outlive its window (its own buttons hold a strong reference), so only a
    // still-visible one is brought forward; presenting a closed modal dialog freezes the UI.
    if let Some(open) = OPEN_PREFERENCES
        .with(|open| open.borrow().as_ref().and_then(|weak| weak.upgrade()))
        .filter(|open| open.is_visible())
    {
        open.present();
        return;
    }
    let dialog = gtk4::Dialog::builder()
        .title("Preferences")
        .transient_for(parent)
        .modal(true)
        .default_width(460)
        .default_height(560)
        .build();
    dialog.add_button("Cancel", gtk4::ResponseType::Cancel);
    dialog.add_button("Apply", gtk4::ResponseType::Apply);
    let content = dialog.content_area();
    content.set_spacing(12);
    content.set_margin_top(20);
    content.set_margin_bottom(20);
    content.set_margin_start(20);
    content.set_margin_end(20);
    // Sections follow upstream's settings sidebar: App, then Terminal.
    let notebook = gtk4::Notebook::new();
    notebook.set_vexpand(true);
    let app = page(&notebook, "App");
    let terminal = page(&notebook, "Terminal");
    let shortcuts = page(&notebook, "Shortcuts");
    crate::shortcut_settings::append(&shortcuts, &dialog);
    content.append(&notebook);
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    let label = gtk4::Label::new(Some("Terminal font size (pt)"));
    label.set_hexpand(true);
    label.set_xalign(0.0);
    let size = gtk4::SpinButton::with_range(6.0, 72.0, 0.5);
    size.set_digits(1);
    let current = saved_font_size()
        .or_else(|| {
            surfaces().first().map(|surface| unsafe {
                crate::ghostty::ffi::ghostty_surface_font_size(*surface as _)
            })
        })
        .unwrap_or(12.0);
    size.set_value(current as f64);
    row.append(&label);
    row.append(&size);
    terminal.append(&row);
    let help = gtk4::Label::new(Some(
        "Applies to all terminal tabs, including new tabs.\nSaved for future launches.",
    ));
    help.set_xalign(0.0);
    help.set_wrap(true);
    terminal.append(&help);
    let invert = gtk4::CheckButton::with_label("Invert scrolling (mouse wheel and touchpad)");
    invert.set_active(invert_scroll());
    terminal.append(&invert);
    let auto_resume = gtk4::CheckButton::with_label("Resume agent sessions on reopen");
    auto_resume.set_active(auto_resume_agents());
    terminal.append(&auto_resume);
    let auto_resume_help = gtk4::Label::new(Some(
        "Agents recorded by cmux hooks (Claude, Codex, pi…) restart with their session. Other resume commands still need an approval below.",
    ));
    auto_resume_help.set_xalign(0.0);
    auto_resume_help.set_wrap(true);
    terminal.append(&auto_resume_help);
    let desktop = gtk4::CheckButton::with_label("Desktop notifications");
    desktop.set_active(desktop_notifications());
    app.append(&desktop);
    let desktop_help = gtk4::Label::new(Some(
        "Off: no system popup when an agent finishes or a terminal rings. The bell, unread dot and pane ring stay.",
    ));
    desktop_help.set_xalign(0.0);
    desktop_help.set_wrap(true);
    app.append(&desktop_help);
    crate::resume_review::append(&terminal, state);
    crate::local_tmux_settings::append(&terminal, state, &dialog);
    let error_label = gtk4::Label::new(None);
    error_label.set_wrap(true);
    content.append(&error_label);
    dialog.connect_response(move |dialog, response| {
        if response != gtk4::ResponseType::Apply {
            dialog.close();
            return;
        }
        size.update();
        let value = size.value() as f32;
        if let Err(error) = save(
            &path(),
            value,
            invert.is_active(),
            auto_resume.is_active(),
            desktop.is_active(),
        ) {
            error_label.set_text(&format!("Could not save preferences: {error}"));
            return;
        }
        INVERT_SCROLL.store(invert.is_active(), Ordering::Relaxed);
        AUTO_RESUME_AGENTS.store(auto_resume.is_active(), Ordering::Relaxed);
        DESKTOP_NOTIFICATIONS.store(desktop.is_active(), Ordering::Relaxed);
        let action = format!("set_font_size:{value}");
        let mut failed = false;
        for surface in surfaces() {
            let applied = unsafe {
                crate::ghostty::ffi::ghostty_surface_binding_action(
                    surface as _,
                    action.as_ptr().cast(),
                    action.len(),
                )
            };
            failed |= !applied;
        }
        crate::diagnostics::event(format_args!(
            "terminal font size saved points={value} live_apply_failed={failed}"
        ));
        if failed {
            error_label.set_text(
                "Saved. Some terminals could not update; reopen those tabs to apply the size.",
            );
        } else {
            dialog.close();
        }
    });
    OPEN_PREFERENCES.with(|open| *open.borrow_mut() = Some(dialog.downgrade()));
    dialog.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// Verify a stored sidebar width is bounded like upstream (240…600) whatever the file holds.
    fn sidebar_width_is_clamped_on_read() {
        let dir = std::env::temp_dir().join(format!("cmux-width-{}", uuid::Uuid::new_v4()));
        let path = dir.join("preferences.json");
        // `save` is also what creates the preferences directory.
        save(&path, 14.0, false, true, true).unwrap();
        for (stored, expected) in [(900.0, 600.0), (10.0, 240.0), (320.0, 320.0)] {
            std::fs::write(
                &path,
                format!(r#"{{"font_size": 14.0, "sidebar_width": {stored}}}"#),
            )
            .unwrap();
            assert_eq!(
                clamp_sidebar_width(read(&path).unwrap().sidebar_width),
                expected
            );
        }
        assert_eq!(clamp_sidebar_width(f64::NAN), SIDEBAR_WIDTH_MIN);
        std::fs::write(&path, r#"{"font_size": 14.0}"#).unwrap();
        assert_eq!(read(&path).unwrap().sidebar_width, SIDEBAR_WIDTH_MIN);
    }

    #[test]
    /// The right sidebar's open state and width are read back bounded like the left one's.
    fn right_sidebar_state_roundtrips_and_is_clamped() {
        let dir = std::env::temp_dir().join(format!("cmux-rwidth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("preferences.json");
        for (stored, expected) in [(900.0, 600.0), (10.0, 240.0), (320.0, 320.0)] {
            std::fs::write(
                &path,
                format!(
                    r#"{{"font_size": 14.0, "right_sidebar_visible": true, "right_sidebar_width": {stored}}}"#
                ),
            )
            .unwrap();
            let prefs = read(&path).unwrap();
            assert!(prefs.right_sidebar_visible);
            assert_eq!(clamp_sidebar_width(prefs.right_sidebar_width), expected);
        }
        std::fs::write(&path, r#"{"font_size": 14.0}"#).unwrap();
        let prefs = read(&path).unwrap();
        assert!(!prefs.right_sidebar_visible);
        assert_eq!(
            clamp_sidebar_width(prefs.right_sidebar_width),
            SIDEBAR_WIDTH_MIN
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    /// Verify stored sizes round-trip and unsupported values are rejected.
    fn font_size_roundtrip_and_invalid_values() {
        let dir = std::env::temp_dir().join(format!("cmux-font-{}", uuid::Uuid::new_v4()));
        let path = dir.join("preferences.json");
        assert_eq!(read_size(&path), None);
        save(&path, 15.5, false, false, false).unwrap();
        assert!(!read(&path).unwrap().auto_resume_agents);
        assert!(!read(&path).unwrap().desktop_notifications);
        std::fs::write(&path, r#"{"font_size": 14.0}"#).unwrap();
        assert!(read(&path).unwrap().auto_resume_agents);
        assert!(read(&path).unwrap().desktop_notifications);
        save(&path, 15.5, true, true, true).unwrap();
        assert_eq!(read_size(&path), Some(15.5));
        assert!(read(&path).unwrap().invert_scroll);
        std::fs::write(&path, r#"{"font_size": 14.0}"#).unwrap();
        assert!(!read(&path).unwrap().invert_scroll);
        save(&path, 15.5, false, true, true).unwrap();
        for invalid in [0.0, 73.0, f32::NAN, f32::INFINITY] {
            assert!(save(&path, invalid, true, true, true).is_err());
            assert_eq!(read_size(&path), Some(15.5));
        }
        std::fs::write(&path, "broken json").unwrap();
        assert_eq!(read_size(&path), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
