//! Short handle refs (`window:N`, `workspace:N`, `surface:N`) for socket clients, as in cmux macOS.
//! Ordinals are minted once per UUID, never reused, and lost on restart (like the upstream registry).
//! Panes keep their existing `pane:<id>` form, which already is a ref.

use std::cell::RefCell;
use std::collections::HashMap;

/// The single GTK window's stable identity (the app has one main window).
pub const MAIN_WINDOW_ID: &str = "main";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HandleKind {
    Window,
    Workspace,
    Surface,
}

impl HandleKind {
    fn prefix(self) -> &'static str {
        match self {
            HandleKind::Window => "window",
            HandleKind::Workspace => "workspace",
            HandleKind::Surface => "surface",
        }
    }
}

/// Parse `kind:N` for the kinds this registry owns; anything else is not a ref.
pub fn parse_ref(value: &str) -> Option<(HandleKind, u32)> {
    let (kind, ordinal) = value.split_once(':')?;
    let kind = match kind.to_ascii_lowercase().as_str() {
        "window" => HandleKind::Window,
        "workspace" => HandleKind::Workspace,
        "surface" => HandleKind::Surface,
        _ => return None,
    };
    Some((kind, ordinal.parse().ok()?))
}

#[derive(Default)]
struct Inner {
    next: HashMap<HandleKind, u32>,
    by_id: HashMap<(HandleKind, String), u32>,
    by_ordinal: HashMap<(HandleKind, u32), String>,
}

/// GTK-thread registry; interior mutability lets read-only handlers mint refs while listing.
#[derive(Default)]
pub struct HandleRegistry(RefCell<Inner>);

impl HandleRegistry {
    /// Return the ordinal for `id`, minting the next one on first sight.
    pub fn ordinal(&self, kind: HandleKind, id: &str) -> u32 {
        let mut inner = self.0.borrow_mut();
        let key = (kind, id.to_ascii_lowercase());
        if let Some(ordinal) = inner.by_id.get(&key) {
            return *ordinal;
        }
        let next = inner.next.entry(kind).or_insert(0);
        *next += 1;
        let ordinal = *next;
        inner.by_ordinal.insert((kind, ordinal), key.1.clone());
        inner.by_id.insert(key, ordinal);
        ordinal
    }

    /// `kind:N` for `id`, minting it if needed.
    pub fn ensure_ref(&self, kind: HandleKind, id: &str) -> String {
        format!("{}:{}", kind.prefix(), self.ordinal(kind, id))
    }

    /// The id a ref was minted for, if any.
    pub fn resolve(&self, kind: HandleKind, ordinal: u32) -> Option<String> {
        self.0.borrow().by_ordinal.get(&(kind, ordinal)).cloned()
    }

    /// Mint refs for every live window, workspace and surface in display order, so refs are
    /// stable and predictable before a client has listed anything (upstream `v2RefreshKnownRefs`).
    pub fn refresh(&self, state: &crate::app_state::AppState) {
        self.ordinal(HandleKind::Window, MAIN_WINDOW_ID);
        for (workspace, engine) in state.workspaces.iter().zip(&state.split_engines) {
            self.ordinal(HandleKind::Workspace, &workspace.uuid.to_string());
            for pane in engine.pane_info() {
                for surface in pane.surface_ids {
                    self.ordinal(HandleKind::Surface, &surface.to_string());
                }
            }
        }
    }
}

/// Parameter keys that may carry a window/workspace/surface handle. Free-text fields are never
/// rewritten, so sending the literal text "workspace:1" to a terminal stays literal.
const HANDLE_KEYS: &[&str] = &[
    "id",
    "window",
    "window_id",
    "workspace",
    "workspace_id",
    "surface",
    "surface_id",
    "surface_ref",
];
const HANDLE_LIST_KEYS: &[&str] = &["workspace_ids"];

