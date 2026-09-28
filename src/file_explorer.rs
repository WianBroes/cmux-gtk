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

/// Everything the tree renders from, split into cells so signal handlers re-entering
/// during a rebuild (see the module docs) never collide on one borrow.
struct ExplorerState {
    /// The applied root; `None` until the first `apply_root`.
    root: RefCell<Option<Root>>,
    /// Loaded nodes of the applied root.
    nodes: RefCell<Vec<Node>>,
    /// Selected row, kept across rebuilds so filtering can restore the cursor.
    selected: RefCell<Option<PathBuf>>,
}

/// The Files panel: root path header, lazy file tree, status label.
pub struct FileExplorer {
    /// Outer widget placed in the right sidebar's content stack.
    root: gtk4::Box,
    header: gtk4::Label,
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

        let state = Rc::new(ExplorerState {
            root: RefCell::new(None),
            nodes: RefCell::new(Vec::new()),
            selected: RefCell::new(None),
        });

        let root_widget = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        root_widget.add_css_class("file-explorer");
        root_widget.append(&header);
        root_widget.append(&scrolled);
        root_widget.append(&status);

        let explorer = FileExplorer {
            root: root_widget,
            header,
            tree: tree.clone(),
            scrolled,
            status,
            store: store.clone(),
            state: state.clone(),
        };
        explorer.connect_signals();
        explorer
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
                    // Rebuilds re-expand loaded rows here; the model already matches.
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
                if let Some(children) = &node.children {
                    for (index, child) in children.iter().enumerate() {
                        append_node(
                            &store,
                            &tree,
                            Some(iter),
                            child,
                            &format!("{parent_path}:{index}"),
                            selected.as_deref(),
                        );
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

    /// The panel widget for the sidebar's content stack.
    pub fn widget(&self) -> &gtk4::Box {
        &self.root
    }

    /// Point the panel at a workspace root; unchanged roots keep their tree as it is.
    pub fn apply_root(&self, root: Root) {
        if self.state.root.borrow().as_ref() == Some(&root) {
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
                        self.header.show();
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
                self.header.hide();
                self.status.set_text(message);
                self.status.show();
            }
        }
    }
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
    store.clear();
    for (index, node) in nodes.iter().enumerate() {
        append_node(
            store,
            tree,
            None,
            node,
            &index.to_string(),
            selected.as_deref(),
        );
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

    /// Links get their own icon; only real directories get the folder.
    #[test]
    fn icons_distinguish_links_and_folders() {
        assert_eq!(icon(false, false), "text-x-generic-symbolic");
        assert_eq!(icon(true, false), "folder-symbolic");
        assert_eq!(icon(true, true), "emblem-symbolic-link");
        assert_eq!(icon(false, true), "emblem-symbolic-link");
    }
}
