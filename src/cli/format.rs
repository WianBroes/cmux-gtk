//! Human-readable output formatters for cmux CLI responses.
//!
//! Handles color support (D-07), list formatting with active markers (D-08),
//! and mutation success messages (D-09).

use super::args::IdFormat;
use serde_json::Value;
use std::io::IsTerminal;

/// Determine whether to use color output based on the --color flag value.
pub fn use_color(color_flag: &str) -> bool {
    match color_flag {
        "always" => true,
        "never" => false,
        _ => std::io::stdout().is_terminal(),
    }
}

/// Highlight active or successful output only when terminal color is enabled.
fn green(s: &str, color: bool) -> String {
    if color {
        format!("\x1b[1;32m{}\x1b[0m", s)
    } else {
        s.to_string()
    }
}

/// De-emphasize supplementary output while preserving plain-text mode.
fn dim(s: &str, color: bool) -> String {
    if color {
        format!("\x1b[2m{}\x1b[0m", s)
    } else {
        s.to_string()
    }
}

/// Format a workspace list response with active marker.
pub fn format_workspace_list(result: &Value, color: bool) -> String {
    let workspaces = match result.get("workspaces").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return format_fallback(result),
    };
    if workspaces.is_empty() {
        return "No workspaces".to_string();
    }
    let mut lines = Vec::new();
    for (i, ws) in workspaces.iter().enumerate() {
        let selected = ws
            .get("selected")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let title = ws
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("untitled");
        let pane_count = match ws.get("pane_count").and_then(Value::as_u64) {
            Some(1) => "1 pane".to_owned(),
            Some(count) => format!("{count} panes"),
            None => "pane count unavailable".to_owned(),
        };
        let marker = if selected { "*" } else { " " };
        let line = format!("{} {}: {} ({})", marker, i + 1, title, pane_count);
        if selected && color {
            lines.push(green(&line, true));
        } else {
            lines.push(line);
        }
    }
    lines.join("\n")
}

/// Format terminal/browser surface identities with the active-tab marker.
pub fn format_surface_list(result: &Value, color: bool) -> String {
    format_identity_list(result, "surfaces", "No surfaces", color)
}

/// Format session-local pane identities with the focused-pane marker.
pub fn format_pane_list(result: &Value, color: bool) -> String {
    format_identity_list(result, "panes", "No panes", color)
}

