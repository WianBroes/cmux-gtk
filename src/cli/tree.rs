//! `tree` and `list-pane-surfaces`, assembled client-side from the list methods like the
//! macOS legacy path (`buildLegacyTreePayload`, `buildTreeWorkspaceNode`, `renderTreeText`).

use super::socket_client::{CliError, SocketClient};
use serde_json::{json, Map, Value};

/// Handles of the focused (or calling) window, workspace, pane and surface.
#[derive(Default)]
struct TreePath {
    window: Option<String>,
    workspace: Option<String>,
    pane: Option<String>,
    surface: Option<String>,
}

impl TreePath {
    /// Read `<kind>_ref`, falling back to `<kind>_id`, from an identify context object.
    fn from(context: &Value) -> Self {
        let handle = |kind: &str| {
            [format!("{kind}_ref"), format!("{kind}_id")]
                .iter()
                .find_map(|key| {
                    context
                        .get(key)
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                })
                .map(str::to_owned)
        };
        Self {
            window: handle("window"),
            workspace: handle("workspace"),
            pane: handle("pane"),
            surface: handle("surface"),
        }
    }
}

/// True when the record's `id`, `uuid` or `ref` equals the handle.
fn matches(item: &Value, handle: Option<&str>) -> bool {
    let Some(handle) = handle.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    ["id", "uuid", "ref"]
        .iter()
        .any(|key| item.get(key).and_then(Value::as_str) == Some(handle))
}