fn for_each_handle(params: &mut serde_json::Value, visit: &mut dyn FnMut(&mut String)) {
    let Some(object) = params.as_object_mut() else {
        return;
    };
    for (key, value) in object.iter_mut() {
        if HANDLE_KEYS.contains(&key.as_str()) {
            if let serde_json::Value::String(text) = value {
                visit(text);
            }
        } else if HANDLE_LIST_KEYS.contains(&key.as_str()) {
            if let Some(items) = value.as_array_mut() {
                for item in items {
                    if let serde_json::Value::String(text) = item {
                        visit(text);
                    }
                }
            }
        } else if key == "caller" {
            for_each_handle(value, visit);
        }
    }
}

/// Refs present in handle-carrying params, deduplicated.
pub fn refs_in(params: &mut serde_json::Value) -> Vec<String> {
    let mut refs = Vec::new();
    for_each_handle(params, &mut |text| {
        if parse_ref(text).is_some() && !refs.contains(text) {
            refs.push(text.clone());
        }
    });
    refs
}

/// Replace each resolved ref by its id; refs absent from `resolved` are left untouched.
pub fn rewrite(params: &mut serde_json::Value, resolved: &HashMap<String, String>) {
    for_each_handle(params, &mut |text| {
        if let Some(id) = resolved.get(text.as_str()) {
            *text = id.clone();
        }
    });
}

/// Resolve every ref against a refreshed registry; the first unknown ref is the error.
pub fn resolve_all(
    registry: &HandleRegistry,
    state: &crate::app_state::AppState,
    refs: &[String],
) -> Result<HashMap<String, String>, String> {
    registry.refresh(state);
    let mut resolved = HashMap::new();
    for reference in refs {
        let id = parse_ref(reference)
            .and_then(|(kind, ordinal)| registry.resolve(kind, ordinal))
            .ok_or_else(|| format!("{reference} not found"))?;
        resolved.insert(reference.clone(), id);
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ordinals_are_per_kind_stable_and_never_reused() {
        let registry = HandleRegistry::default();
        assert_eq!(
            registry.ensure_ref(HandleKind::Workspace, "A"),
            "workspace:1"
        );
        assert_eq!(registry.ensure_ref(HandleKind::Surface, "b"), "surface:1");
        assert_eq!(
            registry.ensure_ref(HandleKind::Workspace, "c"),
            "workspace:2"
        );
        assert_eq!(
            registry.ensure_ref(HandleKind::Workspace, "a"),
            "workspace:1"
        );
        assert_eq!(
            registry.resolve(HandleKind::Workspace, 2).as_deref(),
            Some("c")
        );
        assert_eq!(registry.resolve(HandleKind::Window, 1), None);
    }

    #[test]
    fn parse_ref_accepts_only_owned_kinds() {
        assert_eq!(parse_ref("workspace:3"), Some((HandleKind::Workspace, 3)));
        assert_eq!(parse_ref("Window:1"), Some((HandleKind::Window, 1)));
        assert_eq!(parse_ref("pane:1"), None);
        assert_eq!(parse_ref("surface:x"), None);
        assert_eq!(parse_ref("5e8253b3-3139-4e7c-926a-dbd8ec0b66db"), None);
    }

    #[test]
    fn only_handle_keys_are_rewritten() {
        let mut params = json!({
            "id": "workspace:1",
            "text": "workspace:1",
            "workspace_ids": ["workspace:2", "u"],
            "caller": {"surface_id": "surface:4"},
            "pane": "pane:7",
        });
        let mut refs = refs_in(&mut params);
        refs.sort();
        assert_eq!(refs, vec!["surface:4", "workspace:1", "workspace:2"]);
        let resolved = HashMap::from([
            ("workspace:1".to_string(), "w1".to_string()),
            ("workspace:2".to_string(), "w2".to_string()),
            ("surface:4".to_string(), "s4".to_string()),
        ]);
        rewrite(&mut params, &resolved);
        assert_eq!(
            params,
            json!({
                "id": "w1",
                "text": "workspace:1",
                "workspace_ids": ["w2", "u"],
                "caller": {"surface_id": "s4"},
                "pane": "pane:7",
            })
        );
    }
}
