//! GTK side of shared terminal sizing for mobile viewers (upstream
//! docs/shared-terminal-sizing.md): one sizing engine per terminal, the PC
//! pane is participant "pc".

use crate::ghostty::ffi;
use crate::mobile_sizing::{TerminalDeviceKind, TerminalGridSize, TerminalSizingEngine, TerminalSizingParticipant, TerminalSizingPolicy, TerminalSizingState};
use gtk4::prelude::*;

/// Participant id of the PC's own pane.
pub(crate) const PC_ID: &str = "pc";

struct Entry {
    engine: TerminalSizingEngine,
    surface: ffi::ghostty_surface_t,       // to find the entry from a key press without allocating
    area: gtk4::glib::WeakRef<gtk4::GLArea>,
    /// Ghostty's padding in device pixels, measured once while the pane had its full size:
    /// measured on a pane being resized it is transient, and the shared grid oscillated.
    pad: Option<(f64, f64)>,
    /// Tab size in logical pixels the PC grid was last measured at; it is only
    /// re-measured when the tab itself changes size.
    parent_px: (i32, i32),
    own: Option<TerminalGridSize>,
}

thread_local! {
    static ENGINES: std::cell::RefCell<std::collections::HashMap<String, Entry>> = Default::default();
    /// Policy given to new engines (last one set by any phone, kept across restarts),
    /// upstream default Fit everyone.
    static DEFAULT_POLICY: std::cell::RefCell<TerminalSizingPolicy> = std::cell::RefCell::new(load_policy());
}

fn policy_path() -> std::path::PathBuf {
    cmux_platform::paths::state_dir().join("mobile-sizing-policy.json")
}

