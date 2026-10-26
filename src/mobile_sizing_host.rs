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
}

thread_local! {
    static ENGINES: std::cell::RefCell<std::collections::HashMap<String, Entry>> = Default::default();
    /// Policy given to new engines (last one set by any phone), upstream default Fit everyone.
    static DEFAULT_POLICY: std::cell::RefCell<TerminalSizingPolicy> = Default::default();
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
    let own = own_grid(area);
    ENGINES.with(|cell| {
        let mut engines = cell.borrow_mut();
        let entry = engines.entry(surface_id.to_string()).or_insert_with(|| {
            let initial = own.unwrap_or(TerminalGridSize::new(80, 24));
            let mut engine = TerminalSizingEngine::new(
                initial,
                DEFAULT_POLICY.with(|policy| policy.borrow().clone()),
            );
            let mut pc = TerminalSizingParticipant::new(PC_ID, TerminalDeviceKind::Mac);
            pc.viewport = own;
            pc.device_name = Some("PC".into());
            engine.attach(pc);
            Entry { engine, surface, area: area.downgrade() }
        });
        if let Some(grid) = own {
            entry.engine.report(PC_ID, grid);
        }
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

/// The PC typed: record its activity for the entry owning `surface` without
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
        if let Some(area) = entry.area.upgrade() {
            if let Some(grid) = own_grid(&area) {
                entry.engine.report(PC_ID, grid);
            }
        }
        apply(entry);
        Some(entry.engine.state().clone())
    })
}

/// The grid `area` can show right now: its parent's size against the
/// surface's cell metrics, in cells. `None` for a hidden pane (its parent has
/// no size yet) or before the surface reports cells.
fn own_grid(area: &gtk4::GLArea) -> Option<TerminalGridSize> {
    let parent = area.parent()?;
    if parent.width() <= 0 || parent.height() <= 0 {
        return None;
    }
    let surface = crate::split_engine::surface_for_area(area)?;
    // SAFETY: the surface belongs to this live GLArea on the GTK thread.
    let size = unsafe { ffi::ghostty_surface_size(surface) };
    if size.cell_width_px == 0 || size.cell_height_px == 0 {
        return None;
    }
    let scale = f64::from(area.scale_factor());
    let (cell_w, cell_h) = (f64::from(size.cell_width_px), f64::from(size.cell_height_px));
    // Padding plus the leftover under one cell, as Ghostty lays out this surface.
    let pad_w = f64::from(size.width_px) - f64::from(size.columns) * cell_w;
    let pad_h = f64::from(size.height_px) - f64::from(size.rows) * cell_h;
    let cols = ((f64::from(parent.width()) * scale - pad_w) / cell_w).floor().max(1.0) as u16;
    let rows = ((f64::from(parent.height()) * scale - pad_h) / cell_h).floor().max(1.0) as u16;
    Some(TerminalGridSize::new(cols, rows))
}

/// Shrink a terminal to at most its shared grid, pinned top-left in its tab,
/// like upstream applyMobileViewportLimit. The GLArea itself shrinks: a
/// Ghostty size smaller than the widget would freeze rendering (see
/// surface.rs resize). A hidden pane is left untouched until it is shown.
fn apply(entry: &Entry) {
    let Some(area) = entry.area.upgrade() else { return; };
    let Some(own) = own_grid(&area) else { return; };
    let Some(TerminalGridSize { cols: columns, rows }) = limited(own, entry.engine.state().size()) else {
        release(&area);
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
    // Padding plus the leftover under one cell, as Ghostty lays out this surface.
    let pad_w = f64::from(size.width_px) - f64::from(size.columns) * cell_w;
    let pad_h = f64::from(size.height_px) - f64::from(size.rows) * cell_h;
    let width = ((f64::from(columns) * cell_w + pad_w) / scale).round() as i32;
    let height = ((f64::from(rows) * cell_h + pad_h) / scale).round() as i32;
    area.set_halign(gtk4::Align::Start);
    area.set_valign(gtk4::Align::Start);
    area.set_size_request(width, height);
}

/// Return the pane to its full tab size.
fn release(area: &gtk4::GLArea) {
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
