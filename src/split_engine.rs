//! Workspace pane tree, surface ownership and interactive layout operations.

mod recovery;
mod restore;

use crate::ghostty::ffi;
use gtk4::prelude::*;
use std::sync::atomic::Ordering;
use uuid::Uuid;

/// Owned pane snapshot for protocol listings; numeric identity lasts for this application session.
pub struct PaneInfo {
    pub id: u64,
    pub surface_ids: Vec<Uuid>,
    pub selected_surface: Option<Uuid>,
    /// Parallel to `surface_ids`: `Some(url)` for a browser tab, `None` for a terminal.
    pub browser_urls: Vec<Option<String>>,
}

/// Realized GTK bounds used for pointer automation and layout triage.
pub struct PaneGeometry {
    pub id: u64,
    pub bounds: Option<[f32; 4]>,
    pub surface_bounds: Vec<(Uuid, [f32; 4])>,
}

/// Direction for pane focus navigation (Ctrl+Shift+arrows per D-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusDirection {
    Left,
    Right,
    Up,
    Down,
}

/// A terminal or browser surface shown as a tab inside one pane.
#[derive(Clone)]
pub enum PaneSurface {
    Terminal {
        gl_area: gtk4::GLArea,
        uuid: Uuid,
        resume: Option<crate::resume::ResumeBinding>,
    },
    Browser {
        widgets: crate::browser::PreviewPaneWidgets,
        uuid: Uuid,
    },
}

impl PaneSurface {
    /// Return the stable tab identity shared by persistence and socket commands.
    fn uuid(&self) -> Uuid {
        match self {
            Self::Terminal { uuid, .. } | Self::Browser { uuid, .. } => *uuid,
        }
    }

    /// Clone the GTK page widget without transferring native terminal ownership.
    fn widget(&self) -> gtk4::Widget {
        match self {
            Self::Terminal { gl_area, .. } => gl_area.clone().upcast(),
            Self::Browser { widgets, .. } => widgets.container.clone().upcast(),
        }
    }

    /// Select the default notebook title for a terminal or browser tab.
    fn tab_title(&self) -> &'static str {
        match self {
            Self::Terminal { .. } => "Terminal",
            Self::Browser { .. } => "Browser",
        }
    }

    /// Clone a terminal's GLArea; browser tabs deliberately have no terminal widget.
    fn terminal_area(&self) -> Option<gtk4::GLArea> {
        match self {
            Self::Terminal { gl_area, .. } => Some(gl_area.clone()),
            Self::Browser { .. } => None,
        }
    }

    /// Clone the browser's address entry for navigation or focus, excluding terminal tabs.
    fn url_entry(&self) -> Option<gtk4::Entry> {
        match self {
            Self::Browser { widgets, .. } => Some(widgets.url_entry.clone()),
            Self::Terminal { .. } => None,
        }
    }
}

/// Resolve a realized terminal handle without reading application-owned GTK object data.
fn surface_for_area(area: &gtk4::GLArea) -> Option<ffi::ghostty_surface_t> {
    crate::ghostty::callbacks::GL_TO_SURFACE
        .lock()
        .ok()
        .and_then(|registry| registry.get(&(area.as_ptr() as usize)).copied())
        .map(|surface| surface as ffi::ghostty_surface_t)
}

/// Stop a terminal and remove every global callback route before its widget is
/// detached. Ghostty synchronously stops the PTY/IO and renderer threads in
/// `ghostty_surface_free`, so the shell is gone before GTK destroys the pane.
pub(crate) fn destroy_terminal_area(area: &gtk4::GLArea) {
    unsafe {
        if let Some(retired) =
            area.data::<std::rc::Rc<std::cell::Cell<bool>>>("cmux-surface-retired")
        {
            retired.as_ref().set(true);
        }
        if let Some((bridge, ctx)) = area.steal_data::<(
            std::sync::Arc<crate::ssh::bridge::SshBridge>,
            std::sync::Arc<crate::ssh::bridge::IoWriteContext>,
        )>("cmux-remote-context")
        {
            ctx.surface_ptr.store(0, Ordering::Release);
            bridge.remove_context(ctx.pane_id);
        }
    }
    let raw_area = area.as_ptr();
    // Controllers may receive focus/resize signals during GTK teardown.
    // Clear their shared handle before freeing Ghostty, avoiding callbacks into freed memory.
    unsafe {
        if let Some(cell) = area
            .data::<std::rc::Rc<std::cell::RefCell<Option<ffi::ghostty_surface_t>>>>(
                "cmux-surface-cell",
            )
        {
            *cell.as_ref().borrow_mut() = None;
        }
    }
    // GtkGLArea does not dispose application-owned popover children for us.
    while let Some(child) = area.first_child() {
        child.unparent();
    }
    let surface = crate::ghostty::callbacks::GL_TO_SURFACE
        .lock()
        .ok()
        .and_then(|mut registry| registry.remove(&(raw_area as usize)))
        .map(|raw| raw as ffi::ghostty_surface_t);

    let Some(surface) = surface else {
        return;
    };
    unsafe { ffi::ghostty_surface_set_focus(surface, false) };
    if area.is_realized() {
        area.make_current();
        if area.error().is_none() {
            unsafe { ffi::ghostty_surface_display_unrealized(surface) };
        }
    }
    unsafe { ffi::ghostty_surface_free(surface) };
    // SAFETY: Ghostty has joined its IO thread; no output callback can still borrow this context.
    unsafe {
        area.steal_data::<Box<crate::ghostty::notifications::Context>>("cmux-notification-context");
    }
    crate::ghostty::registry::unregister(surface as usize);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseSurfaceResult {
    Closed,
    LastSurfaceInPane,
    NotFound,
}

/// Result of transferring a live tab without recreating its terminal or browser process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceMoveResult {
    pub pane_id: u64,
    pub position: usize,
}

/// A live GTK surface temporarily detached from one engine during a cross-workspace move.
pub(crate) struct DetachedSurface {
    surface: PaneSurface,
    pub source_pane: u64,
    pub position: usize,
}

/// Route tab-close controls through the window action that owns safe native teardown.
fn request_surface_tab_close(widget: &impl IsA<gtk4::Widget>, uuid: Uuid) {
    let _ = widget.activate_action(
        "win.close-surface-tab",
        Some(&uuid.to_string().to_variant()),
    );
}

/// Build a tab label and close affordance with weak widget captures to avoid ownership cycles.
const TAB_UNREAD_DOT: &str = "tab-unread-dot";
const TAB_TITLE: &str = "tab-title";
const TAB_AGENT_ICON: &str = "tab-agent-icon";

/// Show the mark of the agent recorded for a terminal (upstream `syncTerminalTabAgentIconAsset`):
/// set by the agent's session-start hook, gone with its session-end hook.
fn set_tab_agent_icon(tab: &gtk4::Widget, resume: Option<&crate::resume::ResumeBinding>) {
    let mut child = tab.first_child();
    while let Some(widget) = child {
        if widget.widget_name() == TAB_AGENT_ICON {
            let texture = resume
                .and_then(|binding| binding.kind.as_deref())
                .and_then(crate::agent_icon::texture);
            if let Ok(image) = widget.downcast::<gtk4::Image>() {
                image.set_visible(texture.is_some());
                image.set_paintable(texture.as_ref());
            }
            return;
        }
        child = widget.next_sibling();
    }
}

/// The named child of the tab label shown for a terminal page.
fn tab_part(area: &gtk4::GLArea, name: &str) -> Option<gtk4::Widget> {
    // GtkNotebook pages sit in an internal stack, so the notebook is an ancestor, not the parent.
    let notebook = area
        .ancestor(gtk4::Notebook::static_type())?
        .downcast::<gtk4::Notebook>()
        .ok()?;
    let mut child = notebook.tab_label(area)?.first_child();
    while let Some(widget) = child {
        if widget.widget_name() == name {
            return Some(widget);
        }
        child = widget.next_sibling();
    }
    None
}

