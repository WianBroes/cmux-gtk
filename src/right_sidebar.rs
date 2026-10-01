//! Right sidebar beside the workspace content: the panel upstream toggles with ⌘⌥B
//! (`RightSidebarPanelView.swift`, CLI `cmux right-sidebar …`).
//!
//! Ownership: the panel is built once in `main.rs`, stored on `AppState` (so shortcuts,
//! the menu and the socket can drive it) and lives on the GTK main thread only. Its
//! visibility and width persist in `preferences.json`. Its content stack hosts the
//! sidebar modes; the only mode on Linux so far is Files.

use gtk4::prelude::*;

/// Stack child name and CLI mode of the file explorer panel.
pub const FILES_MODE: &str = "files";

/// Handle to the right sidebar panel. Cheap to clone; every handle addresses the same
/// widgets, which are only touched on the GTK main thread.
#[derive(Clone)]
pub struct RightSidebar {
    /// The end child of the content paned: hiding it gives its width back to the workspaces.
    root: gtk4::Box,
    /// The Files tree; workspace roots are pushed into it by `AppState`.
    explorer: std::rc::Rc<crate::file_explorer::FileExplorer>,
}

impl RightSidebar {
    /// Build the sidebar beside `stack` and return the paned holding both plus the handle.
    /// The panel starts hidden unless a previous run left it open.
    pub fn build(stack: &gtk4::Stack) -> (gtk4::Paned, RightSidebar) {
        let explorer = crate::file_explorer::FileExplorer::new();
        let content = gtk4::Stack::new();
        content.set_transition_type(gtk4::StackTransitionType::None);
        content.set_hexpand(true);
        content.set_vexpand(true);
        content.add_named(explorer.widget(), Some(FILES_MODE));
        content.set_visible_child_name(FILES_MODE);

        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        root.add_css_class("sidebar");
        root.append(&content);
        root.set_visible(crate::preferences::right_sidebar_visible());

        let paned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
        paned.set_wide_handle(true);
        paned.set_start_child(Some(stack));
        paned.set_end_child(Some(&root));
        crate::preferences::attach_right_sidebar_resize(&paned);
        (
            paned,
            RightSidebar {
                root,
                explorer: std::rc::Rc::new(explorer),
            },
        )
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

    /// Point the Files tree at the resolved root of the focused workspace.
    pub fn show_root(&self, workspace: Option<u64>, root: crate::file_explorer::Root) {
        self.explorer.apply_root(workspace, root);
    }

    /// Give the Files tree a way to type into the focused terminal.
    pub fn set_insert_handler(&self, handler: std::rc::Rc<dyn Fn(&str)>) {
        self.explorer.set_insert_handler(handler);
    }

    /// Move widget focus into the Files tree (upstream ⌘⇧E entering the panel).
    pub fn focus_tree(&self) {
        self.explorer.focus_tree();
    }

    /// Whether the Files tree currently holds widget focus.
    pub fn tree_has_focus(&self) -> bool {
        self.explorer.tree_has_focus()
    }
}