/// The records of one list response, empty when the field is missing.
fn records(value: &Value, field: &str) -> Vec<Value> {
    value
        .get(field)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Caller context from the environment, as macOS `treeCallerContextFromEnvironment`.
fn caller_from_environment() -> Option<Value> {
    let mut caller = Map::new();
    for (key, variable) in [
        ("workspace_id", "CMUX_WORKSPACE_ID"),
        ("surface_id", "CMUX_SURFACE_ID"),
    ] {
        if let Some(value) = std::env::var(variable)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
        {
            caller.insert(key.into(), value.into());
        }
    }
    (!caller.is_empty()).then_some(Value::Object(caller))
}

/// One workspace with its panes, each carrying its surfaces in tab order.
fn workspace_node(
    client: &mut SocketClient,
    mut workspace: Value,
    active: &TreePath,
    caller: &TreePath,
) -> Result<Value, CliError> {
    let id = workspace
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let params = json!({ "workspace_id": id });
    let panes = records(&client.call("pane.list", params.clone())?, "panes");
    let surfaces = records(&client.call("surface.list", params)?, "surfaces");
    let pane_nodes: Vec<Value> = panes
        .into_iter()
        .map(|mut pane| {
            let pane_ref = pane.get("ref").and_then(Value::as_str).map(str::to_owned);
            let mut children: Vec<Value> = surfaces
                .iter()
                .filter(|surface| {
                    surface.get("pane_ref").and_then(Value::as_str) == pane_ref.as_deref()
                })
                .cloned()
                .map(|mut surface| {
                    let selected = surface
                        .get("selected_in_pane")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    surface["selected"] = selected.into();
                    surface["active"] = matches(&surface, active.surface.as_deref()).into();
                    surface["here"] = matches(&surface, caller.surface.as_deref()).into();
                    surface
                })
                .collect();
            children.sort_by_key(|surface| surface.get("index_in_pane").and_then(Value::as_u64));
            pane["active"] = matches(&pane, active.pane.as_deref()).into();
            pane["surfaces"] = children.into();
            pane
        })
        .collect();
    workspace["active"] = matches(&workspace, active.workspace.as_deref()).into();
    workspace["panes"] = pane_nodes.into();
    Ok(workspace)
}

/// Build `{active, caller, windows[].workspaces[].panes[].surfaces[]}` as macOS `tree --json`.
pub fn build(
    client: &mut SocketClient,
    all: bool,
    workspace: Option<&str>,
    window: Option<&str>,
) -> Result<Value, CliError> {
    if all && window.is_some() {
        return Err(CliError::Command(
            "tree: --window cannot be combined with --all".into(),
        ));
    }
    let mut identify_params = json!({});
    if let Some(caller) = caller_from_environment() {
        identify_params["caller"] = caller;
    }
    if let Some(window) = window {
        identify_params["window_id"] = window.into();
    }
    let identify = client.call("system.identify", identify_params)?;
    let focused = identify.get("focused").cloned().unwrap_or(Value::Null);
    let caller = identify.get("caller").cloned().unwrap_or(Value::Null);
    let active = TreePath::from(&focused);
    let here = TreePath::from(&caller);

    let windows = records(&client.call("window.list", json!({}))?, "windows");
    let targets: Vec<Value> = if all {
        windows
    } else if let Some(window) = window {
        let chosen: Vec<Value> = windows
            .into_iter()
            .filter(|item| matches(item, Some(window)))
            .collect();
        if chosen.is_empty() {
            return Err(CliError::Command(format!("Window not found: {window}")));
        }
        chosen
    } else {
        let current: Vec<Value> = windows
            .iter()
            .filter(|item| matches(item, active.window.as_deref()))
            .cloned()
            .collect();
        if current.is_empty() {
            windows.into_iter().take(1).collect()
        } else {
            current
        }
    };

    // Linux has a single window: every workspace belongs to it.
    let mut workspaces = records(&client.call("workspace.list", json!({}))?, "workspaces");
    if let Some(workspace) = workspace {
        workspaces.retain(|item| matches(item, Some(workspace)));
        if workspaces.is_empty() {
            return Err(CliError::Command("Workspace not found".into()));
        }
    }
    let mut workspace_nodes = Vec::new();
    for item in workspaces {
        workspace_nodes.push(workspace_node(client, item, &active, &here)?);
    }
    let window_nodes: Vec<Value> = targets
        .into_iter()
        .map(|mut item| {
            let current = matches(&item, active.window.as_deref());
            item["current"] = current.into();
            item["active"] = current.into();
            item["workspace_count"] = workspace_nodes.len().into();
            item["workspaces"] = workspace_nodes.clone().into();
            item
        })
        .collect();
    Ok(json!({ "active": focused, "caller": caller, "windows": window_nodes }))
}

/// Surfaces of one pane (the focused pane by default), as macOS `pane.surfaces`.
pub fn pane_surfaces(client: &mut SocketClient, pane: Option<&str>) -> Result<Value, CliError> {
    let pane = match pane {
        Some(pane) => pane.to_owned(),
        None => client
            .call("system.identify", json!({}))?
            .pointer("/focused/pane_ref")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| CliError::Command("No focused pane".into()))?,
    };
    let owner = records(&client.call("pane.list", json!({}))?, "panes")
        .into_iter()
        .find(|item| matches(item, Some(&pane)))
        .ok_or_else(|| CliError::Command(format!("Pane not found: {pane}")))?;
    let pane_ref = owner.get("ref").cloned().unwrap_or(Value::Null);
    let params = json!({ "workspace_id": owner.get("workspace_id") });
    let surfaces: Vec<Value> = records(&client.call("surface.list", params)?, "surfaces")
        .into_iter()
        .filter(|surface| surface.get("pane_ref") == Some(&pane_ref))
        .collect();
    Ok(json!({
        "pane_id": pane_ref,
        "pane_ref": pane_ref,
        "workspace_id": owner.get("workspace_id"),
        "workspace_ref": owner.get("workspace_ref"),
        "surfaces": surfaces,
    }))
}

/// `kind handle`, preferring the short reference like the macOS default id format.
fn handle(item: &Value) -> &str {
    ["ref", "id", "uuid"]
        .iter()
        .find_map(|key| item.get(key).and_then(Value::as_str))
        .unwrap_or("?")
}

