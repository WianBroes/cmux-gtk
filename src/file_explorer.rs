//! File explorer tree — the Files mode of the right sidebar.
//!
//! Behavior follows the macOS implementation (`FileExplorerStore.swift`,
//! `FileExplorerWorkspaceRootResolver.swift`): the tree lists the focused workspace's
//! current directory, directories first in case-insensitive name order, dot files
//! included (upstream shows hidden files by default) and children loaded on the first
//! expansion. The GTK widgets follow douglas's `file_explorer.rs`: a `TreeView` on a
//! `TreeStore` where an unloaded directory carries a dummy child row for its expander.
//!
//! Ownership: one `FileExplorer` lives inside the right sidebar for the whole session;
//! it is built, read and mutated on the GTK main thread only. Every filesystem read uses
//! `std::fs` (`DirEntry::file_type` never dereferences), so a symlink row can never pull
//! the tree outside the workspace root — symlinks are listed as links, not expanded.
//!
//! The three parts of the state live in separate cells: GTK emits `cursor-changed` (and
//! possibly `row-collapsed`) synchronously from `store.clear()` and `set_cursor()`, so a
//! single cell borrowed across a rebuild would panic in those handlers.

use gtk4::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

// Column indices of the TreeStore.
const COL_ICON: u32 = 0;
const COL_NAME: u32 = 1;
const COL_PATH: u32 = 2;
const COL_IS_DIR: u32 = 3;
const COL_HAS_DUMMY: u32 = 4;

/// Path of the dummy child row that gives an unloaded directory its expand arrow.
/// Starts with \x01 because GTK rejects strings with interior NUL bytes; the row is
/// recognized by the parent's `has_dummy` flag, never by this path.
const DUMMY_PATH: &str = "\x01__cmux_dummy__";

/// Shown instead of the tree for a remote (SSH) workspace.
const REMOTE_MESSAGE: &str = "Non disponible — workspace distant (SSH)";
/// Shown when the workspace has no current directory to list.
const NO_DIRECTORY_MESSAGE: &str = "Aucun dossier courant pour ce workspace";

/// What the tree shows for one workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Root {
    /// Local directory to list.
    Local(PathBuf),
    /// Remote (SSH) workspace: no tree, upstream resolves a remote root we do not have.
    Unavailable,
    /// No directory resolved for this workspace.
    None,
}

/// Pick the tree content for a workspace: a remote workspace is always unavailable,
/// a local one needs a current directory (upstream `FileExplorerWorkspaceRootResolver`).
pub(crate) fn resolve_root(is_remote: bool, directory: Option<PathBuf>) -> Root {
    if is_remote {
        Root::Unavailable
    } else {
        directory.map_or(Root::None, Root::Local)
    }
}

/// Trim trailing slashes, keeping the filesystem root intact.
fn trim_trailing_slashes(value: &str) -> &str {
    let mut end = value.len();
    while end > 1 && value.as_bytes()[end - 1] == b'/' {
        end -= 1;
    }
    &value[..end]
}

/// Shorten a path for the header: the home directory becomes `~` (upstream
/// `FileExplorerRootResolver.displayPath`). Non-UTF-8 paths pass through lossily.
pub(crate) fn display_path(path: &Path, home: Option<&Path>) -> String {
    let path = trim_trailing_slashes(&path.to_string_lossy()).to_string();
    let Some(home) = home else {
        return path;
    };
    let home_lossy = home.to_string_lossy();
    let home = trim_trailing_slashes(&home_lossy);
    if home.is_empty() || home == "/" {
        return path;
    }
    if path == home {
        return "~".to_string();
    }
    match path.strip_prefix(&format!("{home}/")) {
        Some(rest) => format!("~/{rest}"),
        None => path,
    }
}

/// Icon name for a row: links get their own emblem so a symlink is never mistaken
/// for a plain file or a directory.
fn icon(is_dir: bool, is_symlink: bool) -> &'static str {
    if is_symlink {
        "emblem-symbolic-link"
    } else if is_dir {
        "folder-symbolic"
    } else {
        "text-x-generic-symbolic"
    }
}

/// One directory entry as listed by `std::fs`: `is_dir` is only true for real
/// directories (`file_type` does not dereference symlinks, so links stay links).
pub(crate) struct Entry {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) is_dir: bool,
    pub(crate) is_symlink: bool,
}

/// Order entries like upstream's `sortedChildren`: directories first, then
/// case-insensitive name order.
pub(crate) fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
}

/// List one directory with `std::fs` only. Dot files are included (upstream's
/// `showHiddenFiles` defaults to true); unreadable entries are skipped, a failure
/// of the directory itself is returned for the status label.
pub(crate) fn read_entries(dir: &Path) -> std::io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else {
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        entries.push(Entry {
            name: entry.file_name().to_string_lossy().into_owned(),
            path: entry.path(),
            is_dir: file_type.is_dir(),
            is_symlink: file_type.is_symlink(),
        });
    }
    sort_entries(&mut entries);
    Ok(entries)
}

/// One row of the tree; `children` is `None` until the directory is expanded once.
struct Node {
    name: String,
    path: PathBuf,
    is_dir: bool,
    is_symlink: bool,
    children: Option<Vec<Node>>,
    expanded: bool,
}

/// Convert a directory listing into collapsed nodes (children load on demand).
fn nodes_from(dir: &Path) -> std::io::Result<Vec<Node>> {
    Ok(read_entries(dir)?
        .into_iter()
        .map(|entry| Node {
            name: entry.name,
            path: entry.path,
            is_dir: entry.is_dir,
            is_symlink: entry.is_symlink,
            children: None,
            expanded: false,
        })
        .collect())
}

