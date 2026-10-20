//! Rendering probes behind upstream's `debug.terminal.render_stats` and `debug.panel_snapshot`.

use gtk4::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;

const PRESENT_COUNT: &str = "cmux-present-count";

thread_local! {
    /// Last snapshot per surface: width, height and RGBA bytes (upstream `panelSnapshots`).
    static SNAPSHOTS: RefCell<HashMap<String, (i32, i32, Vec<u8>)>> = RefCell::new(HashMap::new());
}

/// One more frame drawn by this terminal's GL area (upstream `presentCount`).
pub fn count_present(area: &gtk4::GLArea) {
    // SAFETY: the key is only ever stored with a `u64` value, by this module.
    unsafe {
        let count = area.data::<u64>(PRESENT_COUNT).map_or(0, |count| *count.as_ref());
        area.set_data(PRESENT_COUNT, count + 1);
    }
}

/// Frames drawn so far by this GL area.
pub fn present_count(area: &gtk4::GLArea) -> u64 {
    // SAFETY: see `count_present`.
    unsafe { area.data::<u64>(PRESENT_COUNT).map_or(0, |count| *count.as_ref()) }
}

type Captured = (i64, i32, i32, String);

/// Snapshot the area now, or after its next painted frame when a redraw is pending: the
/// widget paintable replays the last render node, which GTK drops until the frame is
/// painted (`gtkwidgetpaintable.c`), so a capture right after a keystroke comes out empty.
/// Upstream instead forces a draw and retries. Gives up after one second without a frame.
pub fn panel_snapshot_when_painted(
    area: &gtk4::GLArea,
    surface_id: String,
    label: String,
    done: impl FnOnce(Result<Captured, String>) + 'static,
) {
    match panel_snapshot(area, &surface_id, &label) {
        Ok(Some(captured)) => return done(Ok(captured)),
        Err(message) => return done(Err(message)),
        Ok(None) => {}
    }
    let Some(clock) = area.frame_clock() else {
        return done(Err("ERROR: Failed to capture panel image".to_owned()));
    };
    let done = std::rc::Rc::new(RefCell::new(Some(done)));
    let handler = std::rc::Rc::new(RefCell::new(None::<gtk4::glib::SignalHandlerId>));
    let id = clock.connect_after_paint({
        let (area, done, handler) = (area.clone(), done.clone(), handler.clone());
        move |clock| {
            let Some(done) = done.borrow_mut().take() else { return };
            if let Some(id) = handler.borrow_mut().take() {
                clock.disconnect(id);
            }
            done(match panel_snapshot(&area, &surface_id, &label) {
                Ok(Some(captured)) => Ok(captured),
                Ok(None) => Err("ERROR: Failed to capture panel image".to_owned()),
                Err(message) => Err(message),
            });
        }
    });
    *handler.borrow_mut() = Some(id);
    area.queue_draw();
    gtk4::glib::timeout_add_local_once(std::time::Duration::from_secs(1), move || {
        if let Some(done) = done.borrow_mut().take() {
            if let Some(id) = handler.borrow_mut().take() {
                clock.disconnect(id);
            }
            done(Err("ERROR: Failed to capture panel image".to_owned()));
        }
    });
}

/// Render the area through its window's renderer, save a PNG, and count the pixels changed
/// since the previous snapshot of the same surface (-1 when there is none, as upstream).
/// `Ok(None)` when the widget has no painted frame to replay yet.
fn panel_snapshot(area: &gtk4::GLArea, surface_id: &str, label: &str) -> Result<Option<Captured>, String> {
    let (width, height) = (area.width(), area.height());
    if width <= 0 || height <= 0 {
        return Err("ERROR: Failed to capture panel image".to_owned());
    }
    let renderer = area
        .native()
        .and_then(|native| native.renderer())
        .ok_or_else(|| "ERROR: Failed to capture panel image".to_owned())?;
    let paintable = gtk4::WidgetPaintable::new(Some(area));
    let snapshot = gtk4::Snapshot::new();
    paintable.snapshot(&snapshot, width as f64, height as f64);
    let Some(node) = snapshot.to_node() else {
        return Ok(None);
    };
    let texture = renderer.render_texture(
        &node,
        Some(&gtk4::graphene::Rect::new(0.0, 0.0, width as f32, height as f32)),
    );
    let (tw, th) = (texture.width(), texture.height());
    let stride = tw as usize * 4;
    let mut pixels = vec![0u8; stride * th as usize];
    texture.download(&mut pixels, stride);

    let changed = SNAPSHOTS.with(|snapshots| {
        let mut snapshots = snapshots.borrow_mut();
        let changed = match snapshots.get(surface_id) {
            Some((pw, ph, previous)) if *pw == tw && *ph == th => previous
                .chunks_exact(4)
                .zip(pixels.chunks_exact(4))
                .filter(|(a, b)| a != b)
                .count() as i64,
            // A size change counts every pixel as changed.
            Some(_) => tw as i64 * th as i64,
            None => -1,
        };
        snapshots.insert(surface_id.to_owned(), (tw, th, pixels));
        changed
    });

    let dir = std::env::temp_dir().join("cmux-screenshots");
    std::fs::create_dir_all(&dir).map_err(|e| format!("ERROR: Failed to write file: {e}"))?;
    let stamp = chrono_free_stamp();
    let name = if label.is_empty() { format!("{stamp}.png") } else { format!("{label}_{stamp}.png") };
    let path = dir.join(name);
    texture
        .save_to_png(&path)
        .map_err(|e| format!("ERROR: Failed to write file: {e}"))?;
    Ok(Some((changed, tw, th, path.to_string_lossy().into_owned())))
}

/// Forget a surface's previous snapshot (upstream `debug.panel_snapshot.reset`).
pub fn panel_snapshot_reset(surface_id: &str) {
    SNAPSHOTS.with(|snapshots| snapshots.borrow_mut().remove(surface_id));
}

fn chrono_free_stamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}_{:08x}", now.as_secs(), uuid::Uuid::new_v4().as_u128() as u32)
}