fn surface_tab_label(surface: &PaneSurface) -> gtk4::Box {
    let uuid = surface.uuid();
    let tab = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    tab.add_css_class("surface-tab-label");
    // Upstream shows the same unread state as the pane ring as a dot on the tab.
    let dot = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    dot.set_widget_name(TAB_UNREAD_DOT);
    dot.add_css_class("tab-unread-dot");
    dot.set_valign(gtk4::Align::Center);
    dot.set_visible(false);
    let icon = gtk4::Image::new();
    icon.set_widget_name(TAB_AGENT_ICON);
    icon.set_pixel_size(14);
    icon.set_valign(gtk4::Align::Center);
    icon.set_visible(false);
    let label = gtk4::Label::new(Some(surface.tab_title()));
    label.set_widget_name(TAB_TITLE);
    // Truncate like upstream's tabs: a full title would set the pane's minimum width, and a
    // pane allocated below its minimum overflows onto its neighbour.
    label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    let close = gtk4::Button::from_icon_name("window-close-symbolic");
    close.add_css_class("surface-tab-close");
    close.set_tooltip_text(Some("Close Tab"));
    close.set_focusable(false);
    close.connect_clicked({
        let tab = tab.downgrade();
        move |_| {
            if let Some(tab) = tab.upgrade() {
                request_surface_tab_close(&tab, uuid);
            }
        }
    });
    tab.append(&dot);
    tab.append(&icon);
    tab.append(&label);
    tab.append(&close);
    if let PaneSurface::Terminal { resume, .. } = surface {
        set_tab_agent_icon(tab.upcast_ref(), resume.as_ref());
    }

    let popover = gtk4::Popover::new();
    popover.set_parent(&tab);
    popover.set_has_arrow(false);
    let close_item = gtk4::Button::with_label("Close Tab");
    close_item.add_css_class("flat");
    close_item.connect_clicked({
        let tab = tab.downgrade();
        move |_| {
            if let Some(tab) = tab.upgrade() {
                request_surface_tab_close(&tab, uuid);
            }
        }
    });
    popover.set_child(Some(&close_item));
    let gesture = gtk4::GestureClick::new();
    gesture.set_button(3);
    gesture.connect_released({
        let popover = popover.downgrade();
        move |_, _, x, y| {
            let Some(popover) = popover.upgrade() else {
                return;
            };
            popover.set_pointing_to(Some(&gtk4::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.popup();
        }
    });
    tab.add_controller(gesture);
    let drag = gtk4::DragSource::new();
    drag.set_actions(gtk4::gdk::DragAction::MOVE);
    drag.connect_prepare(move |_, _, _| {
        Some(gtk4::gdk::ContentProvider::for_value(
            &uuid.to_string().to_value(),
        ))
    });
    tab.add_controller(drag);
    tab
}

/// Append a reorderable notebook page and its model entry, optionally selecting it.
fn append_pane_surface(
    notebook: &gtk4::Notebook,
    surfaces: &std::rc::Rc<std::cell::RefCell<Vec<PaneSurface>>>,
    surface: PaneSurface,
    select: bool,
) -> u32 {
    if let PaneSurface::Terminal { gl_area, uuid, .. } = &surface {
        // SAFETY: this private key always stores a UUID, before page realization.
        unsafe {
            gl_area.set_data("cmux-surface-uuid", *uuid);
        }
    }
    let label = surface_tab_label(&surface);
    let page = notebook.append_page(&surface.widget(), Some(&label));
    notebook.set_tab_reorderable(&surface.widget(), true);
    surfaces.borrow_mut().push(surface);
    if select {
        notebook.set_current_page(Some(page));
    }
    page
}

/// Launch command for an explicit project command, falling back to a shell when it exits.
fn project_launch(command: &str) -> String {
    format!(
        "/bin/sh -c {}",
        crate::workspace::shell_quote(&format!(
            "/bin/sh -c {}\nexec /bin/sh",
            crate::workspace::shell_quote(command)
        ))
    )
}

/// Zone a tab dropped on a pane lands in: the pane's own tab strip joins its tabs, the outer
/// quarter of the body splits that way, and the middle joins at the drop's x position.
fn pane_drop_direction(x: f64, y: f64, width: f64, height: f64, tab_bar_bottom: f64) -> &'static str {
    if y <= tab_bar_bottom {
        return "center";
    }
    let candidates = [
        (x / width, "left"),
        ((width - x) / width, "right"),
        (y / height, "up"),
        ((height - y) / height, "down"),
    ];
    candidates
        .into_iter()
        .min_by(|(left, _), (right, _)| left.total_cmp(right))
        .filter(|(distance, _)| *distance <= 0.25)
        .map(|(_, direction)| direction)
        .unwrap_or("center")
}

/// Launch overrides for a terminal created on request (CLI `--command`, `--working-directory`).
#[derive(Default)]
pub(crate) struct TerminalLaunch {
    pub(crate) initial_input: Option<String>,
    pub(crate) working_directory: Option<std::path::PathBuf>,
}

/// CSS class keeping a new terminal from taking keyboard focus when its native surface starts.
pub(crate) const NO_INITIAL_FOCUS: &str = "cmux-no-initial-focus";

/// Upstream's tab width cap (Bonsplit `tabMaxWidth`).
const TAB_MAX_WIDTH: i32 = 220;

/// Give each tab title an even share of the tab strip, up to `TAB_MAX_WIDTH` per tab and never
/// more than the full title; a narrow pane shares less and the titles truncate.
fn fit_tab_titles(notebook: &gtk4::Notebook, actions: &gtk4::ScrolledWindow) {
    let pages = notebook.n_pages() as i32;
    let width = notebook.width();
    if pages == 0 || width <= 0 {
        return;
    }
    let (_, actions_width, _, _) = actions.measure(gtk4::Orientation::Horizontal, -1);
    let share = ((width - actions_width) / pages).min(TAB_MAX_WIDTH);
    for index in 0..notebook.n_pages() {
        let Some(tab) = notebook
            .nth_page(Some(index))
            .and_then(|page| notebook.tab_label(&page))
        else {
            continue;
        };
        let mut child = tab.first_child();
        while let Some(widget) = child {
            if widget.widget_name() == TAB_TITLE {
                // The tab's other parts (dot, icon, close, padding) take their share first.
                let (_, label_natural, _, _) = widget.measure(gtk4::Orientation::Horizontal, -1);
                let chrome = tab.parent().map_or(0, |gizmo| {
                    let (_, natural, _, _) = gizmo.measure(gtk4::Orientation::Horizontal, -1);
                    natural - label_natural
                });
                let target = (share - chrome).clamp(0, label_natural);
                if widget.width_request() != target {
                    widget.set_width_request(target);
                }
                break;
            }
            child = widget.next_sibling();
        }
    }
}

/// Construct a tabbed pane and synchronize native focus when its selected page changes.
fn create_pane(pane_id: u64, initial_surface: PaneSurface) -> SplitNode {
    let notebook = gtk4::Notebook::new();
    notebook.add_css_class("surface-tabs");
    notebook.set_scrollable(true);
    notebook.set_show_border(false);
    notebook.set_hexpand(true);
    notebook.set_vexpand(true);

    let actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 2);
    let terminal_btn = gtk4::Button::from_icon_name("utilities-terminal-symbolic");
    terminal_btn.set_tooltip_text(Some("New Tab (Terminal) (Ctrl+T)"));
    terminal_btn.set_action_name(Some("win.new-terminal-tab"));
    terminal_btn.add_css_class("surface-tab-action");
    let browser_btn = gtk4::Button::from_icon_name("web-browser-symbolic");
    browser_btn.set_tooltip_text(Some("New Tab (Browser) (Ctrl+Shift+L)"));
    browser_btn.set_action_name(Some("win.new-browser-tab"));
    browser_btn.add_css_class("surface-tab-action");
    actions.append(&terminal_btn);
    actions.append(&browser_btn);
    // Split this pane, not whichever pane had focus: select it first, then split.
    for (icon, tooltip, action) in [
        (
            "view-dual-symbolic",
            "Split Right (Ctrl+D)",
            "win.split-right",
        ),
        (
            "object-flip-vertical-symbolic",
            "Split Down (Ctrl+Shift+D)",
            "win.split-down",
        ),
    ] {
        let button = gtk4::Button::from_icon_name(icon);
        button.set_tooltip_text(Some(tooltip));
        button.add_css_class("surface-tab-action");
        button.connect_clicked(move |button| {
            let _ = button.activate_action("win.focus-pane", Some(&pane_id.to_variant()));
            let _ = button.activate_action(action, None);
        });
        actions.append(&button);
    }
    // Full width when there is room; a narrow pane clips the buttons instead of taking their
    // width as its minimum.
    let actions_clip = gtk4::ScrolledWindow::new();
    actions_clip.set_policy(gtk4::PolicyType::External, gtk4::PolicyType::Never);
    actions_clip.set_propagate_natural_width(true);
    actions_clip.set_propagate_natural_height(true);
    actions_clip.set_child(Some(&actions));
    notebook.set_action_widget(&actions_clip, gtk4::PackType::End);
    // GtkNotebook draws tabs at their minimum width, so titles are widened here to fill the
    // space the pane has, like the browser pane follows its size: checked each frame, set on change.
    notebook.add_tick_callback(move |notebook, _| {
        fit_tab_titles(notebook, &actions_clip);
        gtk4::glib::ControlFlow::Continue
    });

    let surfaces = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    append_pane_surface(&notebook, &surfaces, initial_surface, true);

    notebook.connect_switch_page({
        let surfaces = std::rc::Rc::downgrade(&surfaces);
        move |notebook, _, page| {
            let Some(surfaces) = surfaces.upgrade() else {
                return;
            };
            let is_active_pane = notebook.has_css_class("active-pane");
            for (index, surface) in surfaces.borrow().iter().enumerate() {
                if let Some(area) = surface.terminal_area() {
                    let selected = is_active_pane && index == page as usize;
                    if selected {
                        area.add_css_class("active-pane");
                        area.grab_focus();
                    } else {
                        area.remove_css_class("active-pane");
                    }
                    if let Some(handle) = surface_for_area(&area) {
                        unsafe { ffi::ghostty_surface_set_focus(handle, selected) };
                    }
                } else if is_active_pane && index == page as usize {
                    if let Some(entry) = surface.url_entry() {
                        let notebook = notebook.downgrade();
                        glib::idle_add_local_once(move || {
                            let Some(notebook) = notebook.upgrade() else {
                                return;
                            };
                            if notebook.has_css_class("active-pane")
                                && notebook.current_page() == Some(page)
                            {
                                entry.grab_focus();
                                entry.select_region(0, -1);
                            }
                        });
                    }
                }
            }
        }
    });

    // GtkNotebook owns the visual tab order during pointer drags. Mirror that order in
    // the application model immediately so socket listings and session snapshots cannot
    // retain the stale pre-drag order.
    notebook.connect_page_reordered({
        let surfaces = std::rc::Rc::downgrade(&surfaces);
        move |notebook, child, new_index| {
            let Some(surfaces) = surfaces.upgrade() else {
                return;
            };
            let mut surfaces = surfaces.borrow_mut();
            let Some(old_index) = surfaces
                .iter()
                .position(|surface| surface.widget() == *child)
            else {
                return;
            };
            let new_index = (new_index as usize).min(surfaces.len().saturating_sub(1));
            if old_index != new_index {
                let surface = surfaces.remove(old_index);
                surfaces.insert(new_index, surface);
                crate::diagnostics::record(
                    "surface.reordered",
                    serde_json::json!({"surface_id": surfaces[new_index].uuid(), "position": new_index}),
                );
                // `reorder_child` emits this signal synchronously. A socket action can
                // therefore still hold AppState's mutable RefCell borrow here. Defer
                // the save notification until that mutation has returned to GTK.
                let notebook = notebook.downgrade();
                glib::idle_add_local_once(move || {
                    if let Some(notebook) = notebook.upgrade() {
                        let _ = notebook.activate_action("win.surface-tabs-changed", None);
                    }
                });
            }
        }
    });

    // Accept stable surface IDs over the pane body. The nearest outer quarter creates a
    // directional split; the tab strip and the center transfer the tab into this pane. The
    // action resolves current ownership at drop time, so no widget/model borrow crosses the
    // callback.
    let drop_target = gtk4::DropTarget::new(String::static_type(), gtk4::gdk::DragAction::MOVE);
    drop_target.connect_drop({
        let surfaces = std::rc::Rc::downgrade(&surfaces);
        move |target, value, x, y| {
            let Ok(uuid) = value.get::<String>() else {
                return false;
            };
            let Some(notebook) = target.widget().and_downcast::<gtk4::Notebook>() else {
                return false;
            };
            let Some(surfaces) = surfaces.upgrade() else {
                return false;
            };
            let width = f64::from(notebook.width().max(1));
            let height = f64::from(notebook.height().max(1));
            // The tab strip sits inside the top quarter, so measure it: a tab dropped on the
            // tabs must join this pane's tabs, not split the pane again.
            let tab_bar_bottom = surfaces
                .borrow()
                .iter()
                .filter_map(|surface| notebook.tab_label(&surface.widget()))
                .filter_map(|label| label.compute_bounds(&notebook))
                .map(|bounds| f64::from(bounds.y() + bounds.height()))
                .fold(0.0_f64, f64::max);
            let direction = pane_drop_direction(x, y, width, height, tab_bar_bottom);
            let position = if direction == "center" {
                let surfaces = surfaces.borrow();
                let mut position = surfaces.len();
                for (index, surface) in surfaces.iter().enumerate() {
                    let Some(label) = notebook.tab_label(&surface.widget()) else {
                        continue;
                    };
                    let Some(bounds) = label.compute_bounds(&notebook) else {
                        continue;
                    };
                    if x < f64::from(bounds.x() + bounds.width() / 2.0) {
                        position = index;
                        break;
                    }
                }
                position.min(surfaces.len().saturating_sub(1)).to_string()
            } else {
                String::new()
            };
            let payload = format!("{uuid}|{pane_id}|{direction}|{position}");
            notebook
                .activate_action("win.surface-drop", Some(&payload.to_variant()))
                .is_ok()
        }
    });
    notebook.add_controller(drop_target);

    SplitNode::Leaf {
        pane_id,
        notebook,
        surfaces,
        has_attention: false,
    }
}

/// Recursive pane layout tree. Each workspace has one root SplitNode.
/// - Leaf: one pane containing one or more terminal/browser surface tabs
/// - Split: two child pane subtrees separated by a GtkPaned divider
///
/// Per SPLIT-06: this is the Bonsplit Rust port — immutable-style tree where
/// split/close operations return a new root.
#[derive(Clone)]
pub enum SplitNode {
    Leaf {
        pane_id: u64,
        notebook: gtk4::Notebook,
        surfaces: std::rc::Rc<std::cell::RefCell<Vec<PaneSurface>>>,
        /// Phase 4 NOTF-01: true when this pane has unread bell activity.
        has_attention: bool,
    },
    Split {
        orientation: gtk4::Orientation,
        paned: gtk4::Paned,
        start: Box<SplitNode>,
        end: Box<SplitNode>,
    },
}

impl SplitNode {
    /// Returns the root GTK widget for this node.
    pub fn widget(&self) -> gtk4::Widget {
        match self {
            SplitNode::Leaf { notebook, .. } => notebook.clone().upcast(),
            SplitNode::Split { paned, .. } => paned.clone().upcast(),
        }
    }

    /// Find the pane_id of the active (focused) leaf by checking CSS class.
    pub fn find_active_pane_id(&self) -> Option<u64> {
        match self {
            SplitNode::Leaf {
                pane_id, notebook, ..
            } => {
                if notebook.has_css_class("active-pane") {
                    Some(*pane_id)
                } else {
                    None
                }
            }
            SplitNode::Split { start, end, .. } => start
                .find_active_pane_id()
                .or_else(|| end.find_active_pane_id()),
        }
    }

    /// Find the UUID for a pane by pane_id. Returns None if not found.
    pub fn find_uuid_for_pane(&self, target_id: u64) -> Option<String> {
        match self {
            SplitNode::Leaf {
                pane_id,
                notebook,
                surfaces,
                ..
            } => {
                if *pane_id == target_id {
                    let index = notebook.current_page().unwrap_or(0) as usize;
                    surfaces
                        .borrow()
                        .get(index)
                        .map(|surface| surface.uuid().to_string())
                } else {
                    None
                }
            }
            SplitNode::Split { start, end, .. } => start
                .find_uuid_for_pane(target_id)
                .or_else(|| end.find_uuid_for_pane(target_id)),
        }
    }

    /// Apply the active-pane CSS class to the leaf matching active_pane_id.
    /// Removes the class from all other leaves.
    pub fn update_focus_css(&self, active_pane_id: u64) {
        match self {
            SplitNode::Leaf {
                pane_id,
                notebook,
                surfaces,
                ..
            } => {
                if *pane_id == active_pane_id {
                    notebook.add_css_class("active-pane");
                } else {
                    notebook.remove_css_class("active-pane");
                }
                let active_index = notebook.current_page().unwrap_or(0) as usize;
                for (index, surface) in surfaces.borrow().iter().enumerate() {
                    if let Some(area) = surface.terminal_area() {
                        if *pane_id == active_pane_id && index == active_index {
                            area.add_css_class("active-pane");
                        } else {
                            area.remove_css_class("active-pane");
                        }
                    }
                }
            }
            SplitNode::Split { start, end, .. } => {
                start.update_focus_css(active_pane_id);
                end.update_focus_css(active_pane_id);
            }
        }
    }

    /// Find a node by pane_id.
    pub fn find_node(&self, target_id: u64) -> Option<&SplitNode> {
        match self {
            SplitNode::Leaf { pane_id, .. } => {
                if *pane_id == target_id {
                    Some(self)
                } else {
                    None
                }
            }
            SplitNode::Split { start, end, .. } => start
                .find_node(target_id)
                .or_else(|| end.find_node(target_id)),
        }
    }