/// Re-list `dir` and merge it into `old`: surviving folders keep their loaded children
/// and expansion (refreshed recursively), new entries appear collapsed, gone ones drop.
/// An unreadable directory keeps `old` untouched rather than emptying the tree.
fn refreshed(dir: &Path, old: Vec<Node>) -> Vec<Node> {
    let Ok(entries) = read_entries(dir) else {
        return old;
    };
    let mut previous: HashMap<PathBuf, Node> = old
        .into_iter()
        .map(|node| (node.path.clone(), node))
        .collect();
    entries
        .into_iter()
        .map(|entry| match previous.remove(&entry.path) {
            Some(mut node) if node.is_dir == entry.is_dir => {
                node.is_symlink = entry.is_symlink;
                if let Some(children) = node.children.take() {
                    node.children = Some(refreshed(&node.path, children));
                }
                node
            }
            _ => Node {
                name: entry.name,
                path: entry.path,
                is_dir: entry.is_dir,
                is_symlink: entry.is_symlink,
                children: None,
                expanded: false,
            },
        })
        .collect()
}

/// Flat description of every loaded row, used to tell whether a refresh changed anything.
fn listing_signature(nodes: &[Node]) -> Vec<(PathBuf, bool, bool)> {
    let mut out = Vec::new();
    for node in nodes {
        out.push((node.path.clone(), node.is_dir, node.is_symlink));
        if let Some(children) = &node.children {
            out.extend(listing_signature(children));
        }
    }
    out
}

/// Find the node for a filesystem path inside the loaded tree.
fn find_node_mut<'a>(nodes: &'a mut [Node], path: &Path) -> Option<&'a mut Node> {
    for node in nodes {
        if node.path == path {
            return Some(node);
        }
        if let Some(children) = &mut node.children {
            if let Some(found) = find_node_mut(children, path) {
                return Some(found);
            }
        }
    }
    None
}

/// Whether a node shows under the active name filter: its own name matches, or any
/// loaded child does (a directory keeps its context above a match). The filter is
/// compared case-insensitively; unloaded children cannot match what is not loaded.
fn node_matches_filter(node: &Node, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let filter = filter.to_lowercase();
    if node.name.to_lowercase().contains(&filter) {
        return true;
    }
    node.children.as_ref().is_some_and(|children| {
        children
            .iter()
            .any(|child| node_matches_filter(child, &filter))
    })
}