/// Render ordered identity records using current protocol fields and their legacy aliases.
/// Preserve full IDs so output can be passed back to focus/close commands without guessing.
fn format_identity_list(result: &Value, field: &str, empty: &str, color: bool) -> String {
    let Some(records) = result.get(field).and_then(Value::as_array) else {
        return format_fallback(result);
    };
    if records.is_empty() {
        return empty.to_owned();
    }
    records
        .iter()
        .map(|record| {
            let focused = record
                .get("active")
                .and_then(Value::as_bool)
                .or_else(|| record.get("focused").and_then(Value::as_bool))
                .unwrap_or(false);
            let id = record
                .get("id")
                .and_then(Value::as_str)
                .or_else(|| record.get("uuid").and_then(Value::as_str))
                .unwrap_or("unknown");
            let title = record.get("title").and_then(Value::as_str).unwrap_or("");
            let marker = if focused { "*" } else { " " };
            let line = if title.is_empty() {
                format!("{marker} {id}")
            } else {
                format!("{marker} {id} ({title})")
            };
            green(&line, focused && color)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format a window list response.
pub fn format_window_list(result: &Value, color: bool) -> String {
    let windows = match result.get("windows").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return format_fallback(result),
    };
    if windows.is_empty() {
        return "No windows".to_string();
    }
    let mut lines = Vec::new();
    for (i, win) in windows.iter().enumerate() {
        let focused = win
            .get("focused")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let title = win.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let marker = if focused { "*" } else { " " };
        let line = format!("{} {}: {}", marker, i + 1, title);
        if focused && color {
            lines.push(green(&line, true));
        } else {
            lines.push(line);
        }
    }
    lines.join("\n")
}

/// Format identify response.
///
/// The focused topology and the caller anchor are appended as short references when the
/// server returns them; a response without those fields keeps the single-line form.
fn format_identify(result: &Value) -> String {
    let version = result
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let platform = result
        .get("platform")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let pid = result.get("pid").and_then(|v| v.as_u64());
    let mut lines = vec![match pid {
        Some(p) => format!("cmux {} ({}) pid {}", version, platform, p),
        None => format!("cmux {} ({})", version, platform),
    }];
    for (label, field) in [("focused", "focused"), ("caller", "caller")] {
        if let Some(context) = identify_context(result, field) {
            lines.push(format!("{label}: {context}"));
        }
    }
    lines.join("\n")
}

/// Render one `focused`/`caller` object as its identities, preferring short references.
///
/// Returns `None` when the field is absent, empty or not an object, so a server that does
/// not send topology keeps the previous output.
fn identify_context(result: &Value, field: &str) -> Option<String> {
    let record = result.get(field)?;
    if !record.is_object() {
        return None;
    }
    let identities = [
        ("window_id", "window_ref"),
        ("workspace_id", "workspace_ref"),
        ("pane_id", "pane_ref"),
        ("surface_id", "surface_ref"),
    ]
    .into_iter()
    .filter_map(|(id_key, ref_key)| {
        record
            .get(ref_key)
            .and_then(Value::as_str)
            .or_else(|| record.get(id_key).and_then(Value::as_str))
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
    .collect::<Vec<_>>();
    (!identities.is_empty()).then(|| identities.join(" "))
}

/// Apply `--id-format` to a JSON response, dropping the redundant half of each identity.
///
/// Objects are rewritten after their children so nested records and arrays are covered.
/// `Refs` keeps `ref`/`*_ref`/`*_refs` and removes `id`/`*_id`/`*_ids` when the matching
/// reference exists, `uuids` does the reverse, and `both` leaves the response untouched.
pub fn format_ids(value: &mut Value, mode: IdFormat) {
    match value {
        Value::Object(map) => {
            for child in map.values_mut() {
                format_ids(child, mode);
            }
            match mode {
                IdFormat::Both => {}
                IdFormat::Refs => {
                    if map.contains_key("ref") && map.contains_key("id") {
                        map.remove("id");
                    }
                    let keys = map.keys().cloned().collect::<Vec<_>>();
                    for key in &keys {
                        if let Some(prefix) = key.strip_suffix("_id") {
                            if map.contains_key(&format!("{prefix}_ref")) {
                                map.remove(key);
                            }
                        }
                    }
                    for key in &keys {
                        if let Some(prefix) = key.strip_suffix("_ids") {
                            if map.contains_key(&format!("{prefix}_refs")) {
                                map.remove(key);
                            }
                        }
                    }
                }
                IdFormat::Uuids => {
                    if map.contains_key("ref") && map.contains_key("id") {
                        map.remove("ref");
                    }
                    let keys = map.keys().cloned().collect::<Vec<_>>();
                    for key in &keys {
                        if let Some(prefix) = key.strip_suffix("_ref") {
                            if map.contains_key(&format!("{prefix}_id")) {
                                map.remove(key);
                            }
                        }
                    }
                    for key in &keys {
                        if let Some(prefix) = key.strip_suffix("_refs") {
                            if map.contains_key(&format!("{prefix}_ids")) {
                                map.remove(key);
                            }
                        }
                    }
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                format_ids(item, mode);
            }
        }
        _ => {}
    }
}

/// Format capabilities response.
fn format_capabilities(result: &Value, color: bool) -> String {
    let methods = match result.get("methods").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return format_fallback(result),
    };
    let mut lines = vec![format!("{} methods available:", methods.len())];
    for m in methods {
        let name = m.as_str().unwrap_or("?");
        lines.push(format!("  {}", dim(name, color)));
    }
    lines.join("\n")
}

/// Format notification list response.
fn format_notification_list(result: &Value, color: bool) -> String {
    let notifications = match result.get("notifications").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return format_fallback(result),
    };
    if notifications.is_empty() {
        return "No notifications".to_string();
    }
    let mut lines = Vec::new();
    for n in notifications {
        let id = n.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
        let attention = !n.get("is_read").and_then(|v| v.as_bool()).unwrap_or(false);
        let marker = if attention { "!" } else { " " };
        let title = n
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Notification");
        let line = format!("{marker} {id} {title}");
        if attention && color {
            lines.push(green(&line, true));
        } else {
            lines.push(line);
        }
    }
    lines.join("\n")
}

/// Format a mutation command result with a success message.
pub fn format_mutation(command_name: &str, result: &Value) -> String {
    let id = result.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let title = result
        .get("title")
        .or_else(|| result.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match command_name {
        "workspace.create" => {
            if title.is_empty() {
                format!("Created workspace: {}", id)
            } else {
                format!("Created workspace: {} ({})", title, id)
            }
        }
        "workspace.close" => format!("Closed workspace: {}", id),
        "workspace.rename" => {
            let name = result.get("name").and_then(|v| v.as_str()).unwrap_or(title);
            format!("Renamed workspace {} to: {}", id, name)
        }
        "workspace.set_description" => format!("Set workspace description: {}", id),
        "workspace.clear_description" => format!("Cleared workspace description: {}", id),
        // Upstream's reorder summary, with `plan ` prefixing the `--dry-run` reply.
        "workspace.reorder" => {
            let workspace = result
                .get("workspace_ref")
                .or_else(|| result.get("workspace_id"))
                .or_else(|| result.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let window = result
                .get("window_ref")
                .or_else(|| result.get("window_id"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let index = result
                .get("index")
                .map(|value| value.to_string())
                .unwrap_or_else(|| "?".to_owned());
            let plan = if result
                .get("dry_run")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "plan "
            } else {
                ""
            };
            format!("OK {plan}workspace={workspace} window={window} index={index}")
        }
        // Upstream's creation summary: `OK surface:N pane:N workspace:N`.
        "surface.split" | "pane.create" | "surface.create" => {
            let field = |ref_key: &str, id_key: &str| {
                result
                    .get(ref_key)
                    .or_else(|| result.get(id_key))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            };
            let mut parts = vec!["OK".to_owned()];
            parts.extend(field("surface_ref", "surface_id"));
            parts.extend(field("pane_ref", "pane_id"));
            parts.extend(field("workspace_ref", "workspace_id"));
            parts.join(" ")
        }
        "surface.close" => format!("Closed surface: {}", id),
        // Upstream's tab summary: `OK action=… tab=… workspace=… [closed=…] [created=…]`.
        "tab.action" => {
            let action = result
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let tab = result
                .get("surface_ref")
                .or_else(|| result.get("surface_id"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let workspace = result
                .get("workspace_ref")
                .or_else(|| result.get("workspace_id"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let mut line = format!("OK action={action} tab={tab} workspace={workspace}");
            if let Some(closed) = result.get("closed").filter(|value| !value.is_null()) {
                line.push_str(&format!(" closed={closed}"));
            }
            if let Some(created) = result
                .get("created_surface_ref")
                .or_else(|| result.get("created_surface_id"))
                .and_then(Value::as_str)
            {
                line.push_str(&format!(" created={created}"));
            }
            line
        }
        // Upstream's action summary: `OK action=… workspace=… window=… [closed=…] [index=…] [color=…]`.
        "workspace.action" => {
            let action = result
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let workspace = result
                .get("workspace_ref")
                .or_else(|| result.get("workspace_id"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let window = result
                .get("window_ref")
                .or_else(|| result.get("window_id"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let mut line = format!("OK action={action} workspace={workspace} window={window}");
            if let Some(closed) = result.get("closed").filter(|value| !value.is_null()) {
                line.push_str(&format!(" closed={closed}"));
            }
            if let Some(index) = result.get("index").filter(|value| !value.is_null()) {
                line.push_str(&format!(" index={index}"));
            }
            if let Some(color) = result.get("color").and_then(Value::as_str) {
                line.push_str(&format!(" color={color}"));
            }
            line
        }
        "surface.move" => format!("Moved surface: {}", id),
        "surface.reorder" => format!("Reordered surface: {}", id),
        "surface.drag_to_split" => format!("Split moved surface: {}", id),
        "surface.split_off" => format!("Split out surface: {}", id),
        _ => String::new(),
    }
}

/// Format a command response for human-readable output.
///
/// If `json_mode` is true, returns raw JSON (D-06).
/// Otherwise, picks the appropriate formatter based on the method name.
pub fn format_response(method: &str, result: &Value, json_mode: bool, color: bool) -> String {
    if json_mode {
        return serde_json::to_string_pretty(result).unwrap_or_default();
    }

    match method {
        "workspace.list" => format_workspace_list(result, color),
        "workspace.current" => {
            let title = result
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let id = result.get("id").and_then(|v| v.as_str()).unwrap_or("");
            format!("{} ({})", title, id)
        }
        "surface.list" | "pane.surfaces" => format_surface_list(result, color),
        "tree" => super::tree::render_text(result),
        "pane.list" => format_pane_list(result, color),
        "window.list" => format_window_list(result, color),
        "window.current" => {
            let title = result
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            title.to_string()
        }
        "system.ping" => result
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("pong")
            .to_string(),
        "system.identify" => format_identify(result),
        "system.capabilities" => format_capabilities(result, color),
        "notification.list" => format_notification_list(result, color),
        "sidebar.log" | "sidebar.clear_log" => "OK".to_string(),
        "sidebar.list_log" => format_log_list(result),
        "sidebar.state" => format_sidebar_state(result),
        "debug.layout" => serde_json::to_string_pretty(result).unwrap_or_default(),

        // Mutation commands: show success message
        "workspace.create"
        | "workspace.close"
        | "workspace.rename"
        | "workspace.reorder"
        | "workspace.action"
        | "tab.action"
        | "surface.split"
        | "pane.create"
        | "surface.create"
        | "surface.close"
        | "surface.move"
        | "surface.reorder"
        | "surface.drag_to_split"
        | "surface.split_off" => {
            let msg = format_mutation(method, result);
            if msg.is_empty() {
                format_fallback(result)
            } else {
                msg
            }
        }

        // Browser list: human-readable table
        "browser.list" => format_browser_list(result, color),

        // Default: pretty-print JSON for uncommon commands
        _ => format_fallback(result),
    }
}

/// One log line as upstream `list_log` prints it: `[source] [level] message`.
fn log_line(entry: &Value) -> String {
    let field = |key| entry.get(key).and_then(|v| v.as_str()).unwrap_or("");
    let line = format!("[{}] {}", field("level"), field("message"));
    match field("source") {
        "" => line,
        source => format!("[{source}] {line}"),
    }
}

/// `list-log`: one line per entry, oldest first.
fn format_log_list(result: &Value) -> String {
    match result.get("logs").and_then(|v| v.as_array()) {
        Some(logs) if !logs.is_empty() => logs.iter().map(log_line).collect::<Vec<_>>().join("\n"),
        _ => "No log entries".to_string(),
    }
}

/// Entries of a keyed map, highest priority first then by key (the sidebar's order).
fn by_priority(map: Option<&Value>) -> Vec<(&String, &Value)> {
    let mut entries: Vec<_> = map
        .and_then(|v| v.as_object())
        .map(|map| map.iter().collect())
        .unwrap_or_default();
    let priority = |value: &Value| value.get("priority").and_then(|v| v.as_i64()).unwrap_or(0);
    entries.sort_by(|(a, av), (b, bv)| priority(bv).cmp(&priority(av)).then_with(|| a.cmp(b)));
    entries
}

/// `sidebar-state`: upstream's `key=value` lines (`sidebarState`), fields we do not track say `none`.
fn format_sidebar_state(result: &Value) -> String {
    let text = |value: Option<&Value>| value.and_then(|v| v.as_str()).map(str::to_owned);
    let cwd = text(result.get("cwd"));
    let mut lines = vec![
        format!(
            "tab={}",
            text(result.get("workspace_id")).unwrap_or_default()
        ),
        format!(
            "color={}",
            text(result.get("color")).unwrap_or("none".into())
        ),
        format!("cwd={}", cwd.clone().unwrap_or_default()),
        format!("focused_cwd={}", cwd.unwrap_or("unknown".into())),
        format!(
            "focused_panel={}",
            text(result.get("focused_surface_id")).unwrap_or("unknown".into())
        ),
    ];
    lines.push(match result.get("git").filter(|v| !v.is_null()) {
        Some(git) => format!(
            "git_branch={} {}",
            text(git.get("branch")).unwrap_or_default(),
            if git.get("dirty").and_then(|v| v.as_bool()).unwrap_or(false) {
                "dirty"
            } else {
                "clean"
            }
        ),
        None => "git_branch=none".into(),
    });
    lines.push("pr=none".into());
    lines.push("pr_label=none".into());
    let mut ports: Vec<u64> = result
        .get("ports")
        .and_then(|v| v.as_array())
        .map(|ports| {
            ports
                .iter()
                .filter_map(|p| p.get("port").and_then(|v| v.as_u64()))
                .collect()
        })
        .unwrap_or_default();
    ports.sort_unstable();
    ports.dedup();
    lines.push(if ports.is_empty() {
        "ports=none".into()
    } else {
        format!(
            "ports={}",
            ports
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )
    });
    lines.push(match result.get("progress").filter(|v| !v.is_null()) {
        Some(progress) => format!(
            "progress={:.2} {}",
            progress
                .get("value")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            text(progress.get("label")).unwrap_or_default()
        )
        .trim_end()
        .to_string(),
        None => "progress=none".into(),
    });
    let statuses = by_priority(result.get("statuses"));
    lines.push(format!("status_count={}", statuses.len()));
    for (key, status) in statuses {
        let mut line = format!("  {key}={}", text(status.get("value")).unwrap_or_default());
        for field in ["icon", "color", "url"] {
            if let Some(value) = text(status.get(field)) {
                line.push_str(&format!(" {field}={value}"));
            }
        }
        if let Some(priority) = status
            .get("priority")
            .and_then(|v| v.as_i64())
            .filter(|p| *p != 0)
        {
            line.push_str(&format!(" priority={priority}"));
        }
        if let Some(format) = text(status.get("format")).filter(|f| f != "plain") {
            line.push_str(&format!(" format={format}"));
        }
        lines.push(line);
    }
    let blocks = by_priority(result.get("blocks"));
    lines.push(format!("meta_block_count={}", blocks.len()));
    for (key, block) in blocks {
        let mut line = format!(
            "  {key}={}",
            text(block.get("markdown"))
                .unwrap_or_default()
                .replace('\n', "\\n")
        );
        if let Some(priority) = block
            .get("priority")
            .and_then(|v| v.as_i64())
            .filter(|p| *p != 0)
        {
            line.push_str(&format!(" priority={priority}"));
        }
        lines.push(line);
    }
    let logs = result
        .get("logs")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    lines.push(format!("log_count={}", logs.len()));
    for entry in &logs[logs.len().saturating_sub(5)..] {
        let field = |key| entry.get(key).and_then(|v| v.as_str()).unwrap_or("");
        lines.push(format!("  [{}] {}", field("level"), field("message")));
    }
    lines.join("\n")
}

/// Format a browser surface list response.
pub fn format_browser_list(result: &Value, _color: bool) -> String {
    let surfaces = match result.get("surfaces").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return format_fallback(result),
    };
    if surfaces.is_empty() {
        return "No browser surfaces".to_string();
    }
    let mut lines = Vec::new();
    lines.push(format!(
        "{:<12} {:<38} {:<50} {}",
        "REF", "UUID", "URL", "STATUS"
    ));
    for s in surfaces {
        let ref_str = s.get("ref").and_then(|v| v.as_str()).unwrap_or("-");
        let uuid = s.get("uuid").and_then(|v| v.as_str()).unwrap_or("-");
        let url = s.get("url").and_then(|v| v.as_str()).unwrap_or("-");
        let status = s
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        lines.push(format!(
            "{:<12} {:<38} {:<50} {}",
            ref_str, uuid, url, status
        ));
    }
    lines.join("\n")
}

/// Fallback: pretty-print JSON.
fn format_fallback(result: &Value) -> String {
    serde_json::to_string_pretty(result).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Missing layout metadata must not be rendered as a known zero-pane workspace.
    #[test]
    fn workspace_count_availability() {
        let result = json!({"workspaces": [
            {"title": "known", "pane_count": 2, "selected": true},
            {"title": "unknown", "pane_count": null}
        ]});
        assert_eq!(
            format_workspace_list(&result, false),
            "* 1: known (2 panes)\n  2: unknown (pane count unavailable)"
        );
    }

    /// Current surface records must produce reusable UUIDs and truthful selection markers.
    #[test]
    fn surface_protocol_identity_output() {
        let id = "20000000-0000-4000-8000-000000000002";
        let result = json!({"surfaces": [{"uuid": id, "active": true}]});
        assert_eq!(
            format_response("surface.list", &result, false, false),
            format!("* {id}")
        );
        assert_eq!(
            format_surface_list(&result, true),
            format!("\x1b[1;32m* {id}\x1b[0m")
        );
        assert_eq!(
            serde_json::from_str::<Value>(&format_response("surface.list", &result, true, true))
                .unwrap(),
            result
        );
    }

    /// Pane references and non-ASCII legacy IDs retain their full identities without byte slicing.
    #[test]
    fn pane_protocol_and_legacy_fields() {
        let result = json!({"panes": [
            {"id": "pane:100001", "active": false, "focused": true},
            {"id": "ééééééééé", "focused": true, "title": "terminal"}
        ]});
        assert_eq!(
            format_pane_list(&result, false),
            "  pane:100001\n* ééééééééé (terminal)"
        );
        assert_eq!(
            format_surface_list(&json!({"surfaces": []}), false),
            "No surfaces"
        );
        assert_eq!(format_pane_list(&json!({"panes": []}), false), "No panes");
    }

    /// `--id-format` drops only identities that have a counterpart, at every depth.
    #[test]
    fn id_format_shapes_nested_responses() {
        let value = json!({
            "id": "uuid-1",
            "ref": "workspace:1",
            "workspace_id": "uuid-2",
            "workspace_ref": "workspace:2",
            "workspace_ids": ["uuid-2"],
            "workspace_refs": ["workspace:2"],
            "surface_id": "uuid-3",
            "id_only": "uuid-4",
            "title": "Keep",
            "workspaces": [
                {"id": "uuid-5", "ref": "workspace:5"},
                {"pane_ids": ["uuid-6"], "pane_refs": ["pane:6"]}
            ],
            "focused": {"window_id": "uuid-7", "window_ref": "window:7"}
        });

        let mut refs = value.clone();
        format_ids(&mut refs, IdFormat::Refs);
        assert_eq!(refs["ref"], "workspace:1");
        assert!(refs.get("id").is_none());
        assert!(refs.get("workspace_id").is_none());
        assert!(refs.get("workspace_ids").is_none());
        assert_eq!(refs["workspace_ref"], "workspace:2");
        assert_eq!(refs["workspace_refs"][0], "workspace:2");
        // No `surface_ref` or `id_ref` accompanies these, so they survive.
        assert_eq!(refs["surface_id"], "uuid-3");
        assert_eq!(refs["id_only"], "uuid-4");
        assert_eq!(refs["title"], "Keep");
        assert!(refs["workspaces"][0].get("id").is_none());
        assert_eq!(refs["workspaces"][0]["ref"], "workspace:5");
        assert!(refs["workspaces"][1].get("pane_ids").is_none());
        assert_eq!(refs["workspaces"][1]["pane_refs"][0], "pane:6");
        assert!(refs["focused"].get("window_id").is_none());
        assert_eq!(refs["focused"]["window_ref"], "window:7");

        let mut uuids = value.clone();
        format_ids(&mut uuids, IdFormat::Uuids);
        assert_eq!(uuids["id"], "uuid-1");
        assert!(uuids.get("ref").is_none());
        assert_eq!(uuids["workspace_id"], "uuid-2");
        assert!(uuids.get("workspace_ref").is_none());
        assert!(uuids.get("workspace_refs").is_none());
        assert_eq!(uuids["workspace_ids"][0], "uuid-2");
        assert!(uuids["workspaces"][0].get("ref").is_none());
        assert_eq!(uuids["workspaces"][0]["id"], "uuid-5");
        assert!(uuids["workspaces"][1].get("pane_refs").is_none());
        assert_eq!(uuids["focused"]["window_id"], "uuid-7");
        assert_eq!(uuids["title"], "Keep");

        let mut both = value.clone();
        format_ids(&mut both, IdFormat::Both);
        assert_eq!(both, value);
    }

    /// Identify text output adds focused and caller references only when the server sends them.
    #[test]
    fn identify_context_lines_are_optional() {
        let result = json!({
            "version": "0.2.1", "platform": "linux", "pid": 42,
            "focused": {"window_id": "uuid-1", "window_ref": "window:1",
                "workspace_ref": "workspace:2", "pane_id": "uuid-3"},
            "caller": {"workspace_id": "uuid-4"}
        });
        assert_eq!(
            format_response("system.identify", &result, false, false),
            concat!(
                "cmux 0.2.1 (linux) pid 42\n",
                "focused: window:1 workspace:2 uuid-3\n",
                "caller: uuid-4"
            )
        );
        let bare = json!({"version": "0.2.1", "platform": "linux", "pid": 42});
        assert_eq!(
            format_response("system.identify", &bare, false, false),
            "cmux 0.2.1 (linux) pid 42"
        );
        let empty = json!({"version": "0.2.1", "pid": 42, "focused": {}, "caller": ""});
        assert_eq!(
            format_response("system.identify", &empty, false, false),
            "cmux 0.2.1 (unknown) pid 42"
        );
    }
}