    /// Collect terminal widgets so workspace teardown can stop each PTY while
    /// its GL context is still valid and unregister its callbacks.
    pub fn collect_terminal_areas(&self, out: &mut Vec<gtk4::GLArea>) {
        match self {
            SplitNode::Leaf { surfaces, .. } => {
                for surface in surfaces.borrow().iter() {
                    if let Some(area) = surface.terminal_area() {
                        out.push(area);
                    }
                }
            }
            SplitNode::Split { start, end, .. } => {
                start.collect_terminal_areas(out);
                end.collect_terminal_areas(out);
            }
        }
    }

    /// Find the Ghostty surface handle for a specific pane by pane_id.
    /// Used by debug.type to send text to a specific pane's surface.
    pub fn find_surface_for_pane(&self, target_id: u64) -> Option<ffi::ghostty_surface_t> {
        find_gl_area_in_tree(self, target_id).and_then(|area| surface_for_area(&area))
    }

    /// Collect (uuid, pane_id, active) for all leaves in this subtree.
    pub fn collect_pane_info(&self, out: &mut Vec<(Uuid, u64, bool)>, active_id: u64) {
        match self {
            SplitNode::Leaf {
                pane_id,
                notebook,
                surfaces,
                ..
            } => {
                let selected = notebook.current_page().unwrap_or(0) as usize;
                for (index, surface) in surfaces.borrow().iter().enumerate() {
                    out.push((
                        surface.uuid(),
                        *pane_id,
                        *pane_id == active_id && index == selected,
                    ));
                }
            }
            SplitNode::Split { start, end, .. } => {
                start.collect_pane_info(out, active_id);
                end.collect_pane_info(out, active_id);
            }
        }
    }

    /// Clone the terminal tab widget matching a UUID anywhere in this subtree, including hidden tabs.
    fn find_terminal_by_uuid(&self, target_uuid: &str) -> Option<gtk4::GLArea> {
        match self {
            SplitNode::Leaf { surfaces, .. } => {
                surfaces.borrow().iter().find_map(|surface| match surface {
                    PaneSurface::Terminal { gl_area, uuid, .. }
                        if uuid.to_string() == target_uuid =>
                    {
                        Some(gl_area.clone())
                    }
                    _ => None,
                })
            }
            SplitNode::Split { start, end, .. } => start
                .find_terminal_by_uuid(target_uuid)
                .or_else(|| end.find_terminal_by_uuid(target_uuid)),
        }
    }

    /// Find the pane_id for the leaf matching target_uuid (UUID string).
    pub fn find_pane_id_by_uuid(&self, target_uuid: &str) -> Option<u64> {
        match self {
            SplitNode::Leaf {
                surfaces, pane_id, ..
            } => surfaces
                .borrow()
                .iter()
                .any(|surface| surface.uuid().to_string() == target_uuid)
                .then_some(*pane_id),
            SplitNode::Split { start, end, .. } => start
                .find_pane_id_by_uuid(target_uuid)
                .or_else(|| end.find_pane_id_by_uuid(target_uuid)),
        }
    }

    /// Set has_attention on the leaf matching pane_id. Returns true if found.
    pub fn set_attention(&mut self, target_pane_id: u64, value: bool) -> bool {
        match self {
            SplitNode::Leaf {
                pane_id,
                has_attention,
                ..
            } => {
                if *pane_id == target_pane_id {
                    *has_attention = value;
                    true
                } else {
                    false
                }
            }
            SplitNode::Split { start, end, .. } => {
                start.set_attention(target_pane_id, value)
                    || end.set_attention(target_pane_id, value)
            }
        }
    }

    /// Returns true if any leaf in this subtree has attention.
    pub fn any_attention(&self) -> bool {
        match self {
            SplitNode::Leaf { has_attention, .. } => *has_attention,
            SplitNode::Split { start, end, .. } => start.any_attention() || end.any_attention(),
        }
    }

    /// Check if a specific pane has attention.
    pub fn pane_has_attention(&self, target_pane_id: u64) -> bool {
        match self {
            SplitNode::Leaf {
                pane_id,
                has_attention,
                ..
            } => *pane_id == target_pane_id && *has_attention,
            SplitNode::Split { start, end, .. } => {
                start.pane_has_attention(target_pane_id) || end.pane_has_attention(target_pane_id)
            }
        }
    }

    /// Clear attention on all leaves in this subtree.
    pub fn clear_all_attention(&mut self) {
        match self {
            SplitNode::Leaf { has_attention, .. } => *has_attention = false,
            SplitNode::Split { start, end, .. } => {
                start.clear_all_attention();
                end.clear_all_attention();
            }
        }
    }
}