/// Rows in display order with the path they get in the store. Filtered-out rows are
/// skipped, so the indices count emitted rows only, and children appear under an
/// expanded directory or — while filtering — under any directory keeping a match.
fn visible_rows<'a>(nodes: &'a [Node], filter: &str) -> Vec<(String, &'a Node)> {
    fn walk<'a>(nodes: &'a [Node], filter: &str, prefix: &str, out: &mut Vec<(String, &'a Node)>) {
        let mut index = 0usize;
        for node in nodes {
            if !node_matches_filter(node, filter) {
                continue;
            }
            let path = if prefix.is_empty() {
                index.to_string()
            } else {
                format!("{prefix}:{index}")
            };
            index += 1;
            out.push((path.clone(), node));
            // Children show under an expanded directory, and always while filtering so
            // a match stays reachable inside its folder without expanding by hand.
            let show_children = filter.is_empty() && node.expanded;
            if show_children || !filter.is_empty() {
                if let Some(children) = &node.children {
                    walk(children, filter, &path, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(nodes, filter, "", &mut out);
    out
}

/// Everything the tree renders from, split into cells so signal handlers re-entering
/// during a rebuild (see the module docs) never collide on one borrow.
struct ExplorerState {
    /// The applied root; `None` until the first `apply_root`.
    root: RefCell<Option<Root>>,
    /// Root resolved from the workspace (what the tree follows by default).
    followed: RefCell<Option<Root>>,
    /// Directory the user climbed to, overriding `followed` until it changes.
    pinned: RefCell<Option<PathBuf>>,
    /// Workspace the tree was last applied for; a switch drops `pinned`.
    workspace: RefCell<Option<u64>>,
    /// Loaded nodes of the applied root.
    nodes: RefCell<Vec<Node>>,
    /// Selected row, kept across rebuilds so filtering can restore the cursor.
    selected: RefCell<Option<PathBuf>>,
    /// Active name filter (upstream `/` quick search); empty means no filter.
    filter: RefCell<String>,
}

/// Navigation above the workspace directory: the tree follows `incoming` (the
/// workspace's current directory) until the user climbs elsewhere (`pinned`); a change
/// of the followed root drops the pin, so the tree moves again with the work.
/// Returns the root to display.
fn follow_or_pinned(
    followed: &mut Option<Root>,
    pinned: &mut Option<PathBuf>,
    incoming: &Root,
) -> Root {
    if followed.as_ref() != Some(incoming) {
        *followed = Some(incoming.clone());
        *pinned = None;
    }
    pinned
        .as_ref()
        .map_or_else(|| incoming.clone(), |path| Root::Local(path.clone()))
}

/// The Files panel: root path header, lazy file tree, status label.
#[derive(Clone)]
pub struct FileExplorer {
    /// Outer widget placed in the right sidebar's content stack.
    root: gtk4::Box,
    header: gtk4::Label,
    header_row: gtk4::Box,
    up_button: gtk4::Button,
    reset_button: gtk4::Button,
    filter_entry: gtk4::SearchEntry,
    tree: gtk4::TreeView,
    scrolled: gtk4::ScrolledWindow,
    status: gtk4::Label,
    store: gtk4::TreeStore,
    state: Rc<ExplorerState>,
}

impl FileExplorer {
    /// Build the panel with an empty tree; the first `apply_root` fills it.
    pub fn new() -> Self {
        let header = gtk4::Label::new(Some(""));
        header.add_css_class("heading");
        header.set_xalign(0.0);
        header.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        header.set_margin_start(8);
        header.set_margin_end(8);
        header.set_margin_top(6);
        header.set_margin_bottom(4);
        header.set_selectable(true);

        let store = gtk4::TreeStore::new(&[
            glib::Type::STRING, // icon name
            glib::Type::STRING, // display name
            glib::Type::STRING, // full path
            glib::Type::BOOL,   // is directory
            glib::Type::BOOL,   // has dummy child
        ]);
        let tree = gtk4::TreeView::with_model(&store);
        tree.set_headers_visible(false);
        // `/` opens the filter of a later step; plain typing must not hijack it.
        tree.set_enable_search(false);
        tree.set_vexpand(true);

        let icon_renderer = gtk4::CellRendererPixbuf::new();
        let icon_column = gtk4::TreeViewColumn::new();
        icon_column.pack_start(&icon_renderer, false);
        icon_column.add_attribute(&icon_renderer, "icon-name", COL_ICON as i32);
        tree.append_column(&icon_column);

        let text_renderer = gtk4::CellRendererText::new();
        let name_column = gtk4::TreeViewColumn::new();
        name_column.pack_start(&text_renderer, true);
        name_column.add_attribute(&text_renderer, "text", COL_NAME as i32);
        name_column.set_expand(true);
        tree.append_column(&name_column);

        let scrolled = gtk4::ScrolledWindow::new();
        scrolled.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Automatic);
        scrolled.set_child(Some(&tree));
        scrolled.set_vexpand(true);

        scrolled.hide();

        let status = gtk4::Label::new(Some(NO_DIRECTORY_MESSAGE));
        status.add_css_class("dim-label");
        status.set_justify(gtk4::Justification::Center);
        status.set_wrap(true);
        status.set_margin_start(12);
        status.set_margin_end(12);
        status.set_margin_top(12);
        // The message fills the panel below the header when no tree is shown.
        status.set_vexpand(true);
        status.set_valign(gtk4::Align::Center);

        // Name filter of the `/` quick search (upstream navigateRightSidebarRows).
        let filter_entry = gtk4::SearchEntry::new();
        filter_entry.set_placeholder_text(Some("Filtrer par nom ( / )"));
        filter_entry.set_margin_start(8);
        filter_entry.set_margin_end(8);

        let state = Rc::new(ExplorerState {
            root: RefCell::new(None),
            followed: RefCell::new(None),
            pinned: RefCell::new(None),
            workspace: RefCell::new(None),
            nodes: RefCell::new(Vec::new()),
            selected: RefCell::new(None),
            filter: RefCell::new(String::new()),
        });

        let root_widget = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        root_widget.add_css_class("file-explorer");
        let up_button = gtk4::Button::from_icon_name("go-up-symbolic");
        up_button.add_css_class("flat");
        up_button.set_tooltip_text(Some("Dossier parent"));
        let reset_button = gtk4::Button::from_icon_name("go-jump-symbolic");
        reset_button.add_css_class("flat");
        reset_button.set_tooltip_text(Some("Revenir au dossier du workspace"));
        header.set_hexpand(true);
        let header_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        header_row.append(&up_button);
        header_row.append(&header);
        header_row.append(&reset_button);
        root_widget.append(&header_row);
        root_widget.append(&filter_entry);
        root_widget.append(&scrolled);
        root_widget.append(&status);

        let explorer = FileExplorer {
            root: root_widget,
            header,
            header_row,
            up_button,
            reset_button,
            filter_entry,
            tree: tree.clone(),
            scrolled,
            status,
            store: store.clone(),
            state: state.clone(),
        };
        explorer.connect_signals();
        explorer.connect_activation();
        explorer.connect_filter();
        explorer.connect_keys();
        explorer.connect_drag();
        explorer.connect_navigation();
        explorer
    }

    /// Climb to the parent folder (outside the workspace if need be) or go back to
    /// the workspace's own folder.
    fn connect_navigation(&self) {
        self.up_button.connect_clicked({
            let explorer = self.clone();
            move |_| {
                let current = explorer.state.root.borrow().clone();
                let Some(Root::Local(path)) = current else {
                    return;
                };
                if let Some(parent) = path.parent() {
                    *explorer.state.pinned.borrow_mut() = Some(parent.to_path_buf());
                    explorer.show_root(Root::Local(parent.to_path_buf()));
                    explorer.update_navigation();
                }
            }
        });
        self.reset_button.connect_clicked({
            let explorer = self.clone();
            move |_| {
                *explorer.state.pinned.borrow_mut() = None;
                let followed = explorer.state.followed.borrow().clone();
                if let Some(root) = followed {
                    explorer.show_root(root);
                }
                explorer.update_navigation();
            }
        });
    }

    fn update_navigation(&self) {
        let has_parent = matches!(
            self.state.root.borrow().as_ref(),
            Some(Root::Local(path)) if path.parent().is_some()
        );
        self.up_button.set_sensitive(has_parent);
        self.reset_button
            .set_visible(self.state.pinned.borrow().is_some());
    }

    /// Wire expansion, collapse and selection tracking.
    fn connect_signals(&self) {
        // First expansion of a directory: drop the dummy row and load its children.
        self.tree.connect_row_expanded({
            let store = self.store.clone();
            let tree = self.tree.clone();
            let state = self.state.clone();
            move |_tree, iter, path| {
                let has_dummy: bool = store
                    .get_value(iter, COL_HAS_DUMMY as i32)
                    .get()
                    .unwrap_or(false);
                if !has_dummy {
                    // An already-loaded row only flips its flag: rebuilds re-expand it
                    // from here (a rebuild already holds the nodes borrow — skip then).
                    let node_path: String = store
                        .get_value(iter, COL_PATH as i32)
                        .get()
                        .unwrap_or_default();
                    if let Ok(mut nodes) = state.nodes.try_borrow_mut() {
                        if let Some(node) = find_node_mut(&mut nodes, Path::new(&node_path)) {
                            node.expanded = true;
                        }
                    }
                    return;
                }
                while let Some(dummy) = store.iter_children(Some(iter)) {
                    store.remove(&dummy);
                }
                store.set(iter, &[(COL_HAS_DUMMY, &false)]);
                let node_path: String = store
                    .get_value(iter, COL_PATH as i32)
                    .get()
                    .unwrap_or_default();
                let selected = state.selected.borrow().clone();
                let mut nodes = state.nodes.borrow_mut();
                let Some(node) = find_node_mut(&mut nodes, Path::new(&node_path)) else {
                    return;
                };
                let children = match nodes_from(&node.path) {
                    Ok(children) => children,
                    Err(error) => {
                        crate::diagnostics::event(format_args!(
                            "file_explorer.expand_failed os_error={}",
                            error.raw_os_error().unwrap_or(-1)
                        ));
                        Vec::new()
                    }
                };
                node.children = Some(children);
                node.expanded = true;
                let parent_path = path
                    .to_str()
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                let filter = state.filter.borrow().clone();
                let mut shown = 0usize;
                if let Some(children) = &node.children {
                    for child in children.iter() {
                        if !node_matches_filter(child, &filter) {
                            continue;
                        }
                        append_node(
                            &store,
                            &tree,
                            Some(iter),
                            child,
                            &format!("{parent_path}:{shown}"),
                            selected.as_deref(),
                        );
                        shown += 1;
                    }
                }
                // Removing the dummy row can leave the view collapsed even though the
                // row was just expanded; re-assert it so the children show.
                if let Some(tree_path) = gtk4::TreePath::from_string(&parent_path) {
                    tree.expand_row(&tree_path, false);
                }
            }
        });

        // Collapse only flips the model flag; the loaded rows stay in the store.
        self.tree.connect_row_collapsed({
            let store = self.store.clone();
            let state = self.state.clone();
            move |_tree, iter, _path| {
                let node_path: String = store
                    .get_value(iter, COL_PATH as i32)
                    .get()
                    .unwrap_or_default();
                // A rebuild may emit this while it already holds the nodes borrow;
                // the model it derives from is correct either way, so skip.
                if let Ok(mut nodes) = state.nodes.try_borrow_mut() {
                    if let Some(node) = find_node_mut(&mut nodes, Path::new(&node_path)) {
                        node.expanded = false;
                    }
                }
            }
        });

        // Track the selected row so rebuilds (root change, later filtering) keep it.
        self.tree.connect_cursor_changed({
            let store = self.store.clone();
            let state = self.state.clone();
            move |tree| {
                let (path, _) = gtk4::prelude::TreeViewExt::cursor(tree);
                let selected = path
                    .as_ref()
                    .and_then(|path| store.iter(path))
                    .map(|iter| store.get_value(&iter, COL_PATH as i32))
                    .and_then(|value| value.get::<String>().ok())
                    .map(PathBuf::from);
                *state.selected.borrow_mut() = selected;
            }
        });
    }

    /// Activation (Enter or double-click), the upstream `openNode` rule: a directory
    /// toggles its expansion, a file opens with the desktop's default application.
    fn connect_activation(&self) {
        let store = self.store.clone();
        let state = self.state.clone();
        self.tree.connect_row_activated(move |tree, path, _column| {
            let Some(iter) = store.iter(path) else {
                return;
            };
            let node_path: String = store
                .get_value(&iter, COL_PATH as i32)
                .get()
                .unwrap_or_default();
            if node_path == DUMMY_PATH {
                return;
            }
            let is_dir: bool = store
                .get_value(&iter, COL_IS_DIR as i32)
                .get()
                .unwrap_or(false);
            if is_dir {
                let expanded = {
                    let mut nodes = state.nodes.borrow_mut();
                    find_node_mut(&mut nodes, Path::new(&node_path))
                        .is_some_and(|node| node.expanded)
                };
                if expanded {
                    tree.collapse_row(path);
                } else {
                    // First expansion goes through row-expanded, which loads the children.
                    tree.expand_row(path, false);
                }
                return;
            }
            match open_with_default_application(Path::new(&node_path)) {
                Ok(()) => {
                    crate::diagnostics::event(format_args!("file_explorer.open_requested"));
                }
                Err(error) => {
                    crate::diagnostics::event(format_args!(
                        "file_explorer.open_failed os_error={}",
                        error.raw_os_error().unwrap_or(-1)
                    ));
                }
            }
        });
    }

    /// `/` filtering: typing narrows the visible rows (upstream quick search), Enter
    /// returns to the tree with the filter kept, Escape clears it.
    fn connect_filter(&self) {
        let store = self.store.clone();
        let tree = self.tree.clone();
        let state = self.state.clone();
        self.filter_entry.connect_search_changed(move |entry| {
            *state.filter.borrow_mut() = entry.text().to_string();
            rebuild(&store, &tree, &state);
        });
        let tree = self.tree.clone();
        self.filter_entry.connect_stop_search(move |entry| {
            entry.set_text("");
            tree.grab_focus();
        });
        let tree = self.tree.clone();
        self.filter_entry.connect_activate(move |_| {
            tree.grab_focus();
        });
    }

    /// J/K row moves, H/L folding and `/` for the filter, scoped to the tree itself so
    /// the keys never leak into the terminal (upstream `navigateRightSidebarRows`);
    /// the arrow keys keep GTK's native movement.
    fn connect_keys(&self) {
        let store = self.store.clone();
        let tree = self.tree.clone();
        let state = self.state.clone();
        let filter_entry = self.filter_entry.clone();
        let key = gtk4::EventControllerKey::new();
        key.connect_key_pressed(move |_, keyval, _code, modifiers| {
            let chord = modifiers
                & (gtk4::gdk::ModifierType::CONTROL_MASK
                    | gtk4::gdk::ModifierType::ALT_MASK
                    | gtk4::gdk::ModifierType::SUPER_MASK);
            if chord != gtk4::gdk::ModifierType::empty() {
                return gtk4::glib::Propagation::Proceed;
            }
            match keyval.to_lower() {
                gtk4::gdk::Key::j => {
                    move_cursor(&store, &tree, &state, 1);
                    gtk4::glib::Propagation::Stop
                }
                gtk4::gdk::Key::k => {
                    move_cursor(&store, &tree, &state, -1);
                    gtk4::glib::Propagation::Stop
                }
                gtk4::gdk::Key::l => {
                    set_cursor_directory(&store, &tree, &state, true);
                    gtk4::glib::Propagation::Stop
                }
                gtk4::gdk::Key::h => {
                    set_cursor_directory(&store, &tree, &state, false);
                    gtk4::glib::Propagation::Stop
                }
                gtk4::gdk::Key::slash => {
                    filter_entry.grab_focus();
                    gtk4::glib::Propagation::Stop
                }
                _ => gtk4::glib::Propagation::Proceed,
            }
        });
        self.tree.add_controller(key);
    }

    /// Drag a row out of the tree. The terminal's existing file-drop handler receives
    /// the FileList and pastes the shell-escaped path — the same escaped, space-joined
    /// text upstream's `FileExplorerTerminalPathInsertion` inserts (see
    /// `ghostty::text::shell_escape`), so the drop side is reused unchanged.
    fn connect_drag(&self) {
        let store = self.store.clone();
        let tree = self.tree.clone();
        let drag = gtk4::DragSource::new();
        drag.set_actions(gtk4::gdk::DragAction::COPY);
        drag.connect_prepare(move |_source, x, y| {
            let (row, _column, _cell_x, _cell_y) = tree.path_at_pos(x as i32, y as i32)?;
            let row = row?;
            let iter = store.iter(&row)?;
            let node_path: String = store.get_value(&iter, COL_PATH as i32).get().ok()?;
            if node_path == DUMMY_PATH {
                return None;
            }
            let files = gtk4::gdk::FileList::from_array(&[gtk4::gio::File::for_path(&node_path)]);
            Some(gtk4::gdk::ContentProvider::for_value(&files.to_value()))
        });
        self.tree.add_controller(drag);
    }

    /// Move widget focus into the tree (upstream ⌘⇧E entering the panel).
    pub fn focus_tree(&self) {
        self.tree.grab_focus();
    }

    /// Whether the tree currently holds widget focus.
    pub fn tree_has_focus(&self) -> bool {
        self.tree.has_focus()
    }

    /// The panel widget for the sidebar's content stack.
    pub fn widget(&self) -> &gtk4::Box {
        &self.root
    }

    /// Re-read the root and every loaded folder so files created, renamed or deleted
    /// (by an agent, a build, a shell) show up live. The store is only rebuilt when the
    /// listing really changed, so expansion, selection and scroll stay put otherwise.
    fn refresh_contents(&self, path: &Path) {
        let old = std::mem::take(&mut *self.state.nodes.borrow_mut());
        let before = listing_signature(&old);
        let fresh = refreshed(path, old);
        let changed = listing_signature(&fresh) != before;
        *self.state.nodes.borrow_mut() = fresh;
        if changed {
            rebuild(&self.store, &self.tree, &self.state);
        }
    }

    /// Point the panel at a workspace root; unchanged roots keep their tree as it is.
    /// A folder the user climbed to stays until the workspace's directory changes.
    pub fn apply_root(&self, workspace: Option<u64>, root: Root) {
        if self.state.workspace.replace(workspace) != workspace {
            *self.state.pinned.borrow_mut() = None;
        }
        let shown = {
            let mut followed = self.state.followed.borrow_mut();
            let mut pinned = self.state.pinned.borrow_mut();
            follow_or_pinned(&mut followed, &mut pinned, &root)
        };
        self.show_root(shown);
        self.update_navigation();
    }

    fn show_root(&self, root: Root) {
        if self.state.root.borrow().as_ref() == Some(&root) {
            if let Root::Local(path) = &root {
                self.refresh_contents(path);
            }
            return;
        }
        crate::diagnostics::event(format_args!(
            "file_explorer.root_applied kind={}",
            match root {
                Root::Local(_) => "local",
                Root::Unavailable => "unavailable",
                Root::None => "none",
            }
        ));
        match &root {
            Root::Local(path) => {
                let home = std::env::var_os("HOME").map(PathBuf::from);
                self.header.set_text(&display_path(path, home.as_deref()));
                let loaded = nodes_from(path);
                *self.state.root.borrow_mut() = Some(root);
                *self.state.selected.borrow_mut() = None;
                match loaded {
                    Ok(nodes) => {
                        *self.state.nodes.borrow_mut() = nodes;
                        self.status.hide();
                        self.header_row.show();
                        self.scrolled.show();
                        rebuild(&self.store, &self.tree, &self.state);
                    }
                    Err(error) => {
                        self.state.nodes.borrow_mut().clear();
                        rebuild(&self.store, &self.tree, &self.state);
                        self.scrolled.hide();
                        self.status.set_text(&format!(
                            "Lecture impossible ({})",
                            error.raw_os_error().unwrap_or(-1)
                        ));
                        self.status.show();
                    }
                }
            }
            Root::Unavailable | Root::None => {
                let message = if matches!(root, Root::Unavailable) {
                    REMOTE_MESSAGE
                } else {
                    NO_DIRECTORY_MESSAGE
                };
                *self.state.root.borrow_mut() = Some(root);
                self.state.nodes.borrow_mut().clear();
                *self.state.selected.borrow_mut() = None;
                self.store.clear();
                self.scrolled.hide();
                self.header_row.hide();
                self.status.set_text(message);
                self.status.show();
            }
        }
    }
}

/// Open a file with the desktop's default application. Upstream opens its own
/// preview panel; the Linux v1 hands the path straight to `xdg-open`, never through
/// a shell. The launch is fire-and-forget, like upstream's background open.
fn open_with_default_application(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(|_| ())
}

/// Follow the focused workspace's current directory: a one-second tick resolves the
/// root and lets `apply_root` skip unchanged ones, so `cd`, workspace switches and
/// new terminals are picked up without any filesystem watching.
pub fn start_root_refresh(state: &crate::app_state::AppStateRef) {
    let state = std::rc::Rc::downgrade(state);
    gtk4::glib::MainContext::default().spawn_local(async move {
        loop {
            gtk4::glib::timeout_future(std::time::Duration::from_secs(1)).await;
            let Some(state) = state.upgrade() else {
                break;
            };
            state.borrow().refresh_right_sidebar();
        }
    });
}

/// Refill the store from the model, restoring expansion and selection by tree path.
fn rebuild(store: &gtk4::TreeStore, tree: &gtk4::TreeView, state: &ExplorerState) {
    let nodes = state.nodes.borrow();
    let selected = state.selected.borrow().clone();
    let filter = state.filter.borrow().clone();
    let rows = visible_rows(&nodes, &filter);
    store.clear();
    let mut placed: HashMap<String, gtk4::TreeIter> = HashMap::new();
    let mut opened: Vec<String> = Vec::new();
    let mut cursor_path: Option<String> = None;
    for (path, node) in &rows {
        let parent = path
            .rsplit_once(':')
            .and_then(|(parent, _)| placed.get(parent).cloned());
        let iter = store.append(parent.as_ref());
        let full_path = node.path.to_string_lossy().into_owned();
        store.set(
            &iter,
            &[
                (COL_ICON, &icon(node.is_dir, node.is_symlink)),
                (COL_NAME, &node.name.as_str()),
                (COL_PATH, &full_path.as_str()),
                (COL_IS_DIR, &node.is_dir),
                (COL_HAS_DUMMY, &false),
            ],
        );
        if node.is_dir && node.children.is_none() {
            let dummy = store.append(Some(&iter));
            store.set(
                &dummy,
                &[
                    (COL_ICON, &""),
                    (COL_NAME, &"…"),
                    (COL_PATH, &DUMMY_PATH),
                    (COL_IS_DIR, &false),
                    (COL_HAS_DUMMY, &false),
                ],
            );
            store.set(&iter, &[(COL_HAS_DUMMY, &true)]);
        }
        if let Some((parent, _)) = path.rsplit_once(':') {
            opened.push(parent.to_string());
        }
        if selected.as_deref() == Some(node.path.as_path()) {
            cursor_path = Some(path.clone());
        }
        placed.insert(path.clone(), iter);
    }
    // The view opens every folder whose children were emitted before the cursor is
    // restored: those re-entrant row-expanded callbacks find the nodes borrow held and
    // skip their flag writes, so expansion state cannot flip during a rebuild.
    opened.sort();
    opened.dedup();
    for parent in &opened {
        if let Some(tree_path) = gtk4::TreePath::from_string(parent) {
            tree.expand_row(&tree_path, false);
        }
    }
    if let Some(path) = cursor_path.as_deref().and_then(gtk4::TreePath::from_string) {
        set_cursor(tree, &path);
    }
}

/// The cursor row as (row path, filesystem path), when one is set.
fn cursor_row(store: &gtk4::TreeStore, tree: &gtk4::TreeView) -> Option<(gtk4::TreePath, String)> {
    let (path, _) = gtk4::prelude::TreeViewExt::cursor(tree);
    let path = path?;
    let iter = store.iter(&path)?;
    let node_path = store
        .get_value(&iter, COL_PATH as i32)
        .get::<String>()
        .ok()?;
    Some((path, node_path))
}

/// Put the cursor on a row without starting an edit.
fn set_cursor(tree: &gtk4::TreeView, path: &gtk4::TreePath) {
    gtk4::prelude::TreeViewExt::set_cursor(
        tree,
        path,
        Option::<&gtk4::TreeViewColumn>::None,
        false,
    );
}

/// Move the cursor one visible row down (+1) or up (-1) in display order, honouring
/// the active filter; without a cursor the move starts at the first (or last) row.
fn move_cursor(store: &gtk4::TreeStore, tree: &gtk4::TreeView, state: &ExplorerState, offset: i32) {
    let paths: Vec<String> = {
        let nodes = state.nodes.borrow();
        let filter = state.filter.borrow();
        visible_rows(&nodes, &filter)
            .into_iter()
            .map(|(path, _)| path)
            .collect()
    };
    if paths.is_empty() {
        return;
    }
    let current =
        cursor_row(store, tree).and_then(|(path, _)| path.to_str().map(|value| value.to_string()));
    let index = current.and_then(|current| paths.iter().position(|path| *path == current));
    let target = match index {
        Some(index) => (index as i32 + offset).clamp(0, paths.len() as i32 - 1) as usize,
        None if offset < 0 => paths.len() - 1,
        None => 0,
    };
    if let Some(path) = gtk4::TreePath::from_string(&paths[target]) {
        set_cursor(tree, &path);
    }
}

/// Fold (`expand` false) or unfold the directory under the cursor — the upstream
/// H/L rule; files and the dummy row ignore the keys.
fn set_cursor_directory(
    store: &gtk4::TreeStore,
    tree: &gtk4::TreeView,
    state: &ExplorerState,
    expand: bool,
) {
    let Some((row_path, node_path)) = cursor_row(store, tree) else {
        return;
    };
    if node_path == DUMMY_PATH {
        return;
    }
    let Some(iter) = store.iter(&row_path) else {
        return;
    };
    let is_dir: bool = store
        .get_value(&iter, COL_IS_DIR as i32)
        .get()
        .unwrap_or(false);
    if !is_dir {
        return;
    }
    let expanded = {
        let mut nodes = state.nodes.borrow_mut();
        find_node_mut(&mut nodes, Path::new(&node_path)).is_some_and(|node| node.expanded)
    };
    if expand && !expanded {
        tree.expand_row(&row_path, false);
    } else if !expand && expanded {
        tree.collapse_row(&row_path);
    }
}

/// Append `node` and its loaded descendants as one row; an unloaded directory gets a
/// dummy child so the expander shows. `path` is the row's tree path, used to restore
/// expansion and selection.
fn append_node(
    store: &gtk4::TreeStore,
    tree: &gtk4::TreeView,
    parent: Option<&gtk4::TreeIter>,
    node: &Node,
    path: &str,
    selected: Option<&Path>,
) {
    let full_path = node.path.to_string_lossy().into_owned();
    let iter = store.append(parent);
    store.set(
        &iter,
        &[
            (COL_ICON, &icon(node.is_dir, node.is_symlink)),
            (COL_NAME, &node.name.as_str()),
            (COL_PATH, &full_path.as_str()),
            (COL_IS_DIR, &node.is_dir),
            (COL_HAS_DUMMY, &false),
        ],
    );
    if let Some(children) = &node.children {
        for (index, child) in children.iter().enumerate() {
            append_node(
                store,
                tree,
                Some(&iter),
                child,
                &format!("{path}:{index}"),
                selected,
            );
        }
        if node.expanded {
            if let Some(tree_path) = gtk4::TreePath::from_string(path) {
                tree.expand_row(&tree_path, false);
            }
        }
    } else if node.is_dir {
        let dummy = store.append(Some(&iter));
        store.set(
            &dummy,
            &[
                (COL_ICON, &""),
                (COL_NAME, &"…"),
                (COL_PATH, &DUMMY_PATH),
                (COL_IS_DIR, &false),
                (COL_HAS_DUMMY, &false),
            ],
        );
        store.set(&iter, &[(COL_HAS_DUMMY, &true)]);
    }
    if selected == Some(node.path.as_path()) {
        if let Some(tree_path) = gtk4::TreePath::from_string(path) {
            gtk4::prelude::TreeViewExt::set_cursor(
                tree,
                &tree_path,
                Option::<&gtk4::TreeViewColumn>::None,
                false,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The root of each workspace kind matches the upstream resolver: remote is
    /// unavailable, local needs a directory.
    #[test]
    fn root_resolution_follows_the_workspace() {
        assert_eq!(resolve_root(true, Some("/tmp".into())), Root::Unavailable);
        assert_eq!(resolve_root(true, None), Root::Unavailable);
        assert_eq!(
            resolve_root(false, Some("/tmp".into())),
            Root::Local("/tmp".into())
        );
        assert_eq!(resolve_root(false, None), Root::None);
    }

    /// The header shortens the home directory and leaves other paths alone.
    #[test]
    fn climbing_sticks_until_the_workspace_directory_changes() {
        let a = Root::Local("/w/a".into());
        let b = Root::Local("/w/b".into());
        let (mut followed, mut pinned) = (None, None);
        assert_eq!(follow_or_pinned(&mut followed, &mut pinned, &a), a);
        // The user climbs above the workspace: the next ticks keep that folder.
        pinned = Some("/w".into());
        let up = Root::Local("/w".into());
        assert_eq!(follow_or_pinned(&mut followed, &mut pinned, &a), up);
        assert_eq!(follow_or_pinned(&mut followed, &mut pinned, &a), up);
        // The agent moves elsewhere: the tree follows again.
        assert_eq!(follow_or_pinned(&mut followed, &mut pinned, &b), b);
        assert_eq!(pinned, None);
    }

    #[test]
    fn display_path_shrinks_home() {
        let home = Path::new("/home/wian");
        assert_eq!(display_path(Path::new("/home/wian"), Some(home)), "~");
        assert_eq!(
            display_path(Path::new("/home/wian/projets/cmux"), Some(home)),
            "~/projets/cmux"
        );
        assert_eq!(display_path(Path::new("/home/wian/"), Some(home)), "~");
        assert_eq!(display_path(Path::new("/var/log"), Some(home)), "/var/log");
        assert_eq!(display_path(Path::new("/var/log"), None), "/var/log");
        assert_eq!(display_path(Path::new("/"), Some(home)), "/");
    }

    /// Directories lead, names compare case-insensitively after them.
    #[test]
    fn entries_sort_directories_first_then_name() {
        let mut entries = vec![
            Entry {
                name: "B.txt".into(),
                path: "/r/B.txt".into(),
                is_dir: false,
                is_symlink: false,
            },
            Entry {
                name: "a-doc".into(),
                path: "/r/a-doc".into(),
                is_dir: true,
                is_symlink: false,
            },
            Entry {
                name: "a.txt".into(),
                path: "/r/a.txt".into(),
                is_dir: false,
                is_symlink: false,
            },
            Entry {
                name: "Zoo".into(),
                path: "/r/Zoo".into(),
                is_dir: true,
                is_symlink: false,
            },
            Entry {
                name: ".hidden".into(),
                path: "/r/.hidden".into(),
                is_dir: false,
                is_symlink: false,
            },
        ];
        sort_entries(&mut entries);
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["a-doc", "Zoo", ".hidden", "a.txt", "B.txt"]);
    }

    /// Listing keeps dot files (upstream default) and marks symlinks without following them.
    #[test]
    fn read_entries_keeps_hidden_files_and_marks_links() {
        let dir = std::env::temp_dir().join(format!("cmux-fx-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("visible.txt"), "x").unwrap();
        std::fs::write(dir.join(".hidden"), "x").unwrap();
        std::os::unix::fs::symlink(dir.join("sub"), dir.join("link-to-sub")).unwrap();
        let entries = read_entries(&dir).unwrap();
        let find = |name: &str| entries.iter().find(|entry| entry.name == name).unwrap();
        assert!(!find("visible.txt").is_dir);
        assert!(!find(".hidden").is_dir);
        assert!(find("sub").is_dir);
        assert!(!find("link-to-sub").is_dir);
        assert!(find("link-to-sub").is_symlink);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A refresh picks up created and deleted files, in loaded folders too, and keeps
    /// the loaded children and expansion of folders that survive.
    #[test]
    fn refresh_shows_new_and_gone_files_and_keeps_expansion() {
        let dir = std::env::temp_dir().join(format!("cmux-fx-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("old.txt"), "x").unwrap();
        std::fs::write(dir.join("sub/inner.txt"), "x").unwrap();
        let mut nodes = nodes_from(&dir).unwrap();
        let sub = find_node_mut(&mut nodes, &dir.join("sub")).unwrap();
        sub.children = Some(nodes_from(&dir.join("sub")).unwrap());
        sub.expanded = true;
        let before = listing_signature(&nodes);

        std::fs::write(dir.join("agent-made.txt"), "x").unwrap();
        std::fs::write(dir.join("sub/deep-new.txt"), "x").unwrap();
        std::fs::remove_file(dir.join("old.txt")).unwrap();
        let nodes = refreshed(&dir, nodes);

        let names: Vec<&str> = nodes.iter().map(|node| node.name.as_str()).collect();
        assert_eq!(names, ["sub", "agent-made.txt"]);
        let sub = &nodes[0];
        assert!(sub.expanded);
        let inner: Vec<&str> = sub.children.as_ref().unwrap().iter().map(|n| n.name.as_str()).collect();
        assert_eq!(inner, ["deep-new.txt", "inner.txt"]);
        let after = listing_signature(&nodes);
        assert_ne!(after, before);
        // Nothing changed since: a second refresh reports the same listing.
        assert_eq!(listing_signature(&refreshed(&dir, nodes)), after);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Nested nodes are found by their filesystem path.
    #[test]
    fn find_node_walks_loaded_children() {
        let mut nodes = vec![Node {
            name: "r".into(),
            path: "/r".into(),
            is_dir: true,
            is_symlink: false,
            children: Some(vec![Node {
                name: "deep".into(),
                path: "/r/deep".into(),
                is_dir: false,
                is_symlink: false,
                children: None,
                expanded: false,
            }]),
            expanded: false,
        }];
        assert_eq!(
            find_node_mut(&mut nodes, Path::new("/r/deep")).map(|node| node.name.as_str()),
            Some("deep")
        );
        assert!(find_node_mut(&mut nodes, Path::new("/nope")).is_none());
    }

    fn test_node(name: &str, is_dir: bool, children: Option<Vec<Node>>) -> Node {
        Node {
            name: name.to_string(),
            path: PathBuf::from(format!("/r/{name}")),
            is_dir,
            is_symlink: false,
            children,
            expanded: false,
        }
    }

    fn row_names<'a>(rows: &'a [(String, &'a Node)]) -> Vec<(&'a str, &'a str)> {
        rows.iter()
            .map(|(path, node)| (path.as_str(), node.name.as_str()))
            .collect()
    }

    /// The name filter keeps matches with their folders, hides the rest, and the store
    /// indices count only emitted rows; children show while filtering even collapsed.
    #[test]
    fn filter_keeps_matches_parents_and_reindexes() {
        let mut nodes = vec![
            test_node(
                "docs",
                true,
                Some(vec![
                    test_node("alpha.md", false, None),
                    test_node("beta.md", false, None),
                ]),
            ),
            test_node("lisez-moi.txt", false, None),
            test_node("dossier-vide", true, None),
        ];

        // No filter, collapsed: top level only.
        assert_eq!(
            row_names(&visible_rows(&nodes, "")),
            [("0", "docs"), ("1", "lisez-moi.txt"), ("2", "dossier-vide")]
        );

        // Expanded directory: children keep their nested paths.
        nodes[0].expanded = true;
        assert_eq!(
            row_names(&visible_rows(&nodes, "")),
            [
                ("0", "docs"),
                ("0:0", "alpha.md"),
                ("0:1", "beta.md"),
                ("1", "lisez-moi.txt"),
                ("2", "dossier-vide")
            ]
        );

        // Filter (case-insensitive): the folder survives for its matching child even
        // while collapsed, the non-matching sibling is hidden.
        nodes[0].expanded = false;
        assert_eq!(
            row_names(&visible_rows(&nodes, "ALPHA")),
            [("0", "docs"), ("0:0", "alpha.md")]
        );

        // A plain name match is re-indexed to the top of the store.
        assert_eq!(
            row_names(&visible_rows(&nodes, "lisez")),
            [("0", "lisez-moi.txt")]
        );
        // An unloaded folder can only match by its own name.
        assert_eq!(
            row_names(&visible_rows(&nodes, "dossier-vide")),
            [("0", "dossier-vide")]
        );
        assert!(visible_rows(&nodes, "zzz").is_empty());
    }

    /// Links get their own icon; only real directories get the folder.
    #[test]
    fn icons_distinguish_links_and_folders() {
        assert_eq!(icon(false, false), "text-x-generic-symbolic");
        assert_eq!(icon(true, false), "folder-symbolic");
        assert_eq!(icon(true, true), "emblem-symbolic-link");
        assert_eq!(icon(false, true), "emblem-symbolic-link");
    }
}
