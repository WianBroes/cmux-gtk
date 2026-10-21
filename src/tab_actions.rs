//! Tab actions shared by the socket handler and the window's menus and shortcuts.

use crate::app_state::AppState;

/// Which tabs of a pane a close-around call targets, relative to the anchor.
#[derive(Clone, Copy)]
pub enum CloseScope {
    Left,
    Right,
    Others,
}

/// The `split_engines` index whose pane holds `surface`, if any workspace does.
pub fn workspace_of(s: &AppState, surface: &str) -> Option<usize> {
    s.split_engines
        .iter()
        .position(|engine| engine.surface_location(surface).is_some())
}

/// Close the tabs of `surface`'s pane left of it, right of it, or all but it (upstream
/// close_left / close_right / close_others); never the anchor. Returns how many closed.
pub fn close_around(
    s: &mut AppState,
    index: usize,
    surface: &str,
    scope: CloseScope,
) -> Result<usize, (&'static str, &'static str)> {
    let Some((pane_id, slot)) = s.split_engines[index].surface_location(surface) else {
        return Err(("not_found", "surface not found"));
    };
    let Some(pane) = s.split_engines[index]
        .pane_info()
        .into_iter()
        .find(|pane| pane.id == pane_id)
    else {
        return Err(("not_found", "pane not found"));
    };
    let ids = pane.surface_ids;
    let targets: Vec<uuid::Uuid> = match scope {
        CloseScope::Left => ids[..slot].to_vec(),
        CloseScope::Right => ids[(slot + 1).min(ids.len())..].to_vec(),
        CloseScope::Others => {
            let mut others = ids;
            others.remove(slot);
            others
        }
    };
    let mut closed = 0;
    for target in targets {
        if matches!(
            s.split_engines[index].close_surface_and_empty_pane(target),
            crate::split_engine::CloseSurfaceResult::Closed
        ) {
            closed += 1;
        }
    }
    crate::inbox_actions::purge_closed_surfaces(s);
    s.trigger_session_save();
    Ok(closed)
}