/// Attach right-click context menu to a terminal GLArea (D-08).
/// Uses button 3 (right-click only) to avoid interfering with Ghostty's mouse handling.
fn attach_terminal_context_menu(gl_area: &gtk4::GLArea) {
    let menu_model = crate::menus::build_terminal_context_menu();
    let popover = gtk4::PopoverMenu::from_model(Some(&menu_model));
    popover.set_parent(gl_area);
    popover.set_has_arrow(false);

    let gesture = gtk4::GestureClick::new();
    gesture.set_button(3); // Right-click only
    gesture.connect_released({
        let popover = popover.downgrade();
        move |_, _, x, y| {
            let Some(popover) = popover.upgrade() else {
                return;
            };
            popover.set_pointing_to(Some(&gtk4::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            let _ = popover.activate_action("win.context-menu-refresh", None);
            popover.popup();
        }
    });
    gl_area.add_controller(gesture);
}

/// SplitEngine manages one workspace's pane layout tree.
pub struct SplitEngine {
    pub root: SplitNode,
    pub active_pane_id: u64,
    /// Monotonically increasing pane ID counter.
    next_pane_id: u64,
    /// Ghostty app handle needed to create new surfaces.
    ghostty_app: ffi::ghostty_app_t,
    /// Workspace binding used for every new local terminal pane.
    working_directory: Option<std::path::PathBuf>,
    pub launch_command: Option<String>,
    /// Validated workspace overrides, copied into each new terminal before cmux routing identity.
    pub launch_environment: std::collections::BTreeMap<String, String>,
    pub remote_launch: Option<crate::ghostty::surface::SurfaceIoMode>,
}

impl SplitEngine {
    /// Take ownership of an initial terminal widget and create one selected pane on GTK.
    /// The widget owns its deferred native-surface initialization and cleanup callbacks.
    pub fn new(
        ghostty_app: ffi::ghostty_app_t,
        initial_gl_area: gtk4::GLArea,
        pane_id: u64,
        working_directory: Option<std::path::PathBuf>,
    ) -> Self {
        attach_terminal_context_menu(&initial_gl_area);
        let root = create_pane(
            pane_id,
            PaneSurface::Terminal {
                gl_area: initial_gl_area,
                uuid: Uuid::new_v4(),
                resume: None,
            },
        );
        root.update_focus_css(pane_id);
        SplitEngine {
            root,
            active_pane_id: pane_id,
            next_pane_id: pane_id + 1,
            ghostty_app,
            working_directory,
            launch_command: None,
            launch_environment: std::collections::BTreeMap::new(),
            remote_launch: None,
        }
    }

    /// Returns the root widget of this workspace's split tree.
    pub fn root_widget(&self) -> gtk4::Widget {
        self.root.widget()
    }

    /// Grab GTK keyboard focus for the active pane's GLArea.
    /// Called after workspace switch so key events route to Ghostty, not the sidebar.
    pub fn grab_active_focus(&self) {
        if let Some(gl_area) = self.find_gl_area(self.active_pane_id) {
            gl_area.grab_focus();
        } else if let Some(entry) = find_url_entry_in_tree(&self.root, self.active_pane_id) {
            entry.grab_focus();
        }
    }

    /// Returns the UUID of the currently active pane, if found.
    pub fn active_pane_uuid(&self) -> Option<String> {
        self.root.find_uuid_for_pane(self.active_pane_id)
    }

    /// Restore GTK and native focus to this workspace's selected surface on the GTK thread.
    /// Resolve ownership through the pane tree; CSS classes in other workspaces are not identity.
    pub fn focus_active_surface(&self) {
        self.grab_active_focus();
        if let Some(area) = self.find_gl_area(self.active_pane_id) {
            if let Some(surface) = surface_for_area(&area) {
                // SAFETY: the selected widget owns this live native handle; lookup releases
                // the registry lock before calling Ghostty, which may invoke GTK callbacks.
                unsafe { ffi::ghostty_surface_set_focus(surface, true) };
            }
            if area.is_realized() {
                area.queue_render();
            }
        }
    }

    /// Mark a pane active without changing GTK focus. Pointer handlers use this
    /// before focusing the exact child that was clicked.
    pub fn activate_pane(&mut self, pane_id: u64) -> bool {
        if self.root.find_node(pane_id).is_none() {
            return false;
        }
        self.active_pane_id = pane_id;
        self.root.update_focus_css(pane_id);
        true
    }

    /// Select a surface's notebook page and owning pane, then move GTK keyboard focus there.
    /// Return false without changing selection when the surface is absent from this workspace.
    pub fn focus_surface(&mut self, uuid: &str) -> bool {
        let Some(pane_id) = self.root.find_pane_id_by_uuid(uuid) else {
            return false;
        };
        let Some((notebook, surfaces)) = find_pane_tabs(&self.root, pane_id) else {
            return false;
        };
        let page = surfaces
            .borrow()
            .iter()
            .find(|surface| surface.uuid().to_string() == uuid)
            .and_then(|surface| notebook.page_num(&surface.widget()));
        let Some(page) = page else {
            return false;
        };
        // Release the surface-list borrow before GTK emits switch-page callbacks.
        self.active_pane_id = pane_id;
        notebook.set_current_page(Some(page));
        self.root.update_focus_css(pane_id);
        self.grab_active_focus();
        true
    }

    /// Split the active pane to the right (Ctrl+D per D-10).
    /// Replaces the active Leaf with a Split(Horizontal) containing the old leaf + new leaf.
    /// Per D-08: new surface inherits CWD via ghostty_surface_inherited_config.
    /// Per D-09: initial split ratio is 50/50 (set in paned.connect_realize).
    /// Per SPLIT-07: new pane receives focus immediately.
    pub fn split_right(&mut self) -> Option<u64> {
        self.split_active(gtk4::Orientation::Horizontal)
    }

    /// Split the active pane downward (Ctrl+Shift+D per D-10).
    pub fn split_down(&mut self) -> Option<u64> {
        self.split_active(gtk4::Orientation::Vertical)
    }

    /// Split the active pane and focus a new terminal, inheriting native context when available.
    /// Browser-only panes use workspace launch settings. Return None for a missing active pane.
    pub fn split_active(&mut self, orientation: gtk4::Orientation) -> Option<u64> {
        let active_id = self.active_pane_id;
        self.root.find_node(active_id)?;
        let new_pane_id = self.next_pane_id;
        self.next_pane_id += 1;

        // When the root is a Leaf (first split), the GLArea is a direct child of the GtkStack
        // page. The replacer will remove it from the Stack (via remove_widget_from_parent) and
        // place it inside the new Paned. We then need to add the Paned to the Stack page.
        // Only capture this for Leaf roots — for nested splits the outer Paned stays in the Stack.
        let old_root_widget = self.root.widget();
        let stack_slot: Option<(gtk4::Stack, String)> =
            if matches!(self.root, SplitNode::Leaf { .. }) {
                old_root_widget
                    .parent()
                    .and_then(|p| p.downcast::<gtk4::Stack>().ok())
                    .and_then(|stack| {
                        let name = stack.page(&old_root_widget).name()?.to_string();
                        Some((stack, name))
                    })
            } else {
                None
            };

        // SAFETY: the selected native surface is live on GTK; the returned owner
        // retains its allocated directory through deferred widget initialization.
        let inherited_config =
            find_any_terminal_surface(&self.root, active_id).map(|surface| unsafe {
                // Stop the previous terminal receiving input before the new pane takes focus.
                ffi::ghostty_surface_set_focus(surface, false);
                crate::ghostty::inherited::InheritedConfig::from_surface(
                    surface,
                    ffi::ghostty_surface_context_e_GHOSTTY_SURFACE_CONTEXT_SPLIT,
                )
            });
        let new_gl_area = self.create_terminal_widget(new_pane_id, inherited_config);

        // Replace the active leaf in the tree with a Split node.
        let new_leaf = create_pane(
            new_pane_id,
            PaneSurface::Terminal {
                gl_area: new_gl_area.clone(),
                uuid: Uuid::new_v4(),
                resume: None,
            },
        );

        self.replace_leaf_with_split(active_id, new_leaf, orientation)?;

        // If the root was a Leaf, it's now a Split whose Paned has no parent.
        // Re-parent the new Paned root into the GtkStack page we saved above.
        if let Some((stack, name)) = stack_slot {
            let new_root = self.root.widget();
            stack.add_named(&new_root, Some(&name));
            stack.set_visible_child_name(&name);
        }

        // After realize, update active focus to the new pane.
        self.active_pane_id = new_pane_id;
        self.root.update_focus_css(new_pane_id);

        // Focus the new GLArea widget so it receives keyboard events.
        new_gl_area.grab_focus();

        Some(new_pane_id)
    }

    /// Construct a terminal widget on GTK with shared workspace launch and context-menu policy.
    /// Native initialization is deferred to realization; the widget owns surface cleanup.
    fn create_terminal_widget(
        &self,
        pane_id: u64,
        inherited: Option<crate::ghostty::inherited::InheritedConfig>,
    ) -> gtk4::GLArea {
        let (gl_area, _surface_cell) = crate::ghostty::surface::create_surface(
            self.ghostty_app,
            inherited,
            self.working_directory.clone(),
            pane_id,
            self.remote_launch.clone().unwrap_or_else(|| {
                crate::ghostty::surface::SurfaceIoMode::Configured {
                    initial_input: None,
                    command: self.launch_command.clone(),
                    environment: self.launch_environment.clone(),
                }
            }),
        );
        attach_terminal_context_menu(&gl_area);
        gl_area
    }

    /// Launch an explicit local project command in a selected sibling tab without changing future launch defaults.
    pub(crate) fn new_project_command(
        &mut self,
        command: &str,
        directory: std::path::PathBuf,
    ) -> Option<Uuid> {
        if self.remote_launch.is_some() {
            return None;
        }
        let previous_command = self.launch_command.replace(project_launch(command));
        let previous_directory = self.working_directory.replace(directory);
        let result = self.new_terminal_tab();
        self.launch_command = previous_command;
        self.working_directory = previous_directory;
        result
    }

    /// Launch an explicit local command in a new pane to the right, like Split Right.
    pub(crate) fn split_right_command(&mut self, command: &str) -> Option<u64> {
        if self.remote_launch.is_some() {
            return None;
        }
        let previous_command = self.launch_command.replace(project_launch(command));
        let result = self.split_right();
        self.launch_command = previous_command;
        result
    }

    /// Build a terminal for a socket creation request: `initial_input` is typed into the
    /// interactive shell followed by Enter, like upstream's `initial_input`. Without `focus`
    /// the terminal does not take keyboard focus when its native surface starts.
    fn create_requested_terminal(
        &self,
        pane_id: u64,
        inherited: Option<crate::ghostty::inherited::InheritedConfig>,
        launch: TerminalLaunch,
        focus: bool,
    ) -> Result<gtk4::GLArea, &'static str> {
        let io_mode = match (self.remote_launch.clone(), launch.initial_input) {
            (Some(mut remote), input) => {
                if let crate::ghostty::surface::SurfaceIoMode::Remote { initial_input, .. } =
                    &mut remote
                {
                    *initial_input = input.map(|text| format!("{text}\r").into_bytes());
                }
                remote
            }
            // Ghostty drops the launch command when input is given (Mosh, startup scripts).
            (None, Some(_)) if self.launch_command.is_some() => {
                return Err("--command is not supported in a workspace with a launch command");
            }
            (None, initial_input) => crate::ghostty::surface::SurfaceIoMode::Configured {
                initial_input,
                command: self.launch_command.clone(),
                environment: self.launch_environment.clone(),
            },
        };
        let (gl_area, _) = crate::ghostty::surface::create_surface(
            self.ghostty_app,
            inherited,
            launch
                .working_directory
                .or_else(|| self.working_directory.clone()),
            pane_id,
            io_mode,
        );
        if !focus {
            gl_area.add_css_class(NO_INITIAL_FOCUS);
        }
        attach_terminal_context_menu(&gl_area);
        Ok(gl_area)
    }

    /// Split `target_pane` with a new terminal on `direction`'s side of it. Keyboard focus and
    /// the active pane move to the new terminal only with `focus`.
    pub(crate) fn split_pane_terminal(
        &mut self,
        target_pane: u64,
        direction: FocusDirection,
        launch: TerminalLaunch,
        focus: bool,
    ) -> Result<(u64, Uuid), &'static str> {
        if self.root.find_node(target_pane).is_none() {
            return Err("pane not found");
        }
        let orientation = match direction {
            FocusDirection::Left | FocusDirection::Right => gtk4::Orientation::Horizontal,
            FocusDirection::Up | FocusDirection::Down => gtk4::Orientation::Vertical,
        };
        let before = matches!(direction, FocusDirection::Left | FocusDirection::Up);
        // SAFETY: the pane lookup returns a live GTK-owned native surface. The
        // non-copying result owns its directory independently of that source.
        let inherited = find_any_terminal_surface(&self.root, target_pane).map(|surface| unsafe {
            crate::ghostty::inherited::InheritedConfig::from_surface(
                surface,
                ffi::ghostty_surface_context_e_GHOSTTY_SURFACE_CONTEXT_SPLIT,
            )
        });
        let new_pane_id = self.next_pane_id;
        let gl_area = self.create_requested_terminal(new_pane_id, inherited, launch, focus)?;
        self.next_pane_id += 1;
        let uuid = Uuid::new_v4();
        let new_leaf = create_pane(
            new_pane_id,
            PaneSurface::Terminal {
                gl_area: gl_area.clone(),
                uuid,
                resume: None,
            },
        );
        let stack_slot = if matches!(self.root, SplitNode::Leaf { .. }) {
            self.root
                .widget()
                .parent()
                .and_then(|parent| parent.downcast::<gtk4::Stack>().ok())
                .and_then(|stack| {
                    let name = stack.page(&self.root.widget()).name()?.to_string();
                    Some((stack, name))
                })
        } else {
            None
        };
        self.replace_leaf_with_split_position(target_pane, new_leaf, orientation, before)
            .ok_or("pane not found")?;
        if let Some((stack, name)) = stack_slot {
            let root = self.root.widget();
            stack.add_named(&root, Some(&name));
            stack.set_visible_child_name(&name);
        }
        if focus {
            if let Some(surface) = self.find_surface(self.active_pane_id) {
                // SAFETY: the active pane's native surface is live on GTK.
                unsafe { ffi::ghostty_surface_set_focus(surface, false) };
            }
            self.active_pane_id = new_pane_id;
            self.root.update_focus_css(new_pane_id);
            gl_area.grab_focus();
        }
        Ok((new_pane_id, uuid))
    }

    /// Add a terminal tab to `pane_id`; keyboard focus and the active pane move to it only with
    /// `focus`. Without it the tab is still selected in another pane, so its shell starts; in the
    /// active pane selecting would take focus, so it waits there until the tab is opened.
    pub(crate) fn new_terminal_surface(
        &mut self,
        pane_id: u64,
        launch: TerminalLaunch,
        focus: bool,
    ) -> Result<Uuid, &'static str> {
        let (notebook, surfaces) = find_pane_tabs(&self.root, pane_id).ok_or("pane not found")?;
        // SAFETY: as in `new_terminal_tab`.
        let inherited = find_any_terminal_surface(&self.root, pane_id).map(|surface| unsafe {
            crate::ghostty::inherited::InheritedConfig::from_surface(
                surface,
                ffi::ghostty_surface_context_e_GHOSTTY_SURFACE_CONTEXT_TAB,
            )
        });
        let gl_area = self.create_requested_terminal(pane_id, inherited, launch, focus)?;
        if focus {
            if let Some(surface) = self.find_surface(self.active_pane_id) {
                // SAFETY: the active pane's native surface is live on GTK.
                unsafe { ffi::ghostty_surface_set_focus(surface, false) };
            }
        }
        let uuid = Uuid::new_v4();
        append_pane_surface(
            &notebook,
            &surfaces,
            PaneSurface::Terminal {
                gl_area: gl_area.clone(),
                uuid,
                resume: None,
            },
            focus || pane_id != self.active_pane_id,
        );
        if focus {
            self.active_pane_id = pane_id;
            self.root.update_focus_css(pane_id);
            gl_area.grab_focus();
        }
        Ok(uuid)
    }

    /// Pane named by `pane:N` or by the UUID of one of its surfaces, if it is in this workspace.
    pub(crate) fn pane_for_ref(&self, reference: &str) -> Option<u64> {
        match reference.strip_prefix("pane:") {
            Some(number) => number
                .parse::<u64>()
                .ok()
                .filter(|pane_id| self.contains_pane(*pane_id)),
            None => self.find_pane_id_by_uuid(reference),
        }
    }

    /// Pane currently shown as active in this workspace.
    pub(crate) fn active_pane(&self) -> u64 {
        self.active_pane_id
    }

    /// Create and select a terminal surface tab in the focused pane.
    pub fn new_terminal_tab(&mut self) -> Option<Uuid> {
        let pane_id = self.active_pane_id;
        // SAFETY: the pane lookup returns a live GTK-owned native surface. The
        // non-copying result owns its directory independently of that source.
        let inherited = find_any_terminal_surface(&self.root, pane_id).map(|surface| unsafe {
            crate::ghostty::inherited::InheritedConfig::from_surface(
                surface,
                ffi::ghostty_surface_context_e_GHOSTTY_SURFACE_CONTEXT_TAB,
            )
        });
        if let Some(surface) = self.find_surface(pane_id) {
            unsafe { ffi::ghostty_surface_set_focus(surface, false) };
        }
        let gl_area = self.create_terminal_widget(pane_id, inherited);
        let uuid = Uuid::new_v4();
        let (notebook, surfaces) = find_pane_tabs(&self.root, pane_id)?;
        append_pane_surface(
            &notebook,
            &surfaces,
            PaneSurface::Terminal {
                gl_area: gl_area.clone(),
                uuid,
                resume: None,
            },
            true,
        );
        self.root.update_focus_css(pane_id);
        gl_area.grab_focus();
        Some(uuid)
    }

    /// Select a pane and create a terminal tab there (used by Ghostty actions).
    pub fn new_terminal_tab_for_pane(&mut self, pane_id: u64) -> Option<Uuid> {
        self.root.find_node(pane_id)?;
        self.active_pane_id = pane_id;
        self.root.update_focus_css(pane_id);
        self.new_terminal_tab()
    }

    /// Create and select a browser surface tab in the focused pane.
    pub fn split_active_with_preview(&mut self) -> Option<crate::browser::PreviewPaneWidgets> {
        self.add_preview(true)
    }

    /// Append a browser surface, optionally selecting it and moving keyboard focus to its address.
    pub fn add_preview(&mut self, select: bool) -> Option<crate::browser::PreviewPaneWidgets> {
        self.add_preview_with_profile(select, None)
    }

    /// Append a browser surface with an explicit persistent agent-browser profile selector.
    pub fn add_preview_with_profile(
        &mut self,
        select: bool,
        profile: Option<String>,
    ) -> Option<crate::browser::PreviewPaneWidgets> {
        let active_id = self.active_pane_id;
        let mut widgets = crate::browser::create_preview_pane();
        widgets.profile = profile;

        // Phase 9: Attach right-click context menu to browser preview (D-09)
        {
            let menu_model = crate::menus::build_browser_context_menu();
            let popover = gtk4::PopoverMenu::from_model(Some(&menu_model));
            popover.set_parent(&widgets.container);
            popover.set_has_arrow(false);

            let gesture = gtk4::GestureClick::new();
            gesture.set_button(3); // Right-click only
            gesture.connect_released({
                let popover = popover.clone();
                move |_, _, x, y| {
                    popover.set_pointing_to(Some(&gtk4::gdk::Rectangle::new(
                        x as i32, y as i32, 1, 1,
                    )));
                    popover.popup();
                }
            });
            widgets.container.add_controller(gesture);
        }

        let (notebook, surfaces) = find_pane_tabs(&self.root, active_id)?;
        append_pane_surface(
            &notebook,
            &surfaces,
            PaneSurface::Browser {
                widgets: widgets.clone(),
                uuid: widgets.uuid,
            },
            select,
        );
        self.root.update_focus_css(active_id);
        if select {
            widgets.url_entry.grab_focus();
            widgets.url_entry.select_region(0, -1);
        }

        Some(widgets)
    }

    /// Replace the leaf with `target_pane_id` with a Split(orientation) node.
    /// Returns Some(()) on success, None if the leaf was not found.
    fn replace_leaf_with_split(
        &mut self,
        target_pane_id: u64,
        new_leaf: SplitNode,
        orientation: gtk4::Orientation,
    ) -> Option<()> {
        self.replace_leaf_with_split_position(target_pane_id, new_leaf, orientation, false)
    }

    /// Insert a prepared pane on either side of an existing leaf.
    fn replace_leaf_with_split_position(
        &mut self,
        target_pane_id: u64,
        new_leaf: SplitNode,
        orientation: gtk4::Orientation,
        before: bool,
    ) -> Option<()> {
        let orientation_cap = orientation;
        let mut replacer = Some(|old_leaf: SplitNode| {
            let old_widget = old_leaf.widget();
            let new_widget = new_leaf.widget();

            // GTK4 requires a widget to have no parent before set_start/end_child.
            // old_widget may be parented to the Stack (first split) or an outer Paned (nested).
            remove_widget_from_parent(&old_widget);

            let paned = gtk4::Paned::new(orientation_cap);
            // Both children must be allowed to resize — GTK4 default for resize_end_child
            // is TRUE but be explicit to ensure drag works in both directions.
            paned.set_resize_start_child(true);
            paned.set_resize_end_child(true);
            // Nested panes must be allowed below their natural size. Requiring every
            // terminal's minimum width makes deep valid trees livelock GTK allocation.
            paned.set_shrink_start_child(true);
            paned.set_shrink_end_child(true);
            // Wide handle makes the divider grabable (default is ~5px, hard to click).
            paned.set_wide_handle(true);

            if before {
                paned.set_start_child(Some(&new_widget));
                paned.set_end_child(Some(&old_widget));
            } else {
                paned.set_start_child(Some(&old_widget));
                paned.set_end_child(Some(&new_widget));
            }

            // Set 50/50 position after the first layout pass (per D-09 and RESEARCH Pitfall 2).
            // connect_realize fires before GTK allocates sizes, so p.width() is 0 there.
            // idle_add_local_once defers to the next main-loop idle, after layout completes.
            {
                let paned_ref = paned.clone();
                gtk4::glib::idle_add_local_once(move || {
                    let size = if orientation_cap == gtk4::Orientation::Horizontal {
                        paned_ref.width()
                    } else {
                        paned_ref.height()
                    };
                    if size > 0 {
                        paned_ref.set_position(size / 2);
                    }
                });
            }

            recovery::install(&paned);
            install_divider_bounds(&paned);

            let (start, end) = if before {
                (Box::new(new_leaf), Box::new(old_leaf))
            } else {
                (Box::new(old_leaf), Box::new(new_leaf))
            };
            SplitNode::Split {
                orientation: orientation_cap,
                paned: paned.clone(),
                start,
                end,
            }
        });
        replace_in_tree(&mut self.root, target_pane_id, &mut replacer)
    }

    /// The pane owning `uuid` and the tab's zero-based index inside that pane's strip.
    ///
    /// Used to anchor relative placement (`--before` / `--after`); returns `None` when no
    /// pane in this workspace holds the surface.
    pub fn surface_location(&self, uuid: &str) -> Option<(u64, usize)> {
        let pane_id = self.find_pane_id_by_uuid(uuid)?;
        let (_, surfaces) = find_pane_tabs(&self.root, pane_id)?;
        let index = surfaces
            .borrow()
            .iter()
            .position(|surface| surface.uuid().to_string() == uuid)?;
        Some((pane_id, index))
    }

    /// Reorder one tab inside its current pane. GTK emits `page-reordered`, which updates
    /// the model before this method returns.
    pub fn reorder_surface(
        &mut self,
        uuid: Uuid,
        position: usize,
    ) -> Result<SurfaceMoveResult, &'static str> {
        let pane_id = self
            .find_pane_id_by_uuid(&uuid.to_string())
            .ok_or("surface not found")?;
        let (notebook, surfaces) = find_pane_tabs(&self.root, pane_id).ok_or("pane not found")?;
        let count = surfaces.borrow().len();
        if position >= count {
            return Err("surface position out of range");
        }
        let widget = surfaces
            .borrow()
            .iter()
            .find(|surface| surface.uuid() == uuid)
            .map(PaneSurface::widget)
            .ok_or("surface not found")?;
        notebook.reorder_child(&widget, Some(position as u32));
        Ok(SurfaceMoveResult { pane_id, position })
    }

    /// Move a live tab into another pane in this workspace. The widget and its native
    /// process keep the same UUID and ownership; an emptied source pane is collapsed.
    pub fn move_surface(
        &mut self,
        uuid: Uuid,
        destination_pane: u64,
        position: Option<usize>,
        focus: bool,
    ) -> Result<SurfaceMoveResult, &'static str> {
        let source_pane = self
            .find_pane_id_by_uuid(&uuid.to_string())
            .ok_or("surface not found")?;
        let (_, destination_surfaces) =
            find_pane_tabs(&self.root, destination_pane).ok_or("destination pane not found")?;
        let destination_count = destination_surfaces.borrow().len();
        if source_pane == destination_pane {
            let count = destination_count;
            if count == 0 {
                return Err("destination pane is empty");
            }
            let Some(position) = position else {
                let current = destination_surfaces
                    .borrow()
                    .iter()
                    .position(|surface| surface.uuid() == uuid)
                    .ok_or("surface not found")?;
                if focus {
                    self.focus_surface(&uuid.to_string());
                }
                return Ok(SurfaceMoveResult {
                    pane_id: destination_pane,
                    position: current,
                });
            };
            let result = self.reorder_surface(uuid, position)?;
            if focus {
                self.focus_surface(&uuid.to_string());
            }
            return Ok(result);
        }
        let position = position.unwrap_or(destination_count);
        if position > destination_count {
            return Err("surface position out of range");
        }

        let (source_notebook, source_surfaces) =
            find_pane_tabs(&self.root, source_pane).ok_or("source pane not found")?;
        let source_index = source_surfaces
            .borrow()
            .iter()
            .position(|surface| surface.uuid() == uuid)
            .ok_or("surface not found")?;
        let surface = source_surfaces.borrow_mut().remove(source_index);
        let widget = surface.widget();
        let page = source_notebook
            .page_num(&widget)
            .ok_or("surface widget missing")?;
        source_notebook.remove_page(Some(page));
        let source_empty = source_surfaces.borrow().is_empty();

        if source_empty {
            let sibling = remove_leaf_from_tree(&mut self.root, source_pane)
                .ok_or("cannot empty the only pane")?;
            if self.active_pane_id == source_pane {
                self.active_pane_id = sibling;
            }
        }

        let (destination_notebook, destination_surfaces) =
            find_pane_tabs(&self.root, destination_pane).ok_or("destination pane not found")?;
        append_pane_surface(&destination_notebook, &destination_surfaces, surface, focus);
        if position < destination_count {
            destination_notebook.reorder_child(&widget, Some(position as u32));
        }
        if focus {
            self.active_pane_id = destination_pane;
        }
        self.root.update_focus_css(self.active_pane_id);
        if focus {
            self.focus_active_surface();
        }
        crate::diagnostics::record(
            "surface.moved",
            serde_json::json!({
                "surface_id": uuid,
                "source_pane": source_pane,
                "destination_pane": destination_pane,
                "position": position,
                "focused": focus,
            }),
        );
        Ok(SurfaceMoveResult {
            pane_id: destination_pane,
            position,
        })
    }

    /// Validate a cross-workspace destination before detaching the source widget.
    pub(crate) fn can_insert_surface(&self, pane_id: u64, position: Option<usize>) -> bool {
        find_pane_tabs(&self.root, pane_id).is_some_and(|(_, surfaces)| {
            position.is_none_or(|position| position <= surfaces.borrow().len())
        })
    }

    pub(crate) fn contains_pane(&self, pane_id: u64) -> bool {
        self.root.find_node(pane_id).is_some()
    }

    /// Whether this engine currently owns no surface. This transient state is allowed only
    /// while AppState removes an emptied source workspace in the same GTK mutation.
    pub(crate) fn is_empty(&self) -> bool {
        self.all_panes().is_empty()
    }

    /// Detach one stable surface without destroying its terminal or browser owner.
    pub(crate) fn detach_surface(&mut self, uuid: Uuid) -> Result<DetachedSurface, &'static str> {
        let source_pane = self
            .find_pane_id_by_uuid(&uuid.to_string())
            .ok_or("surface not found")?;
        let (notebook, surfaces) =
            find_pane_tabs(&self.root, source_pane).ok_or("source pane not found")?;
        let position = surfaces
            .borrow()
            .iter()
            .position(|surface| surface.uuid() == uuid)
            .ok_or("surface not found")?;
        let surface = surfaces.borrow_mut().remove(position);
        let page = notebook
            .page_num(&surface.widget())
            .ok_or("surface widget missing")?;
        notebook.remove_page(Some(page));
        if surfaces.borrow().is_empty() && !matches!(self.root, SplitNode::Leaf { .. }) {
            let sibling = remove_leaf_from_tree(&mut self.root, source_pane)
                .ok_or("source pane collapse failed")?;
            if self.active_pane_id == source_pane {
                self.active_pane_id = sibling;
            }
        }
        self.root.update_focus_css(self.active_pane_id);
        Ok(DetachedSurface {
            surface,
            source_pane,
            position,
        })
    }

    /// Attach a previously validated detached surface to this workspace.
    pub(crate) fn insert_detached_surface(
        &mut self,
        detached: DetachedSurface,
        pane_id: u64,
        position: Option<usize>,
        focus: bool,
    ) -> SurfaceMoveResult {
        let (notebook, surfaces) = find_pane_tabs(&self.root, pane_id)
            .expect("cross-workspace destination validated before detach");
        let count = surfaces.borrow().len();
        let position = position.unwrap_or(count).min(count);
        let widget = detached.surface.widget();
        append_pane_surface(&notebook, &surfaces, detached.surface, focus);
        if position < count {
            notebook.reorder_child(&widget, Some(position as u32));
        }
        if focus {
            self.active_pane_id = pane_id;
        }
        self.root.update_focus_css(self.active_pane_id);
        if focus {
            self.focus_active_surface();
        }
        SurfaceMoveResult { pane_id, position }
    }

    /// Turn an existing tab into a new pane next to a target pane without creating a
    /// placeholder terminal. This is the programmatic boundary used by tab drag-to-split.
    pub fn drag_surface_to_split(
        &mut self,
        uuid: Uuid,
        target_pane: u64,
        direction: FocusDirection,
    ) -> Result<u64, &'static str> {
        let source_pane = self
            .find_pane_id_by_uuid(&uuid.to_string())
            .ok_or("surface not found")?;
        if self.root.find_node(target_pane).is_none() {
            return Err("target pane not found");
        }
        let (source_notebook, source_surfaces) =
            find_pane_tabs(&self.root, source_pane).ok_or("source pane not found")?;
        if source_pane == target_pane && source_surfaces.borrow().len() == 1 {
            return Err("cannot split a pane's only surface by moving it");
        }
        let source_index = source_surfaces
            .borrow()
            .iter()
            .position(|surface| surface.uuid() == uuid)
            .ok_or("surface not found")?;
        let surface = source_surfaces.borrow_mut().remove(source_index);
        let widget = surface.widget();
        let page = source_notebook
            .page_num(&widget)
            .ok_or("surface widget missing")?;
        source_notebook.remove_page(Some(page));
        if source_surfaces.borrow().is_empty() {
            let sibling = remove_leaf_from_tree(&mut self.root, source_pane)
                .ok_or("cannot empty the only pane")?;
            if self.active_pane_id == source_pane {
                self.active_pane_id = sibling;
            }
        }

        let orientation = match direction {
            FocusDirection::Left | FocusDirection::Right => gtk4::Orientation::Horizontal,
            FocusDirection::Up | FocusDirection::Down => gtk4::Orientation::Vertical,
        };
        let before = matches!(direction, FocusDirection::Left | FocusDirection::Up);
        let stack_slot = if matches!(self.root, SplitNode::Leaf { .. }) {
            self.root
                .widget()
                .parent()
                .and_then(|parent| parent.downcast::<gtk4::Stack>().ok())
                .and_then(|stack| {
                    let name = stack.page(&self.root.widget()).name()?.to_string();
                    Some((stack, name))
                })
        } else {
            None
        };
        let new_pane_id = self.next_pane_id;
        self.next_pane_id += 1;
        let new_leaf = create_pane(new_pane_id, surface);
        self.replace_leaf_with_split_position(target_pane, new_leaf, orientation, before)
            .ok_or("target pane disappeared")?;
        if let Some((stack, name)) = stack_slot {
            let root = self.root.widget();
            stack.add_named(&root, Some(&name));
            stack.set_visible_child_name(&name);
        }
        self.active_pane_id = new_pane_id;
        self.root.update_focus_css(new_pane_id);
        self.focus_active_surface();
        crate::diagnostics::record(
            "surface.drag_to_split",
            serde_json::json!({
                "surface_id": uuid,
                "source_pane": source_pane,
                "target_pane": target_pane,
                "new_pane": new_pane_id,
                "direction": format!("{direction:?}").to_ascii_lowercase(),
            }),
        );
        Ok(new_pane_id)
    }

    /// Close the active pane (Ctrl+Shift+X per UI-SPEC).
    /// Removes the active leaf, replaces its parent Split with the surviving sibling.
    /// Returns the new active pane_id, or None if this was the last pane.
    pub fn close_active(&mut self) -> Option<u64> {
        self.close_pane(self.active_pane_id)
    }

    /// Remove an explicit pane and its PTYs, preserving a surviving active pane.
    /// Return the resulting focused pane, or None for a missing/final pane without mutation.
    fn close_pane(&mut self, pane_id: u64) -> Option<u64> {
        self.root.find_node(pane_id)?;
        if matches!(&self.root, SplitNode::Leaf { .. }) {
            return None;
        }
        let terminal_areas = find_pane_tabs(&self.root, pane_id)
            .map(|(_, surfaces)| {
                surfaces
                    .borrow()
                    .iter()
                    .filter_map(PaneSurface::terminal_area)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // End PTYs and renderers before removing their valid GL contexts.
        for area in &terminal_areas {
            destroy_terminal_area(area);
        }
        let sibling_id = remove_leaf_from_tree(&mut self.root, pane_id)?;
        if self.root.find_node(self.active_pane_id).is_none() {
            self.active_pane_id = sibling_id;
        }
        self.root.update_focus_css(self.active_pane_id);
        self.focus_active_surface();
        Some(self.active_pane_id)
    }

    /// Close a tab and its pane when empty, leaving final-workspace policy to the caller.
    /// LastSurfaceInPane means only the workspace's final pane remains; no workspace is removed here.
    pub fn close_surface_and_empty_pane(&mut self, uuid: Uuid) -> CloseSurfaceResult {
        match self.close_surface_tab(uuid) {
            CloseSurfaceResult::LastSurfaceInPane => {
                let pane_id = self.find_pane_id_by_uuid(&uuid.to_string());
                if pane_id
                    .and_then(|pane_id| self.close_pane(pane_id))
                    .is_some()
                {
                    CloseSurfaceResult::Closed
                } else {
                    CloseSurfaceResult::LastSurfaceInPane
                }
            }
            result => result,
        }
    }

    /// Close one sibling surface tab without removing its containing pane.
    /// Closing the final surface delegates to the existing pane-close flow.
    pub fn close_surface_tab(&mut self, uuid: Uuid) -> CloseSurfaceResult {
        let Some(pane_id) = self.find_pane_id_by_uuid(&uuid.to_string()) else {
            crate::diagnostics::event(format_args!("surface-tab close not found uuid={uuid}"));
            return CloseSurfaceResult::NotFound;
        };
        let Some((notebook, surfaces)) = find_pane_tabs(&self.root, pane_id) else {
            crate::diagnostics::event(format_args!(
                "surface-tab close missing pane tabs uuid={uuid} pane={pane_id}"
            ));
            return CloseSurfaceResult::NotFound;
        };
        let index = surfaces
            .borrow()
            .iter()
            .position(|surface| surface.uuid() == uuid);
        let Some(index) = index else {
            crate::diagnostics::event(format_args!(
                "surface-tab close missing surface uuid={uuid} pane={pane_id}"
            ));
            return CloseSurfaceResult::NotFound;
        };
        if surfaces.borrow().len() == 1 {
            crate::diagnostics::event(format_args!(
                "surface-tab close delegates to pane uuid={uuid} pane={pane_id}"
            ));
            return CloseSurfaceResult::LastSurfaceInPane;
        }

        let surface = surfaces.borrow()[index].clone();
        crate::diagnostics::event(format_args!(
            "surface-tab closing uuid={uuid} pane={pane_id} kind={} index={index}",
            surface.tab_title().to_ascii_lowercase(),
        ));
        let widget = surface.widget();
        if let Some(area) = surface.terminal_area() {
            destroy_terminal_area(&area);
        }
        let page = notebook.page_num(&widget);
        let selected = notebook.current_page();
        // Update the model before remove_page emits synchronous selection callbacks.
        surfaces.borrow_mut().remove(index);
        if let Some(page) = page {
            let next = selected.and_then(|selected| {
                crate::selection::after_removal(
                    selected as usize,
                    page as usize,
                    notebook.n_pages() as usize - 1,
                )
            });
            notebook.remove_page(Some(page));
            if let Some(next) = next {
                notebook.set_current_page(Some(next as u32));
            }
        }
        self.root.update_focus_css(self.active_pane_id);
        self.focus_active_surface();
        crate::diagnostics::event(format_args!(
            "surface-tab closed uuid={uuid} pane={pane_id}"
        ));
        CloseSurfaceResult::Closed
    }

    /// Navigate focus to the pane adjacent in `direction` (Ctrl+Alt+arrows per D-10).
    pub fn focus_next_in_direction(&mut self, direction: FocusDirection) -> bool {
        let active_id = self.active_pane_id;
        if let Some(new_id) = find_adjacent(&self.root, active_id, direction) {
            // Unfocus old surface.
            if let Some(old_surface) = self.find_surface(active_id) {
                unsafe {
                    ffi::ghostty_surface_set_focus(old_surface, false);
                }
            }
            self.active_pane_id = new_id;
            self.root.update_focus_css(new_id);
            // Focus new surface or Preview URL entry.
            if let Some(new_surface) = self.find_surface(new_id) {
                unsafe {
                    ffi::ghostty_surface_set_focus(new_surface, true);
                }
            }
            if let Some(gl_area) = self.find_gl_area(new_id) {
                gl_area.grab_focus();
            } else if let Some(entry) = find_url_entry_in_tree(&self.root, new_id) {
                entry.grab_focus();
            }
            true
        } else {
            false
        }
    }

    /// Resolve only the selected terminal; a selected browser never falls back to a hidden terminal.
    fn find_surface(&self, pane_id: u64) -> Option<ffi::ghostty_surface_t> {
        self.root.find_surface_for_pane(pane_id)
    }

    /// Resolve the pane's selected terminal widget for focus and rendering operations.
    fn find_gl_area(&self, pane_id: u64) -> Option<gtk4::GLArea> {
        find_gl_area_in_tree(&self.root, pane_id)
    }

    /// Returns all leaf panes in this engine as (uuid, pane_id, active) tuples.
    pub fn all_panes(&self) -> Vec<(Uuid, u64, bool)> {
        let mut panes = Vec::new();
        self.root.collect_pane_info(&mut panes, self.active_pane_id);
        panes
    }

    /// Snapshot split panes in traversal order, keeping sibling tabs grouped under their owner.
    pub fn pane_info(&self) -> Vec<PaneInfo> {
        let mut panes = Vec::new();
        collect_pane_snapshots(&self.root, &mut panes);
        panes
    }

    /// Return pane and tab-label bounds relative to one top-level GTK widget.
    pub fn pane_geometry(&self, ancestor: &gtk4::Widget) -> Vec<PaneGeometry> {
        let mut panes = Vec::new();
        collect_pane_geometry(&self.root, ancestor, &mut panes);
        panes
    }

    /// Read or replace a terminal's resume metadata without selection or native process changes.
    /// A checkpoint mismatch leaves the binding intact, preventing stale hook cleanup.
    pub fn resume_action(
        &self,
        surface_id: &str,
        action: &crate::resume::ResumeAction,
    ) -> Result<Option<crate::resume::ResumeBinding>, &'static str> {
        let pane_id = self
            .find_pane_id_by_uuid(surface_id)
            .ok_or("surface not found")?;
        let (notebook, surfaces) =
            find_pane_tabs(&self.root, pane_id).ok_or("pane not found")?;
        let mut surfaces = surfaces.borrow_mut();
        let surface = surfaces
            .iter_mut()
            .find(|surface| surface.uuid().to_string() == surface_id)
            .ok_or("surface not found")?;
        let tab = notebook.tab_label(&surface.widget());
        let PaneSurface::Terminal { resume, .. } = surface else {
            return Err("resume bindings require a terminal");
        };
        match action {
            crate::resume::ResumeAction::Set(binding) => {
                binding.validate()?;
                *resume = Some(binding.clone());
            }
            crate::resume::ResumeAction::Show => {}
            crate::resume::ResumeAction::Clear { checkpoint_id } => {
                if let Some(expected) = checkpoint_id {
                    if resume
                        .as_ref()
                        .and_then(|binding| binding.checkpoint_id.as_ref())
                        != Some(expected)
                    {
                        return Err("checkpoint mismatch");
                    }
                }
                *resume = None;
            }
        }
        if let Some(tab) = tab {
            set_tab_agent_icon(&tab, resume.as_ref());
        }
        Ok(resume.clone())
    }

    /// Focus a session-local pane reference or a legacy surface UUID without switching its tab.
    pub fn focus_pane_ref(&mut self, reference: &str) -> bool {
        let Some(pane_id) = self.pane_for_ref(reference) else {
            return false;
        };
        if !self.activate_pane(pane_id) {
            return false;
        }
        self.grab_active_focus();
        true
    }

    /// Clone a terminal widget by stable tab identity without changing notebook selection or focus.
    pub fn gl_area_for_surface(&self, uuid: &str) -> Option<gtk4::GLArea> {
        self.root.find_terminal_by_uuid(uuid)
    }

    /// Show a terminal's program-set title on its tab; false when the tab is not here.
    pub fn set_surface_title(&self, uuid: &str, title: &str) -> bool {
        let label = self
            .gl_area_for_surface(uuid)
            .and_then(|area| tab_part(&area, TAB_TITLE))
            .and_then(|widget| widget.downcast::<gtk4::Label>().ok());
        label.is_some_and(|label| {
            // Notebook tabs shrink an ellipsizing label to "…", so bound the text instead.
            let mut text: String = title.trim().chars().take(32).collect();
            if title.trim().chars().count() > 32 {
                text.push('…');
            }
            label.set_text(if text.is_empty() { "Terminal" } else { &text });
            true
        })
    }

    /// The title currently shown on a terminal's tab.
    pub fn surface_title(&self, uuid: &str) -> Option<String> {
        self.gl_area_for_surface(uuid)
            .and_then(|area| tab_part(&area, TAB_TITLE))
            .and_then(|widget| widget.downcast::<gtk4::Label>().ok())
            .map(|label| label.text().to_string())
    }

    /// Show the unread dot on exactly the terminal tabs listed.
    pub fn set_unread_tabs(&self, unread: &std::collections::HashSet<String>) {
        let mut areas = Vec::new();
        self.root.collect_terminal_areas(&mut areas);
        for area in areas {
            // SAFETY: this private key always stores a UUID (see append_pane_surface).
            let Some(uuid) = (unsafe { area.data::<Uuid>("cmux-surface-uuid") })
                .map(|uuid| unsafe { *uuid.as_ref() })
            else {
                continue;
            };
            if let Some(dot) = tab_part(&area, TAB_UNREAD_DOT) {
                dot.set_visible(unread.contains(&uuid.to_string()));
            }
        }
    }

    /// Look up a surface by its UUID string. Returns the ghostty surface handle if found.
    pub fn find_surface_by_uuid(&self, target_uuid: &str) -> Option<ffi::ghostty_surface_t> {
        self.gl_area_for_surface(target_uuid)
            .and_then(|area| surface_for_area(&area))
    }

    /// Look up a pane_id by its UUID string.
    pub fn find_pane_id_by_uuid(&self, target_uuid: &str) -> Option<u64> {
        self.root.find_pane_id_by_uuid(target_uuid)
    }

    /// Look up a GLArea by pane_id (public wrapper for socket handlers).
    pub fn gl_area_for_pane(&self, pane_id: u64) -> Option<gtk4::GLArea> {
        find_gl_area_in_tree(&self.root, pane_id)
    }

    /// Browser tabs reconstructed from a saved session and awaiting signal wiring.
    pub fn browser_tabs(&self) -> Vec<crate::browser::PreviewPaneWidgets> {
        let mut tabs = Vec::new();
        collect_browser_tabs(&self.root, &mut tabs);
        tabs
    }
}