fn flag(item: &Value, key: &str) -> bool {
    item.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn text(item: &Value, key: &str) -> Option<String> {
    item.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn window_label(window: &Value) -> String {
    let mut parts = vec![format!("window {}", handle(window))];
    if flag(window, "current") {
        parts.push("[current]".into());
    }
    if flag(window, "active") {
        parts.push("◀ active".into());
    }
    parts.join(" ")
}

fn workspace_label(workspace: &Value) -> String {
    let mut parts = vec![format!("workspace {}", handle(workspace))];
    if let Some(title) = text(workspace, "title") {
        parts.push(format!("\"{title}\""));
    }
    if flag(workspace, "selected") {
        parts.push("[selected]".into());
    }
    if flag(workspace, "active") {
        parts.push("◀ active".into());
    }
    parts.join(" ")
}

fn pane_label(pane: &Value) -> String {
    let mut parts = vec![format!("pane {}", handle(pane))];
    if flag(pane, "focused") {
        parts.push("[focused]".into());
    }
    if flag(pane, "active") {
        parts.push("◀ active".into());
    }
    parts.join(" ")
}

fn surface_label(surface: &Value) -> String {
    let kind = text(surface, "type").unwrap_or_else(|| "unknown".into());
    let mut parts = vec![format!("surface {}", handle(surface)), format!("[{kind}]")];
    if let Some(title) = text(surface, "title") {
        parts.push(format!("\"{title}\""));
    }
    if flag(surface, "selected") {
        parts.push("[selected]".into());
    }
    if flag(surface, "active") {
        parts.push("◀ active".into());
    }
    if flag(surface, "here") {
        parts.push("◀ here".into());
    }
    if kind == "browser" {
        if let Some(url) = text(surface, "url") {
            parts.push(url);
        }
    }
    parts.join(" ")
}

/// Box-drawing rendering identical in shape to macOS `renderTreeText`.
pub fn render_text(result: &Value) -> String {
    let windows = records(result, "windows");
    if windows.is_empty() {
        return "No windows".into();
    }
    let branch = |last: bool| if last { "└── " } else { "├── " };
    let indent = |last: bool| if last { "    " } else { "│   " };
    let mut lines = Vec::new();
    for window in &windows {
        lines.push(window_label(window));
        let workspaces = records(window, "workspaces");
        for (i, workspace) in workspaces.iter().enumerate() {
            let last_workspace = i + 1 == workspaces.len();
            lines.push(format!(
                "{}{}",
                branch(last_workspace),
                workspace_label(workspace)
            ));
            let panes = records(workspace, "panes");
            for (j, pane) in panes.iter().enumerate() {
                let last_pane = j + 1 == panes.len();
                lines.push(format!(
                    "{}{}{}",
                    indent(last_workspace),
                    branch(last_pane),
                    pane_label(pane)
                ));
                let surfaces = records(pane, "surfaces");
                for (k, surface) in surfaces.iter().enumerate() {
                    lines.push(format!(
                        "{}{}{}{}",
                        indent(last_workspace),
                        indent(last_pane),
                        branch(k + 1 == surfaces.len()),
                        surface_label(surface)
                    ));
                }
            }
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text tree nests workspace → pane → surface with macOS labels and markers.
    #[test]
    fn render_text_matches_macos_shape() {
        let result = json!({"windows": [{
            "ref": "window:1", "current": true, "active": true,
            "workspaces": [
                {"ref": "workspace:1", "title": "Code", "selected": true, "active": true, "panes": [
                    {"ref": "pane:7", "focused": true, "active": true, "surfaces": [
                        {"ref": "surface:1", "type": "terminal", "title": "zsh", "selected": true, "active": true, "here": true},
                        {"ref": "surface:2", "type": "browser", "url": "https://example.org"}
                    ]}
                ]},
                {"ref": "workspace:2", "title": "", "panes": []}
            ]
        }]});
        assert_eq!(
            render_text(&result),
            "window window:1 [current] ◀ active\n\
             ├── workspace workspace:1 \"Code\" [selected] ◀ active\n\
             │   └── pane pane:7 [focused] ◀ active\n\
             │       ├── surface surface:1 [terminal] \"zsh\" [selected] ◀ active ◀ here\n\
             │       └── surface surface:2 [browser] https://example.org\n\
             └── workspace workspace:2"
        );
        assert_eq!(render_text(&json!({"windows": []})), "No windows");
    }

    /// Identify contexts prefer references and ignore empty values.
    #[test]
    fn tree_path_prefers_refs() {
        let path = TreePath::from(&json!({
            "window_id": "main", "window_ref": "window:1",
            "workspace_id": "uuid-w", "pane_ref": "", "pane_id": "pane:3",
            "surface_ref": "surface:2"
        }));
        assert_eq!(path.window.as_deref(), Some("window:1"));
        assert_eq!(path.workspace.as_deref(), Some("uuid-w"));
        assert_eq!(path.pane.as_deref(), Some("pane:3"));
        assert_eq!(path.surface.as_deref(), Some("surface:2"));
        assert!(matches(
            &json!({"uuid": "uuid-w"}),
            path.workspace.as_deref()
        ));
        assert!(!matches(&json!({"ref": "x"}), None));
    }
}