/// The last policy a phone chose; a missing or unreadable file gives the default.
fn load_policy() -> TerminalSizingPolicy {
    std::fs::read(policy_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_policy(policy: &TerminalSizingPolicy) {
    if let Ok(bytes) = serde_json::to_vec(policy) {
        let _ = std::fs::write(policy_path(), bytes);
    }
}

/// Attach `phone` to `area`'s terminal, creating its engine on first use with
/// the PC's own grid (or 80 x 24 while the pane is hidden), then apply the
/// decided grid to the pane and publish it.
pub(crate) fn viewport(
    area: &gtk4::GLArea,
    surface_id: &str,
    phone: TerminalSizingParticipant,
) -> Option<TerminalSizingState> {
    let surface = crate::split_engine::surface_for_area(area)?;
    ENGINES.with(|cell| {
        let mut engines = cell.borrow_mut();
        let entry = engines.entry(surface_id.to_string()).or_insert_with(|| {
            let pad = measure_pad(area);
            let (parent_px, own) = own_grid(area, pad);
            let initial = own.unwrap_or(TerminalGridSize::new(80, 24));
            let mut engine = TerminalSizingEngine::new(
                initial,
                DEFAULT_POLICY.with(|policy| policy.borrow().clone()),
            );
            let mut pc = TerminalSizingParticipant::new(PC_ID, TerminalDeviceKind::Mac);
            pc.viewport = own;
            pc.device_name = Some("PC".into());
            engine.attach(pc);
            Entry { engine, surface, area: area.downgrade(), pad, parent_px, own }
        });
        remeasure(entry);
        if entry.engine.contains(&phone.id) {
            entry.engine.update_identity(&phone);
            if let Some(viewport) = phone.viewport {
                entry.engine.report(&phone.id, viewport);
            } else {
                entry.engine.clear_viewport(&phone.id);
            }
        } else {
            entry.engine.attach(phone);
        }
        apply(entry);
        Some(entry.engine.state().clone())
    })
}

/// Detach one viewer; when only the PC remains, forget the terminal and give
/// the pane its full size back, else apply the grid the rest still needs.
pub(crate) fn detach(surface_id: &str, participant_id: &str) -> Option<TerminalSizingState> {
    ENGINES.with(|cell| {
        let mut engines = cell.borrow_mut();
        let only_pc = {
            let entry = engines.get_mut(surface_id)?;
            entry.engine.detach(participant_id);
            !entry.engine.participant_ids().any(|id| id != PC_ID)
        };
        if only_pc {
            let removed = engines.remove(surface_id)?;
            if let Some(area) = removed.area.upgrade() {
                release(&area);
                if let (Some(own), Some(pad)) = (removed.own, removed.pad) {
                    size_hidden(&area, own, pad);
                }
            }
            None
        } else {
            let entry = engines.get_mut(surface_id)?;
            apply(entry);
            Some(entry.engine.state().clone())
        }
    })
}

/// Set one terminal's policy, remembering it as the default for engines
/// created later, then apply.
pub(crate) fn set_policy(
    surface_id: &str,
    policy: TerminalSizingPolicy,
) -> Option<TerminalSizingState> {
    ENGINES.with(|cell| {
        DEFAULT_POLICY.with(|default| *default.borrow_mut() = policy.clone());
        save_policy(&policy);
        let mut engines = cell.borrow_mut();
        let entry = engines.get_mut(surface_id)?;
        entry.engine.set_policy(policy);
        apply(entry);
        Some(entry.engine.state().clone())
    })
}

/// Record viewer activity (focus click, key, paste) and apply, so the owner
/// rules see who acted last.
pub(crate) fn note_activity(surface_id: &str, participant_id: &str) -> Option<TerminalSizingState> {
    ENGINES.with(|cell| {
        let mut engines = cell.borrow_mut();
        let entry = engines.get_mut(surface_id)?;
        entry.engine.note_activity(participant_id);
        apply(entry);
        Some(entry.engine.state().clone())
    })
}

/// The PC typed or clicked: record its activity for the entry owning `surface` without
/// allocating, applying only when that changed the published state.
pub(crate) fn note_pc_key(surface: ffi::ghostty_surface_t) {
    ENGINES.with(|cell| {
        let mut engines = cell.borrow_mut();
        if engines.is_empty() {
            return;
        }
        let Some(entry) = engines.values_mut().find(|entry| entry.surface == surface) else {
            return;
        };
        if entry.engine.note_activity(PC_ID) {
            apply(entry);
        }
    });
}

/// Recompute the PC's own grid (its window resized) and apply it when the
/// reported viewport changed.
pub(crate) fn refresh_pc(surface_id: &str) -> Option<TerminalSizingState> {
    ENGINES.with(|cell| {
        let mut engines = cell.borrow_mut();
        let entry = engines.get_mut(surface_id)?;
        remeasure(entry);
        apply(entry);
        Some(entry.engine.state().clone())
    })
}

/// Ghostty's padding plus the leftover under one cell, in device pixels, as it lays
/// out this surface right now. Only meaningful while the pane has its full size.
fn measure_pad(area: &gtk4::GLArea) -> Option<(f64, f64)> {
    let surface = crate::split_engine::surface_for_area(area)?;
    // SAFETY: the surface belongs to this live GLArea on the GTK thread.
    let size = unsafe { ffi::ghostty_surface_size(surface) };
    if size.cell_width_px == 0 || size.cell_height_px == 0 {
        return None;
    }
    Some((
        f64::from(size.width_px) - f64::from(size.columns) * f64::from(size.cell_width_px),
        f64::from(size.height_px) - f64::from(size.rows) * f64::from(size.cell_height_px),
    ))
}

/// The grid `area`'s tab can show, from the tab size and the cell size, with the
/// padding measured once. A pane not shown since start (its tab has no size yet)
/// keeps the grid Ghostty already has, unless a phone shrank it.
fn own_grid(area: &gtk4::GLArea, pad: Option<(f64, f64)>) -> ((i32, i32), Option<TerminalGridSize>) {
    let Some(parent) = area.parent() else {
        return ((0, 0), None);
    };
    let parent_px = (parent.width(), parent.height());
    if parent_px.0 <= 0 || parent_px.1 <= 0 {
        let surface = crate::split_engine::surface_for_area(area);
        let (Some(surface), (-1, -1)) = (surface, area.size_request()) else {
            return (parent_px, None);
        };
        // SAFETY: the surface belongs to this live GLArea on the GTK thread.
        let size = unsafe { ffi::ghostty_surface_size(surface) };
        if size.columns == 0 || size.rows == 0 {
            return (parent_px, None);
        }
        return (parent_px, Some(TerminalGridSize::new(size.columns, size.rows)));
    }
    let (Some((pad_w, pad_h)), Some(surface)) = (pad, crate::split_engine::surface_for_area(area)) else {
        return (parent_px, None);
    };
    // SAFETY: the surface belongs to this live GLArea on the GTK thread.
    let size = unsafe { ffi::ghostty_surface_size(surface) };
    if size.cell_width_px == 0 || size.cell_height_px == 0 {
        return (parent_px, None);
    }
    let scale = f64::from(area.scale_factor());
    let (cell_w, cell_h) = (f64::from(size.cell_width_px), f64::from(size.cell_height_px));
    let cols = ((f64::from(parent_px.0) * scale - pad_w) / cell_w).floor().max(1.0) as u16;
    let rows = ((f64::from(parent_px.1) * scale - pad_h) / cell_h).floor().max(1.0) as u16;
    (parent_px, Some(TerminalGridSize::new(cols, rows)))
}

/// Re-measure the PC grid only when its tab changed size, and report it.
fn remeasure(entry: &mut Entry) {
    let Some(area) = entry.area.upgrade() else { return; };
    let current = area.parent().map(|parent| (parent.width(), parent.height())).unwrap_or((0, 0));
    if current != entry.parent_px || entry.own.is_none() {
        if entry.pad.is_none() && area.size_request() == (-1, -1) {
            entry.pad = measure_pad(&area);
        }
        let (parent_px, own) = own_grid(&area, entry.pad);
        crate::diagnostics::record(
            "mobile.sizing.measure",
            serde_json::json!({"tab_px": [parent_px.0, parent_px.1], "pad": entry.pad.map(|(w, h)| [w, h]),
                "own": own.map(|g| [g.cols, g.rows])}),
        );
        entry.parent_px = parent_px;
        entry.own = own;
    }
    if let Some(grid) = entry.own {
        entry.engine.report(PC_ID, grid);
    }
}

/// Shrink a terminal to at most its shared grid, pinned top-left in its tab,
/// like upstream applyMobileViewportLimit. The GLArea itself shrinks: a
/// Ghostty size smaller than the widget would freeze rendering (see
/// surface.rs resize). A hidden pane is left untouched until it is shown.
fn apply(entry: &Entry) {
    let Some(area) = entry.area.upgrade() else { return; };
    let (Some(own), Some((pad_w, pad_h))) = (entry.own, entry.pad) else { return; };
    let Some(TerminalGridSize { cols: columns, rows }) = limited(own, entry.engine.state().size()) else {
        release(&area);
        size_hidden(&area, own, (pad_w, pad_h));
        return;
    };
    let Some(surface) = crate::split_engine::surface_for_area(&area) else {
        release(&area);
        return;
    };
    // SAFETY: the surface belongs to this live GLArea on the GTK thread.
    let size = unsafe { ffi::ghostty_surface_size(surface) };
    if size.cell_width_px == 0 || size.cell_height_px == 0 {
        release(&area);
        return;
    }
    let scale = f64::from(area.scale_factor());
    let (cell_w, cell_h) = (f64::from(size.cell_width_px), f64::from(size.cell_height_px));
    let width = ((f64::from(columns) * cell_w + pad_w) / scale).round() as i32;
    let height = ((f64::from(rows) * cell_h + pad_h) / scale).round() as i32;
    // Same size again: leave the widget alone, a new request relayouts the pane.
    if area.size_request() == (width, height) {
        return;
    }
    crate::diagnostics::record(
        "mobile.sizing.apply",
        serde_json::json!({"own": [own.cols, own.rows], "grid": [columns, rows], "px": [width, height]}),
    );
    area.set_halign(gtk4::Align::Start);
    area.set_valign(gtk4::Align::Start);
    area.set_size_request(width, height);
    size_hidden(&area, TerminalGridSize::new(columns, rows), (pad_w, pad_h));
}

/// A pane of a workspace not shown gets no allocation, so a size request only
/// lands once it is shown, while the phone watches it now. Size Ghostty
/// directly; nothing renders until it is shown, at the requested size.
fn size_hidden(area: &gtk4::GLArea, grid: TerminalGridSize, (pad_w, pad_h): (f64, f64)) {
    if area.is_mapped() {
        return;
    }
    let Some(surface) = crate::split_engine::surface_for_area(area) else { return; };
    // SAFETY: the surface belongs to this live GLArea on the GTK thread.
    let size = unsafe { ffi::ghostty_surface_size(surface) };
    if size.cell_width_px == 0 || size.cell_height_px == 0 {
        return;
    }
    let phys_w = (f64::from(grid.cols) * f64::from(size.cell_width_px) + pad_w).round() as u32;
    let phys_h = (f64::from(grid.rows) * f64::from(size.cell_height_px) + pad_h).round() as u32;
    // SAFETY: as above.
    unsafe { ffi::ghostty_surface_set_size(surface, phys_w, phys_h) };
}

/// Return the pane to its full tab size.
fn release(area: &gtk4::GLArea) {
    if area.size_request() == (-1, -1) {
        return;
    }
    area.set_size_request(-1, -1);
    area.set_halign(gtk4::Align::Fill);
    area.set_valign(gtk4::Align::Fill);
}

/// `grid` bounded to `own`, component by component; `None` when the pane
/// already fits `grid`, so it must not be shrunk.
fn limited(own: TerminalGridSize, grid: TerminalGridSize) -> Option<TerminalGridSize> {
    let bounded = TerminalGridSize::new(grid.cols.min(own.cols), grid.rows.min(own.rows));
    (bounded != own).then_some(bounded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limited_shrinks_both_axes() {
        let own = TerminalGridSize::new(120, 40);
        assert_eq!(
            limited(own, TerminalGridSize::new(80, 24)),
            Some(TerminalGridSize::new(80, 24))
        );
    }

    #[test]
    fn limited_shrinks_one_axis() {
        let own = TerminalGridSize::new(120, 40);
        assert_eq!(
            limited(own, TerminalGridSize::new(200, 24)),
            Some(TerminalGridSize::new(120, 24))
        );
    }

    #[test]
    fn limited_keeps_equal_or_larger_grid() {
        let own = TerminalGridSize::new(80, 24);
        assert_eq!(limited(own, own), None);
        assert_eq!(limited(own, TerminalGridSize::new(80, 30)), None);
        assert_eq!(limited(own, TerminalGridSize::new(90, 30)), None);
    }
}