/// Collect browser widgets with their stable surface IDs across the pane tree.
fn collect_browser_tabs(node: &SplitNode, out: &mut Vec<crate::browser::PreviewPaneWidgets>) {
    match node {
        SplitNode::Leaf { surfaces, .. } => {
            out.extend(surfaces.borrow().iter().filter_map(|surface| {
                if let PaneSurface::Browser { widgets, .. } = surface {
                    Some(widgets.clone())
                } else {
                    None
                }
            }));
        }
        SplitNode::Split { start, end, .. } => {
            collect_browser_tabs(start, out);
            collect_browser_tabs(end, out);
        }
    }
}

// ── Tree traversal helpers ───────────────────────────────────────────────────

/// Replace the leaf with `target_id` using `replacer` function. Returns Some(()) if found.
fn replace_in_tree<F>(node: &mut SplitNode, target_id: u64, replacer: &mut Option<F>) -> Option<()>
where
    F: FnOnce(SplitNode) -> SplitNode,
{
    match node {
        SplitNode::Leaf { pane_id, .. } if *pane_id == target_id => {
            if let Some(r) = replacer.take() {
                // Take ownership of the old node to pass to replacer.
                let old = std::mem::replace(
                    node,
                    create_pane(
                        0,
                        PaneSurface::Terminal {
                            gl_area: gtk4::GLArea::new(),
                            uuid: Uuid::new_v4(),
                            resume: None,
                        },
                    ),
                );
                *node = r(old);
                Some(())
            } else {
                None
            }
        }
        SplitNode::Leaf { .. } => None,
        SplitNode::Split {
            start, end, paned, ..
        } => {
            if let Some(()) = replace_in_tree(start, target_id, replacer) {
                // Update paned start child to new widget.
                paned.set_start_child(Some(&start.widget()));
                Some(())
            } else if let Some(()) = replace_in_tree(end, target_id, replacer) {
                paned.set_end_child(Some(&end.widget()));
                Some(())
            } else {
                None
            }
        }
    }
}

