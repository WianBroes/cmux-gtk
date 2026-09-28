//! Right sidebar beside the workspace content: the panel upstream toggles with ⌘⌥B
//! (`RightSidebarPanelView.swift`, CLI `cmux right-sidebar …`).
//!
//! Ownership: the panel is built once in `main.rs`, stored on `AppState` (so shortcuts,
//! the menu and the socket can drive it) and lives on the GTK main thread only. Its
//! visibility and width persist in `preferences.json`; the Files tree arrives as content
//! in the next step.

use gtk4::prelude::*;

/// Handle to the right sidebar panel. Cheap to clone; every handle addresses the same
/// widgets, which are only touched on the GTK main thread.
#[derive(Clone)]
pub struct RightSidebar {
    /// The end child of the content paned: hiding it gives its width back to the workspaces.
    root: gtk4::Box,
}

impl RightSidebar {
    /// Build the sidebar beside `stack` and return the paned holding both plus the handle.
    /// The panel starts hidden unless a previous run left it open.
    pub fn build(stack: &gtk4::Stack) -> (gtk4::Paned, RightSidebar) {
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        root.add_css_class("sidebar");
        root.set_visible(crate::preferences::right_sidebar_visible());

        let paned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
        paned.set_wide_handle(true);
        paned.set_start_child(Some(stack));
        paned.set_end_child(Some(&root));
        crate::preferences::attach_right_sidebar_resize(&paned);
        (paned, RightSidebar { root })
    }

    /// Whether the panel is on screen.
    pub fn is_visible(&self) -> bool {
        self.root.is_visible()
    }

    /// Show or hide the panel, remembering the choice for the next launch.
    pub fn set_visible(&self, visible: bool) {
        if self.is_visible() == visible {
            return;
        }
        self.root.set_visible(visible);
        crate::preferences::set_right_sidebar_visible(visible);
    }

    /// Toggle visibility (upstream ⌘⌥B, here `Ctrl+Alt+B`).
    pub fn toggle(&self) {
        self.set_visible(!self.is_visible());
    }
}