/// Remove leaf `target_id` from the tree. Returns the surviving sibling's pane_id.
/// Replaces the parent Split with the surviving sibling in the GTK widget tree.
fn remove_leaf_from_tree(node: &mut SplitNode, target_id: u64) -> Option<u64> {
    match node {
        SplitNode::Leaf { .. } => None, // Caller ensures we never remove the root leaf
        SplitNode::Split {
            start, end, paned, ..
        } => {
            // Check if start is the target leaf.
            let start_is_target = match start.as_ref() {
                SplitNode::Leaf { pane_id, .. } => *pane_id == target_id,
                _ => false,
            };
            if start_is_target {
                // Surviving sibling is end. Replace this Split with end in the GTK tree.
                let surviving = *end.clone();
                let surviving_widget = surviving.widget();
                // Detach from the split being removed before inserting into
                // its parent; GTK rejects widgets that already have a parent.
                remove_widget_from_parent(&surviving_widget);
                // Find the paned's parent and replace it with the surviving widget.
                if let Some(parent) = paned.parent() {
                    replace_child_in_parent(&parent, &paned.clone().upcast(), &surviving_widget);
                }
                let surviving_id = first_pane_id(&surviving);
                *node = surviving;
                return Some(surviving_id);
            }
            // Check if end is the target leaf.
            let end_is_target = match end.as_ref() {
                SplitNode::Leaf { pane_id, .. } => *pane_id == target_id,
                _ => false,
            };
            if end_is_target {
                let surviving = *start.clone();
                let surviving_widget = surviving.widget();
                remove_widget_from_parent(&surviving_widget);
                if let Some(parent) = paned.parent() {
                    replace_child_in_parent(&parent, &paned.clone().upcast(), &surviving_widget);
                }
                let surviving_id = first_pane_id(&surviving);
                *node = surviving;
                return Some(surviving_id);
            }
            // Recurse into start subtree.
            if let Some(id) = remove_leaf_from_tree(start, target_id) {
                paned.set_start_child(Some(&start.widget()));
                return Some(id);
            }
            // Recurse into end subtree.
            if let Some(id) = remove_leaf_from_tree(end, target_id) {
                paned.set_end_child(Some(&end.widget()));
                return Some(id);
            }
            None
        }
    }
}

/// Replace `old_widget` with `new_widget` in `parent`. Handles GtkPaned children and GtkStack pages.
fn replace_child_in_parent(
    parent: &gtk4::Widget,
    old_widget: &gtk4::Widget,
    new_widget: &gtk4::Widget,
) {
    if let Some(paned) = parent.downcast_ref::<gtk4::Paned>() {
        if paned
            .start_child()
            .as_ref()
            .map(|w| w == old_widget)
            .unwrap_or(false)
        {
            paned.set_start_child(Some(new_widget));
        } else {
            paned.set_end_child(Some(new_widget));
        }
    } else if let Some(stack) = parent.downcast_ref::<gtk4::Stack>() {
        let page = stack.page(old_widget);
        if let Some(name) = page.name() {
            let name_str = name.to_string();
            stack.remove(old_widget);
            // new_widget may still be parented to the Paned we're replacing; unparent first.
            remove_widget_from_parent(new_widget);
            stack.add_named(new_widget, Some(&name_str));
            stack.set_visible_child_name(&name_str);
        } else {
            stack.remove(old_widget);
        }
    }
    // If parent is something else, the widget swap is a no-op (should not happen in Phase 2).
}

/// Return the first (leftmost/topmost) pane_id in a subtree.
fn first_pane_id(node: &SplitNode) -> u64 {
    match node {
        SplitNode::Leaf { pane_id, .. } => *pane_id,
        SplitNode::Split { start, .. } => first_pane_id(start),
    }
}

/// Find a pane once and clone its notebook/model handles for selected-tab operations.
fn find_pane_tabs(
    node: &SplitNode,
    pane_id: u64,
) -> Option<(
    gtk4::Notebook,
    std::rc::Rc<std::cell::RefCell<Vec<PaneSurface>>>,
)> {
    match node {
        SplitNode::Leaf {
            pane_id: id,
            notebook,
            surfaces,
            ..
        } if *id == pane_id => Some((notebook.clone(), surfaces.clone())),
        SplitNode::Leaf { .. } => None,
        SplitNode::Split { start, end, .. } => {
            find_pane_tabs(start, pane_id).or_else(|| find_pane_tabs(end, pane_id))
        }
    }
}

/// Find a realized terminal for inheritance even when the pane currently shows a browser tab.
fn find_any_terminal_surface(node: &SplitNode, pane_id: u64) -> Option<ffi::ghostty_surface_t> {
    let (_, surfaces) = find_pane_tabs(node, pane_id)?;
    let found = surfaces
        .borrow()
        .iter()
        .filter_map(PaneSurface::terminal_area)
        .find_map(|area| surface_for_area(&area));
    found
}

/// Return the selected terminal widget in a pane located through the shared tree lookup.
fn find_gl_area_in_tree(node: &SplitNode, pane_id: u64) -> Option<gtk4::GLArea> {
    let (notebook, surfaces) = find_pane_tabs(node, pane_id)?;
    let page = notebook.current_page()?;
    let area = surfaces
        .borrow()
        .get(page as usize)
        .and_then(PaneSurface::terminal_area);
    area
}

/// Return the selected browser's address entry, excluding terminal tabs.
fn find_url_entry_in_tree(node: &SplitNode, pane_id: u64) -> Option<gtk4::Entry> {
    let (notebook, surfaces) = find_pane_tabs(node, pane_id)?;
    let page = notebook.current_page()?;
    let entry = surfaces
        .borrow()
        .get(page as usize)
        .and_then(PaneSurface::url_entry);
    entry
}

/// Find the pane adjacent to `active_id` in `direction`.
/// Strategy: collect ordered leaf positions and find the neighbor.
/// This is a directional approximation: Left/Up = previous leaf, Right/Down = next leaf.
/// A full spatial algorithm (comparing widget coordinates) can be added in a future phase.
fn find_adjacent(root: &SplitNode, active_id: u64, direction: FocusDirection) -> Option<u64> {
    let mut leaves = Vec::new();
    collect_leaves_in_order(root, &mut leaves);
    let pos = leaves.iter().position(|&id| id == active_id)?;
    match direction {
        FocusDirection::Left | FocusDirection::Up => {
            if pos > 0 {
                Some(leaves[pos - 1])
            } else {
                None
            }
        }
        FocusDirection::Right | FocusDirection::Down => {
            if pos + 1 < leaves.len() {
                Some(leaves[pos + 1])
            } else {
                None
            }
        }
    }
}

/// Pane minimum and divider range, from upstream's Bonsplit defaults (`minimumPaneWidth`/`Height`
/// 100, `dividerPositionRange` 0.1...0.9).
const MIN_PANE_SIZE: f64 = 100.0;
const DIVIDER_RANGE: (f64, f64) = (0.1, 0.9);

/// Clamp a divider position like Bonsplit's `clampedDividerPosition`: both panes keep the
/// minimum, and when the split is too small for two minimums both get half.
fn clamped_divider_position(position: i32, available: i32) -> i32 {
    if available <= 0 {
        return position;
    }
    let available = available as f64;
    let min_ratio = (MIN_PANE_SIZE.min(available / 2.0) / available).min(0.5);
    let lower = min_ratio.max(DIVIDER_RANGE.0);
    let upper = (1.0 - min_ratio).min(DIVIDER_RANGE.1);
    let (lower, upper) = if lower <= upper {
        (lower, upper)
    } else {
        (0.5, 0.5)
    };
    (position as f64)
        .clamp(available * lower, available * upper)
        .round() as i32
}

/// Keep a split divider within `clamped_divider_position`, on drag and on every resize.
/// With both children shrinkable, GTK's `max-position` is the space the two panes share.
fn install_divider_bounds(paned: &gtk4::Paned) {
    paned.connect_position_notify(|paned| {
        let position = paned.position();
        let clamped = clamped_divider_position(position, paned.max_position());
        if clamped != position {
            paned.set_position(clamped);
        }
    });
}

/// Remove `widget` from its current GTK parent so it can be reparented.
/// GTK4 requires `gtk_widget_get_parent(child) == NULL` before set_start/end_child.
fn remove_widget_from_parent(widget: &gtk4::Widget) {
    let Some(parent) = widget.parent() else {
        return;
    };
    if let Some(paned) = parent.downcast_ref::<gtk4::Paned>() {
        if paned
            .start_child()
            .as_ref()
            .map(|w| w == widget)
            .unwrap_or(false)
        {
            paned.set_start_child(None::<&gtk4::Widget>);
        } else {
            paned.set_end_child(None::<&gtk4::Widget>);
        }
    } else if let Some(stack) = parent.downcast_ref::<gtk4::Stack>() {
        stack.remove(widget);
    }
}

/// Copy pane/tab identities and notebook selection without retaining widgets or moving focus.
fn collect_pane_snapshots(node: &SplitNode, panes: &mut Vec<PaneInfo>) {
    match node {
        SplitNode::Leaf {
            pane_id,
            notebook,
            surfaces,
            ..
        } => {
            let surface_ids: Vec<Uuid> = surfaces.borrow().iter().map(PaneSurface::uuid).collect();
            let browser_urls = surfaces
                .borrow()
                .iter()
                .map(|surface| surface.url_entry().map(|entry| entry.text().to_string()))
                .collect();
            let selected_surface = notebook
                .current_page()
                .and_then(|index| surface_ids.get(index as usize))
                .copied();
            panes.push(PaneInfo {
                id: *pane_id,
                surface_ids,
                selected_surface,
                browser_urls,
            });
        }
        SplitNode::Split { start, end, .. } => {
            collect_pane_snapshots(start, panes);
            collect_pane_snapshots(end, panes);
        }
    }
}

/// Copy only realized coordinates; missing bounds remain explicit instead of fabricated.
fn collect_pane_geometry(node: &SplitNode, ancestor: &gtk4::Widget, panes: &mut Vec<PaneGeometry>) {
    match node {
        SplitNode::Leaf {
            pane_id,
            notebook,
            surfaces,
            ..
        } => {
            let bounds = notebook
                .compute_bounds(ancestor)
                .map(|bounds| [bounds.x(), bounds.y(), bounds.width(), bounds.height()]);
            let surface_bounds = surfaces
                .borrow()
                .iter()
                .filter_map(|surface| {
                    let label = notebook.tab_label(&surface.widget())?;
                    let bounds = label.compute_bounds(ancestor)?;
                    Some((
                        surface.uuid(),
                        [bounds.x(), bounds.y(), bounds.width(), bounds.height()],
                    ))
                })
                .collect();
            panes.push(PaneGeometry {
                id: *pane_id,
                bounds,
                surface_bounds,
            });
        }
        SplitNode::Split { start, end, .. } => {
            collect_pane_geometry(start, ancestor, panes);
            collect_pane_geometry(end, ancestor, panes);
        }
    }
}

/// Collect pane IDs in split traversal order for directional focus and restore fallback.
fn collect_leaves_in_order(node: &SplitNode, out: &mut Vec<u64>) {
    match node {
        SplitNode::Leaf { pane_id, .. } => out.push(*pane_id),
        SplitNode::Split { start, end, .. } => {
            collect_leaves_in_order(start, out);
            collect_leaves_in_order(end, out);
        }
    }
}

/// Serde-friendly mirror of SplitNode for session persistence.
/// GTK widget references (GLArea, Paned) cannot be serialized — this parallel type holds
/// only the data needed to reconstruct the tree on restore.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum SplitNodeData {
    Leaf {
        pane_id: u64,
        surface_uuid: Uuid,
        /// Shell executable path, e.g. "/bin/zsh" or "/bin/bash"
        shell: String,
        /// Last known terminal directory; empty until a launch path or native report is known.
        cwd: String,
    },
    Pane {
        #[serde(default)]
        active_surface_uuid: Option<Uuid>,
        #[serde(default)]
        surfaces: Vec<PaneSurfaceData>,
    },
    Split {
        /// "horizontal" or "vertical"
        orientation: String,
        /// Divider position as fraction 0.0-1.0 relative to parent size (D-03).
        #[serde(default = "default_ratio")]
        ratio: f64,
        start: Box<SplitNodeData>,
        end: Box<SplitNodeData>,
    },
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
#[allow(clippy::large_enum_variant)] // Terminal snapshots own bounded resume/env/history state.
#[serde(tag = "type")]
pub enum PaneSurfaceData {
    Terminal {
        surface_uuid: Uuid,
        shell: String,
        cwd: String,
        /// Explicit per-surface launch overrides. Older snapshots default empty.
        #[serde(default, deserialize_with = "surface_environment")]
        environment: std::collections::BTreeMap<String, String>,
        /// Construction-only input for project layouts; live snapshots never retain it.
        #[serde(skip)]
        initial_input: Option<String>,
        #[serde(default)]
        resume: Option<crate::resume::ResumeBinding>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scrollback: Option<std::sync::Arc<str>>,
    },
    Browser {
        surface_uuid: Uuid,
        #[serde(default = "default_browser_url")]
        url: String,
        /// Explicit agent-browser profile selector; absent snapshots remain ephemeral.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<String>,
    },
}

/// Apply project-action environment bounds to persisted per-surface launch overrides.
fn surface_environment<'de, D>(
    deserializer: D,
) -> Result<std::collections::BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = <serde_json::Value as serde::Deserialize>::deserialize(deserializer)?;
    crate::project_config::project_action::project_layout::environment(
        &serde_json::json!({ "env": value }),
    )
    .map_err(serde::de::Error::custom)
}

/// Supply a blank page when an older saved browser tab lacks its URL.
fn default_browser_url() -> String {
    "about:blank".to_string()
}

/// Restore equal pane sizes when a saved split omits its divider ratio.
fn default_ratio() -> f64 {
    0.5
}

impl SplitNode {
    /// Produce a serializable snapshot of this node's tree structure.
    /// Directories come from each terminal's native reports or explicit launch path.
    /// Unknown directories stay empty; the shell retains the configured environment default.
    pub fn to_data(&self) -> SplitNodeData {
        self.to_data_with_history(&mut 0)
    }

    /// Capture a session tree under one shared history byte budget; GTK owns all native handles.
    pub fn to_data_with_history(&self, budget: &mut usize) -> SplitNodeData {
        match self {
            SplitNode::Leaf {
                notebook, surfaces, ..
            } => {
                let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
                let surface_data = surfaces
                    .borrow()
                    .iter()
                    .map(|surface| match surface {
                        PaneSurface::Terminal {
                            gl_area,
                            uuid,
                            resume,
                        } => PaneSurfaceData::Terminal {
                            resume: resume.clone(),
                            environment: unsafe {
                                gl_area.data::<std::collections::BTreeMap<String, String>>(
                                    "cmux-launch-environment",
                                )
                            }
                            .map(|value| unsafe { value.as_ref().clone() })
                            .unwrap_or_default(),
                            initial_input: None,
                            scrollback: crate::scrollback::capture(
                                gl_area,
                                surface_for_area(gl_area),
                                budget,
                            ),
                            surface_uuid: *uuid,
                            shell: shell.clone(),
                            cwd: surface_for_area(gl_area)
                                .map(|pointer| {
                                    crate::ghostty::registry::working_directory(pointer as usize)
                                })
                                .unwrap_or_else(|| {
                                    // SAFETY: create_surface stores String under this private key on GTK.
                                    unsafe { gl_area.data::<String>("cmux-launch-directory") }
                                        .map(|directory| unsafe { directory.as_ref().clone() })
                                        .unwrap_or_default()
                                }),
                        },
                        PaneSurface::Browser { widgets, uuid } => PaneSurfaceData::Browser {
                            surface_uuid: *uuid,
                            url: widgets.url_entry.text().to_string(),
                            profile: widgets.profile.clone(),
                        },
                    })
                    .collect();
                let active_surface_uuid = notebook
                    .current_page()
                    .and_then(|page| surfaces.borrow().get(page as usize).map(PaneSurface::uuid));
                SplitNodeData::Pane {
                    active_surface_uuid,
                    surfaces: surface_data,
                }
            }
            SplitNode::Split {
                orientation,
                paned,
                start,
                end,
                ..
            } => {
                let total_size = if *orientation == gtk4::Orientation::Horizontal {
                    paned.width()
                } else {
                    paned.height()
                };
                let ratio = if total_size > 0 {
                    (paned.position() as f64) / (total_size as f64)
                } else {
                    0.5 // default if not yet laid out
                };
                SplitNodeData::Split {
                    orientation: match orientation {
                        gtk4::Orientation::Horizontal => "horizontal".to_string(),
                        gtk4::Orientation::Vertical => "vertical".to_string(),
                        _ => "horizontal".to_string(),
                    },
                    ratio,
                    start: Box::new(start.to_data_with_history(budget)),
                    end: Box::new(end.to_data_with_history(budget)),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify dividers keep both panes at the minimum, within 10-90 %, or split evenly when small.
    #[test]
    fn divider_positions_keep_both_panes_visible() {
        // Wide split: the 10-90 % range binds before the 100 px minimum.
        assert_eq!(clamped_divider_position(5, 2000), 200);
        assert_eq!(clamped_divider_position(1995, 2000), 1800);
        assert_eq!(clamped_divider_position(700, 2000), 700);
        // Narrow split: the 100 px minimum binds.
        assert_eq!(clamped_divider_position(3, 500), 100);
        assert_eq!(clamped_divider_position(497, 500), 400);
        // Too small for two minimums: both panes get half.
        assert_eq!(clamped_divider_position(10, 150), 75);
        // Not allocated yet: left alone.
        assert_eq!(clamped_divider_position(42, 0), 42);
    }

    /// Verify a tab dropped on the tab strip joins the pane instead of splitting its top quarter.
    #[test]
    fn pane_drop_zones_keep_the_tab_strip_out_of_the_split_quarters() {
        let (width, height, tab_bar_bottom) = (400.0, 400.0, 35.0);
        let zone = |x, y| pane_drop_direction(x, y, width, height, tab_bar_bottom);
        // Anywhere on the tabs (and just under them) adds the tab to this pane.
        assert_eq!(zone(10.0, 10.0), "center");
        assert_eq!(zone(200.0, 35.0), "center");
        // The body keeps the split quarters, top one included.
        assert_eq!(zone(200.0, 40.0), "up");
        assert_eq!(zone(10.0, 200.0), "left");
        assert_eq!(zone(390.0, 200.0), "right");
        assert_eq!(zone(200.0, 390.0), "down");
        // The middle of the body joins at the drop's x, and a missing tab strip splits nothing.
        assert_eq!(zone(200.0, 200.0), "center");
        assert_eq!(pane_drop_direction(200.0, 5.0, width, height, 0.0), "up");
    }

    /// Verify legacy leaf JSON retains the stable surface identity.
    #[test]
    fn split_node_data_leaf_has_surface_uuid() {
        // Build a minimal SplitNodeData::Leaf directly and verify surface_uuid field exists.
        let id = Uuid::new_v4();
        let data = SplitNodeData::Leaf {
            pane_id: 42,
            surface_uuid: id,
            shell: "/bin/bash".to_string(),
            cwd: "/home/user".to_string(),
        };
        if let SplitNodeData::Leaf {
            surface_uuid,
            pane_id,
            ..
        } = data
        {
            assert_eq!(surface_uuid, id);
            assert_eq!(pane_id, 42);
        } else {
            panic!("Expected SplitNodeData::Leaf");
        }
    }

    /// Preserve a legacy terminal leaf through JSON serialization.
    #[test]
    fn split_node_data_roundtrip_json() {
        // Verify SplitNodeData serializes and deserializes via serde_json.
        let leaf = SplitNodeData::Leaf {
            pane_id: 1,
            surface_uuid: Uuid::new_v4(),
            shell: "/bin/zsh".to_string(),
            cwd: "/tmp".to_string(),
        };
        let json = serde_json::to_string(&leaf).expect("serialize failed");
        let restored: SplitNodeData = serde_json::from_str(&json).expect("deserialize failed");
        if let (
            SplitNodeData::Leaf {
                pane_id: p1,
                surface_uuid: u1,
                ..
            },
            SplitNodeData::Leaf {
                pane_id: p2,
                surface_uuid: u2,
                ..
            },
        ) = (&leaf, &restored)
        {
            assert_eq!(p1, p2);
            assert_eq!(u1, u2);
        } else {
            panic!("Roundtrip changed variant");
        }
    }

    /// Preserve mixed terminal/browser tab order, URLs and selection through serialization.
    #[test]
    fn pane_tabs_roundtrip_preserves_browser_url_and_active_surface() {
        let terminal_uuid = Uuid::new_v4();
        let browser_uuid = Uuid::new_v4();
        let pane = SplitNodeData::Pane {
            active_surface_uuid: Some(browser_uuid),
            surfaces: vec![
                PaneSurfaceData::Terminal {
                    resume: None,
                    environment: std::collections::BTreeMap::from([(
                        "PROJECT_VALUE".into(),
                        "literal".into(),
                    )]),
                    initial_input: Some("must-not-repeat".into()),
                    scrollback: None,
                    surface_uuid: terminal_uuid,
                    shell: "/bin/sh".to_string(),
                    cwd: "/tmp".to_string(),
                },
                PaneSurfaceData::Browser {
                    surface_uuid: browser_uuid,
                    url: "https://example.com/path".to_string(),
                    profile: Some("Profile 2".to_string()),
                },
            ],
        };

        let json = serde_json::to_string(&pane).expect("serialize pane tabs");
        let restored: SplitNodeData = serde_json::from_str(&json).expect("restore pane tabs");
        let SplitNodeData::Pane {
            active_surface_uuid,
            surfaces,
        } = restored
        else {
            panic!("expected Pane session node");
        };
        assert_eq!(active_surface_uuid, Some(browser_uuid));
        assert!(!json.contains("must-not-repeat"));
        assert!(matches!(
            &surfaces[0],
            PaneSurfaceData::Terminal { environment, initial_input, .. }
                if environment.get("PROJECT_VALUE").map(String::as_str) == Some("literal")
                    && initial_input.is_none()
        ));
        assert!(matches!(
            &surfaces[1],
            PaneSurfaceData::Browser { surface_uuid, url, profile }
                if *surface_uuid == browser_uuid
                    && url == "https://example.com/path"
                    && profile.as_deref() == Some("Profile 2")
        ));
        let injected = json.replace(
            "\"environment\":{\"PROJECT_VALUE\":\"literal\"}",
            "\"environment\":{\"BAD=KEY\":\"literal\"},\"initial_input\":\"must-not-run\"",
        );
        assert!(serde_json::from_str::<SplitNodeData>(&injected).is_err());
    }

    /// Preserve split orientation, divider ratio and child identities in saved layouts.
    #[test]
    fn split_node_data_split_roundtrip_json() {
        // Verify nested SplitNodeData serializes correctly with ratio field.
        let split = SplitNodeData::Split {
            orientation: "horizontal".to_string(),
            ratio: 0.35,
            start: Box::new(SplitNodeData::Leaf {
                pane_id: 1,
                surface_uuid: Uuid::new_v4(),
                shell: String::new(),
                cwd: String::new(),
            }),
            end: Box::new(SplitNodeData::Leaf {
                pane_id: 2,
                surface_uuid: Uuid::new_v4(),
                shell: String::new(),
                cwd: String::new(),
            }),
        };
        let json = serde_json::to_string(&split).expect("serialize failed");
        let restored: SplitNodeData = serde_json::from_str(&json).expect("deserialize failed");
        if let SplitNodeData::Split {
            orientation, ratio, ..
        } = restored
        {
            assert_eq!(orientation, "horizontal");
            assert!(
                (ratio - 0.35).abs() < f64::EPSILON,
                "ratio not preserved in roundtrip"
            );
        } else {
            panic!("Roundtrip changed variant to non-Split");
        }

        // Verify v1-compat: Split without ratio field deserializes with default 0.5
        let v1_json = r#"{"type":"Split","orientation":"vertical","start":{"type":"Leaf","pane_id":1,"surface_uuid":"00000000-0000-0000-0000-000000000000","shell":"","cwd":""},"end":{"type":"Leaf","pane_id":2,"surface_uuid":"00000000-0000-0000-0000-000000000000","shell":"","cwd":""}}"#;
        let v1_restored: SplitNodeData =
            serde_json::from_str(v1_json).expect("v1 deserialize failed");
        if let SplitNodeData::Split { ratio, .. } = v1_restored {
            assert!(
                (ratio - 0.5).abs() < f64::EPSILON,
                "v1 missing ratio should default to 0.5"
            );
        } else {
            panic!("v1 deserialize changed variant");
        }
    }
}
