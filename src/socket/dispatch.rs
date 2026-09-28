//! Worker-side request validation, typed command construction and response correlation.
//! Owns no GTK state; admitted commands cross the bounded bridge for UI execution.
use super::{
    commands,
    response::{err, ok},
};

/// Decode a nullable optional target without silently turning malformed IDs into active-pane fallback.
fn optional_target(params: &serde_json::Value) -> Result<Option<String>, &'static str> {
    match params.get("id") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(id)) => Ok(Some(id.clone())),
        _ => Err("id must be a string or null"),
    }
}

/// Parameters shared by the terminal creation methods (`surface.split`, `pane.create`,
/// `surface.create`), with upstream's names.
struct CreationParams {
    workspace: Option<String>,
    /// Surface of the calling terminal: its workspace is the default one.
    caller: Option<String>,
    pane: Option<String>,
    launch: crate::split_engine::TerminalLaunch,
    focus: Option<bool>,
}

/// Read creation parameters. Only terminals are created here: browser panes open through
/// `browser.open`. `initial_input` may end with the Enter upstream's CLI appends; cmux adds its own.
fn creation_params(params: &serde_json::Value) -> Result<CreationParams, &'static str> {
    let text = |key: &str| -> Result<Option<String>, &'static str> {
        match params.get(key) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
            Some(_) => Err("creation parameters must be strings"),
        }
    };
    match text("type")?.as_deref() {
        None | Some("terminal") => {}
        Some("browser") => {
            return Err("browser panes are not created here yet; use `cmux browser open`")
        }
        Some(_) => return Err("type must be terminal"),
    }
    let initial_input = text("initial_input")?
        .map(|input| {
            input
                .strip_suffix('\r')
                .or_else(|| input.strip_suffix('\n'))
                .unwrap_or(&input)
                .to_owned()
        })
        .filter(|input| !input.trim().is_empty());
    let working_directory = match text("working_directory")? {
        Some(path) if !path.starts_with('/') || path.contains('\0') => {
            return Err("working_directory must be an absolute path")
        }
        path => path.map(std::path::PathBuf::from),
    };
    let focus = match params.get("focus") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Bool(focus)) => Some(*focus),
        Some(_) => return Err("focus must be a boolean"),
    };
    Ok(CreationParams {
        workspace: text("workspace_id")?,
        caller: text("caller_surface_id")?,
        pane: text("pane_id")?,
        launch: crate::split_engine::TerminalLaunch {
            initial_input,
            working_directory,
        },
        focus,
    })
}

/// Read an optional string parameter under this fork's name and upstream's alias.
fn optional_text(
    params: &serde_json::Value,
    key: &'static str,
    alias: &'static str,
) -> Result<Option<String>, String> {
    for name in [key, alias] {
        match params.get(name) {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::String(value)) => return Ok(Some(value.clone())),
            Some(_) => return Err(format!("{key} must be a string")),
        }
    }
    Ok(None)
}

/// Refuse a caller-named window: this Linux build owns exactly one GTK window.
fn only_main_window(params: &serde_json::Value) -> Result<(), String> {
    match optional_text(params, "window", "window_id")? {
        Some(window) if window != super::handles::MAIN_WINDOW_ID && window != "window:1" => Err(
            format!("this build has a single window; pass window:1 (got {window})"),
        ),
        _ => Ok(()),
    }
}

/// Read the placement shared by `surface.move`, `surface.reorder` and `workspace.reorder`:
/// at most one of an index or an anchor surface/workspace. Callers decide whether a
/// placement is mandatory (`reorder`) or optional (`move`).
fn placement(
    params: &serde_json::Value,
    index_keys: (&str, &str),
    before_alias: &'static str,
    after_alias: &'static str,
) -> Result<(Option<usize>, Option<String>, Option<String>), String> {
    let raw_index = match params.get(index_keys.0).or_else(|| params.get(index_keys.1)) {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => match value.as_u64().and_then(|value| usize::try_from(value).ok()) {
            Some(value) => Some(value),
            None => return Err(format!("{} must be a non-negative integer", index_keys.0)),
        },
    };
    let before = optional_text(params, "before", before_alias)?;
    let after = optional_text(params, "after", after_alias)?;
    let given = [raw_index.is_some(), before.is_some(), after.is_some()]
        .into_iter()
        .filter(|given| *given)
        .count();
    if given > 1 {
        return Err(format!(
            "only one of {}, before or after may be given",
            index_keys.0
        ));
    }
    Ok((raw_index, before, after))
}

/// The error a reorder issues when neither an index nor an anchor names the new slot.
fn placement_required(index_key: &str) -> String {
    format!("one of {index_key}, before or after is required")
}

/// Map a split direction; `horizontal`/`vertical` are this fork's original right/down names.
fn split_side(value: &str) -> Option<crate::split_engine::FocusDirection> {
    use crate::split_engine::FocusDirection;
    match value.to_ascii_lowercase().as_str() {
        "left" | "l" => Some(FocusDirection::Left),
        "right" | "r" | "horizontal" => Some(FocusDirection::Right),
        "up" | "u" => Some(FocusDirection::Up),
        "down" | "d" | "vertical" => Some(FocusDirection::Down),
        _ => None,
    }
}

/// Publish `agent.hook.<HookEventName>` with operational identifiers only (upstream privacy rules:
/// no prompt or tool input). `surface_id` is the exact surface the hook ran in.
fn agent_hook_event(params: &serde_json::Value) -> Result<(), &'static str> {
    let text = |key: &str| -> Result<Option<String>, &'static str> {
        match params.get(key) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(value))
                if value.len() <= 1024 && !value.chars().any(char::is_control) =>
            {
                Ok(Some(value.clone()))
            }
            Some(_) => Err("hook fields must be short strings"),
        }
    };
    let name = text("hook_event_name")?
        .filter(|name| {
            name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        .ok_or("hook_event_name must be an identifier")?;
    let source = text("source")?.ok_or("source is required")?;
    let surface = text("surface_id")?
        .map(|id| uuid::Uuid::parse_str(&id).map(|id| id.to_string()))
        .transpose()
        .map_err(|_| "surface_id must be a UUID")?;
    let payload = serde_json::json!({
        "session_id": text("session_id")?, "hook_event_name": name, "_source": source,
        "surface_id": surface, "tool_name": text("tool_name")?, "phase": "received",
    });
    crate::events::publish(
        &format!("agent.hook.{name}"),
        &source,
        crate::events::Scope {
            surface,
            ..Default::default()
        },
        payload,
    );
    Ok(())
}

/// Parse a JSON-RPC line and dispatch to the appropriate SocketCommand.
/// Consumes raw input and releases unused JSON fields before awaiting execution.
/// Returns encoded JSON and its validated operation identity for transport diagnostics.
pub(super) async fn dispatch_line(
    line: String,
    cmd_tx: &tokio::sync::mpsc::Sender<commands::SocketCommand>,
) -> DispatchedResponse {
    let mut operation = None;
    let response = dispatch_request(line, cmd_tx, &mut operation).await;
    let body = super::response::encode(response, operation.as_mut());
    DispatchedResponse {
        body,
        trace_id: operation.as_ref().map(|operation| operation.id),
    }
}

/// Carry correlation across encoding without retaining request contents or operation accounting.
pub(super) struct DispatchedResponse {
    pub body: String,
    pub trace_id: Option<uuid::Uuid>,
}

/// Validate and execute one request; the caller retains its operation through response encoding.
async fn dispatch_request(
    line: String,
    cmd_tx: &tokio::sync::mpsc::Sender<commands::SocketCommand>,
    operation: &mut Option<crate::diagnostics::Operation>,
) -> serde_json::Value {
    let mut req: serde_json::Value = match serde_json::from_str(&line) {
        Ok(v) => v,
        Err(_) => {
            return err(serde_json::Value::Null, "parse_error", "invalid JSON");
        }
    };

    drop(line);
    if !req.is_object() {
        return err(
            serde_json::Value::Null,
            "invalid_request",
            "request must be an object",
        );
    }
    let req_id = req
        .get_mut("id")
        .map(serde_json::Value::take)
        .unwrap_or(serde_json::Value::Null);
    let method = match req.get_mut("method").map(serde_json::Value::take) {
        Some(serde_json::Value::String(method)) => method,
        _ => return err(req_id, "invalid_request", "method must be a string"),
    };
    let mut params = req
        .get_mut("params")
        .map(serde_json::Value::take)
        .unwrap_or(serde_json::Value::Object(Default::default()));

    let operation = operation.insert(crate::diagnostics::Operation::begin(
        &method,
        req.get("trace_id").and_then(|id| id.as_str()),
    ));
    drop(req);
    // Early validation failures below are errors; cancellation while awaiting
    // execution is reset explicitly before yielding to the response channel.
    operation.finish(false);

    if params.is_null() {
        params = serde_json::json!({});
    } else if !params.is_object() {
        return err(req_id, "invalid_params", "params must be an object or null");
    }

    // Agent hook bridges report here; the event needs no GTK state, only the process-wide bus.
    if method == "events.agent_hook" {
        return match agent_hook_event(&params) {
            Ok(()) => {
                operation.finish(true);
                ok(req_id, serde_json::json!({"published": true}))
            }
            Err(message) => err(req_id, "invalid_params", message),
        };
    }

    if method == "system.diagnostics" {
        drop(params);
        operation.pending();
        return match tokio::task::spawn_blocking(crate::diagnostics::snapshot).await {
            Ok(snapshot) => {
                operation.finish(true);
                ok(req_id, snapshot)
            }
            Err(_) => {
                operation.finish(false);
                err(req_id, "internal_error", "diagnostic sampling failed")
            }
        };
    }

    // Short refs (`workspace:N`…) resolve on GTK against the live topology, then decode as UUIDs.
    let refs = super::handles::refs_in(&mut params);
    if !refs.is_empty() {
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        let resolve = commands::SocketCommand::ResolveHandles {
            req_id: req_id.clone(),
            refs,
            resp_tx,
        };
        if cmd_tx.try_send(resolve).is_err() {
            return err(req_id, "overloaded", "GTK command queue is full");
        }
        let response = resp_rx
            .await
            .unwrap_or_else(|_| err(req_id.clone(), "internal_error", "handler dropped response"));
        let Some(resolved) = response.get("result").and_then(|result| {
            serde_json::from_value::<std::collections::HashMap<String, String>>(result.clone()).ok()
        }) else {
            return response;
        };
        super::handles::rewrite(&mut params, &resolved);
    }

    let target = if matches!(
        method.as_str(),
        "surface.split"
            | "surface.send_text"
            | "surface.send_key"
            | "surface.read_text"
            | "surface.read_scrollback"
            | "surface.health"
            | "surface.refresh"
            | "pane.focus"
    ) {
        match optional_target(&params) {
            Ok(target) => target,
            Err(message) => return err(req_id, "invalid_params", message),
        }
    } else {
        None
    };

    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();

    let cmd = match method.as_str() {
        "surface.resume.set" | "surface.resume.show" | "surface.resume.clear" => {
            let id = match params.get("surface_id").or_else(|| params.get("id")) {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(id)) if uuid::Uuid::parse_str(id).is_ok() => {
                    Some(id.clone())
                }
                _ => return err(req_id, "invalid_params", "surface_id must be a UUID"),
            };
            let action = match method.as_str() {
                "surface.resume.set" => {
                    if params
                        .get("auto_resume")
                        .is_some_and(|value| value != &serde_json::Value::Bool(false))
                    {
                        return err(
                            req_id,
                            "not_supported",
                            "automatic resume requires a configured hook policy",
                        );
                    }
                    let mut binding =
                        match serde_json::from_value::<crate::resume::ResumeBinding>(params.take())
                        {
                            Ok(binding) => binding,
                            Err(_) => {
                                return err(
                                    req_id,
                                    "invalid_params",
                                    "invalid resume binding fields",
                                )
                            }
                        };
                    if let Err(message) = binding.validate() {
                        return err(req_id, "invalid_params", message);
                    }
                    binding.sanitize_environment();
                    crate::resume::ResumeAction::Set(binding)
                }
                "surface.resume.clear" => {
                    let checkpoint_id = match params.get("checkpoint_id") {
                        None | Some(serde_json::Value::Null) => None,
                        Some(serde_json::Value::String(value))
                            if value.len() <= 16384 && !value.contains('\0') =>
                        {
                            Some(value.clone())
                        }
                        _ => return err(req_id, "invalid_params", "invalid checkpoint_id"),
                    };
                    crate::resume::ResumeAction::Clear { checkpoint_id }
                }
                _ => crate::resume::ResumeAction::Show,
            };
            commands::SocketCommand::SurfaceResume {
                req_id: req_id.clone(),
                id,
                action,
                resp_tx,
            }
        }
        "system.ping" => commands::SocketCommand::Ping {
            req_id: req_id.clone(),
            resp_tx,
        },
        "system.identify" => commands::SocketCommand::Identify {
            req_id: req_id.clone(),
            caller: params
                .get_mut("caller")
                .map(serde_json::Value::take)
                .filter(|caller| caller.is_object()),
            resp_tx,
        },
        "system.capabilities" => commands::SocketCommand::Capabilities {
            req_id: req_id.clone(),
            resp_tx,
        },

        "workspace.list" => commands::SocketCommand::WorkspaceList {
            req_id: req_id.clone(),
            resp_tx,
        },
        "workspace.current" => commands::SocketCommand::WorkspaceCurrent {
            req_id: req_id.clone(),
            resp_tx,
        },
        "workspace.create" => {
            let remote_target = params
                .get("remote_target")
                .and_then(|v| v.as_str())
                .map(String::from);
            let mut name = params
                .get("name")
                .and_then(|v| v.as_str())
                .map(String::from);
            let working_directory = params
                .get("working_directory")
                .or_else(|| params.get("cwd"))
                .and_then(|v| v.as_str());
            let working_directory = if remote_target.is_none() {
                match working_directory {
                    Some(path) => match crate::workspace::prepare_local_workspace(
                        name.as_deref().unwrap_or(""),
                        std::path::Path::new(path),
                    ) {
                        Ok((prepared_name, path)) => {
                            name = Some(prepared_name);
                            Some(path)
                        }
                        Err(message) => {
                            return err(req_id, "invalid_directory", &message);
                        }
                    },
                    None => None,
                }
            } else {
                None
            };
            let remote_directory = params
                .get("remote_directory")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            if remote_directory
                .as_deref()
                .is_some_and(|value| !value.starts_with('/') || value.contains('\0'))
            {
                return err(
                    req_id,
                    "invalid_params",
                    "remote_directory must be an absolute path",
                );
            }
            let terminal_transport = match params
                .get("terminal_transport")
                .and_then(|value| value.as_str())
                .unwrap_or("ssh")
            {
                "ssh" => crate::remote_transport::TerminalTransport::Ssh,
                "mosh" if remote_target.is_some() => {
                    crate::remote_transport::TerminalTransport::Mosh
                }
                "mosh" => {
                    return err(
                        req_id,
                        "invalid_params",
                        "Mosh terminal transport requires an SSH remote workspace",
                    )
                }
                _ => {
                    return err(
                        req_id,
                        "invalid_params",
                        "terminal_transport must be ssh or mosh",
                    )
                }
            };
            let terminal_profile = match params
                .get("terminal_profile")
                .and_then(|value| value.as_str())
                .unwrap_or("shell")
            {
                "shell" => crate::remote_transport::TerminalProfile::Shell,
                "tmux" => crate::remote_transport::TerminalProfile::Tmux,
                _ => {
                    return err(
                        req_id,
                        "invalid_params",
                        "terminal_profile must be shell or tmux",
                    )
                }
            };
            let mut terminal_tmux_session = params
                .get("terminal_tmux_session")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            if matches!(
                terminal_profile,
                crate::remote_transport::TerminalProfile::Tmux
            ) && crate::remote_transport::validate_tmux_session(
                terminal_tmux_session.as_deref().unwrap_or("main"),
            )
            .is_err()
            {
                return err(req_id, "invalid_params", "invalid tmux session name");
            }
            if matches!(
                terminal_profile,
                crate::remote_transport::TerminalProfile::Tmux
            ) && terminal_transport != crate::remote_transport::TerminalTransport::Mosh
            {
                return err(
                    req_id,
                    "invalid_params",
                    "tmux terminal profile requires Mosh transport",
                );
            }
            if matches!(
                terminal_profile,
                crate::remote_transport::TerminalProfile::Tmux
            ) {
                terminal_tmux_session.get_or_insert_with(|| "main".into());
            } else if terminal_tmux_session.is_some() {
                return err(
                    req_id,
                    "invalid_params",
                    "terminal_tmux_session requires the tmux profile",
                );
            }
            commands::SocketCommand::WorkspaceCreate {
                req_id: req_id.clone(),
                remote_target,
                name,
                working_directory,
                remote_directory,
                terminal_transport,
                terminal_profile,
                terminal_tmux_session,
                initial_input: match params.get("initial_input") {
                    None | Some(serde_json::Value::Null) => None,
                    Some(serde_json::Value::String(input)) => Some(
                        input
                            .strip_suffix('\r')
                            .or_else(|| input.strip_suffix('\n'))
                            .unwrap_or(input)
                            .to_owned(),
                    )
                    .filter(|input| !input.trim().is_empty()),
                    Some(_) => {
                        return err(req_id, "invalid_params", "initial_input must be a string")
                    }
                },
                resp_tx,
            }
        }
        "workspace.select" => commands::SocketCommand::WorkspaceSelect {
            req_id: req_id.clone(),
            id: params
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            resp_tx,
        },
        "workspace.close" => commands::SocketCommand::WorkspaceClose {
            req_id: req_id.clone(),
            id: params
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            resp_tx,
        },
        "workspace.rename" => commands::SocketCommand::WorkspaceRename {
            req_id: req_id.clone(),
            id: params
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            name: params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            resp_tx,
        },
        "workspace.set_description" | "workspace.clear_description" => {
            commands::SocketCommand::WorkspaceDescription {
                req_id: req_id.clone(),
                id: params
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                description: params
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                resp_tx,
            }
        }
        "workspace.next" => commands::SocketCommand::WorkspaceNext {
            req_id: req_id.clone(),
            resp_tx,
        },
        "workspace.previous" => commands::SocketCommand::WorkspacePrev {
            req_id: req_id.clone(),
            resp_tx,
        },
        "workspace.last" => commands::SocketCommand::WorkspaceLast {
            req_id: req_id.clone(),
            resp_tx,
        },
        "workspace.reorder_many" => {
            let Some(values) = params
                .get("workspace_ids")
                .or_else(|| params.get("order"))
                .and_then(|value| value.as_array())
                .filter(|values| !values.is_empty() && values.len() <= 4096)
            else {
                return err(
                    req_id,
                    "invalid_params",
                    "workspace_ids must contain 1..4096 UUIDs",
                );
            };
            let Some(order) = values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .and_then(|value| uuid::Uuid::parse_str(value).ok())
                })
                .collect::<Option<Vec<_>>>()
            else {
                return err(req_id, "invalid_params", "invalid workspace UUID");
            };
            let dry_run = match params.get("dry_run") {
                None => false,
                Some(value) => match value.as_bool() {
                    Some(value) => value,
                    None => return err(req_id, "invalid_params", "dry_run must be boolean"),
                },
            };
            commands::SocketCommand::WorkspaceReorderMany {
                req_id: req_id.clone(),
                order,
                dry_run,
                resp_tx,
            }
        }
        "workspace.reorder" => {
            let Some(id) = params
                .get("id")
                .or_else(|| params.get("workspace_id"))
                .and_then(|value| value.as_str())
            else {
                return err(req_id, "invalid_params", "id must be a workspace UUID");
            };
            let (position, before, after) = match placement(
                &params,
                ("position", "index"),
                "before_workspace_id",
                "after_workspace_id",
            ) {
                Ok(placement) => placement,
                Err(message) => return err(req_id, "invalid_params", &message),
            };
            if position.is_none() && before.is_none() && after.is_none() {
                return err(req_id, "invalid_params", &placement_required("position"));
            }
            if let Err(message) = only_main_window(&params) {
                return err(req_id, "invalid_params", &message);
            }
            let dry_run = match params.get("dry_run") {
                None | Some(serde_json::Value::Null) => false,
                Some(serde_json::Value::Bool(value)) => *value,
                Some(_) => return err(req_id, "invalid_params", "dry_run must be boolean"),
            };
            commands::SocketCommand::WorkspaceReorder {
                req_id: req_id.clone(),
                id: id.to_owned(),
                position,
                before,
                after,
                dry_run,
                resp_tx,
            }
        }
        "workspace.action" => {
            let Some(action) = params.get("action").and_then(serde_json::Value::as_str) else {
                return err(req_id, "invalid_params", "action must be a string");
            };
            let (title, color, description) = match (
                optional_text(&params, "title", "title"),
                optional_text(&params, "color", "color"),
                optional_text(&params, "description", "description"),
            ) {
                (Ok(title), Ok(color), Ok(description)) => (title, color, description),
                (Err(message), _, _) | (_, Err(message), _) | (_, _, Err(message)) => {
                    return err(req_id, "invalid_params", &message)
                }
            };
            let workspace = match optional_text(&params, "workspace_id", "workspace_id") {
                Ok(workspace) => workspace,
                Err(message) => return err(req_id, "invalid_params", &message),
            };
            if let Err(message) = only_main_window(&params) {
                return err(req_id, "invalid_params", &message);
            }
            commands::SocketCommand::WorkspaceAction {
                req_id: req_id.clone(),
                action: action.to_owned(),
                workspace,
                title,
                color,
                description,
                resp_tx,
            }
        }
        "workspace.group.list" => commands::SocketCommand::WorkspaceGroupList {
            req_id: req_id.clone(),
            resp_tx,
        },
        "workspace.group.create" => commands::SocketCommand::WorkspaceGroupCreate {
            req_id: req_id.clone(),
            name: params
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string(),
            color: match params.get("color") {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(value)) => Some(value.clone()),
                _ => return err(req_id, "invalid_params", "color must be a string or null"),
            },
            resp_tx,
        },
        "workspace.group.update" => {
            let Some(id) = params
                .get("id")
                .and_then(|value| value.as_str())
                .and_then(|value| uuid::Uuid::parse_str(value).ok())
            else {
                return err(req_id, "invalid_params", "invalid group UUID");
            };
            let name = match params.get("name") {
                None => None,
                Some(serde_json::Value::String(value)) => Some(value.clone()),
                _ => return err(req_id, "invalid_params", "name must be a string"),
            };
            let color = match params.get("color") {
                None => None,
                Some(serde_json::Value::Null) => Some(None),
                Some(serde_json::Value::String(value)) => Some(Some(value.clone())),
                _ => return err(req_id, "invalid_params", "color must be a string or null"),
            };
            let collapsed = match params.get("collapsed") {
                None => None,
                Some(value) => match value.as_bool() {
                    Some(value) => Some(value),
                    None => return err(req_id, "invalid_params", "collapsed must be boolean"),
                },
            };
            let position = match params.get("position") {
                None => None,
                Some(value) => match value.as_u64().and_then(|value| usize::try_from(value).ok()) {
                    Some(value) => Some(value),
                    None => {
                        return err(
                            req_id,
                            "invalid_params",
                            "position must be a nonnegative integer",
                        )
                    }
                },
            };
            commands::SocketCommand::WorkspaceGroupUpdate {
                req_id: req_id.clone(),
                id,
                name,
                color,
                collapsed,
                position,
                resp_tx,
            }
        }
        "workspace.group.assign" => {
            let id = match params.get("id") {
                None | Some(serde_json::Value::Null) => None,
                Some(value) => match value
                    .as_str()
                    .and_then(|value| uuid::Uuid::parse_str(value).ok())
                {
                    Some(id) => Some(id),
                    None => return err(req_id, "invalid_params", "invalid group UUID"),
                },
            };
            let Some(values) = params
                .get("workspace_ids")
                .and_then(|value| value.as_array())
                .filter(|values| !values.is_empty() && values.len() <= 4096)
            else {
                return err(
                    req_id,
                    "invalid_params",
                    "workspace_ids must contain 1..4096 UUIDs",
                );
            };
            let Some(workspaces) = values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .and_then(|value| uuid::Uuid::parse_str(value).ok())
                })
                .collect::<Option<Vec<_>>>()
            else {
                return err(req_id, "invalid_params", "invalid workspace UUID");
            };
            commands::SocketCommand::WorkspaceGroupAssign {
                req_id: req_id.clone(),
                id,
                workspaces,
                resp_tx,
            }
        }
        "workspace.group.delete" => {
            let Some(id) = params
                .get("id")
                .and_then(|value| value.as_str())
                .and_then(|value| uuid::Uuid::parse_str(value).ok())
            else {
                return err(req_id, "invalid_params", "invalid group UUID");
            };
            commands::SocketCommand::WorkspaceGroupDelete {
                req_id: req_id.clone(),
                id,
                resp_tx,
            }
        }

        "surface.list" => {
            let workspace = match params.get("workspace_id").filter(|value| !value.is_null()) {
                None => None,
                Some(value) => match value
                    .as_str()
                    .and_then(|value| uuid::Uuid::parse_str(value).ok())
                {
                    Some(id) => Some(id),
                    None => return err(req_id, "invalid_params", "invalid workspace UUID"),
                },
            };
            commands::SocketCommand::SurfaceList {
                req_id: req_id.clone(),
                workspace,
                resp_tx,
            }
        }
        "surface.split" | "pane.create" => {
            let creation = match creation_params(&params) {
                Ok(creation) => creation,
                Err(message) => return err(req_id, "invalid_params", message),
            };
            let direction = match params.get("direction") {
                // `surface.split` has always defaulted to a right split, as `pane.create` does upstream.
                None => crate::split_engine::FocusDirection::Right,
                Some(serde_json::Value::String(value)) => match split_side(value) {
                    Some(direction) => direction,
                    None => {
                        return err(
                            req_id,
                            "invalid_params",
                            "direction must be left, right, up, down, horizontal or vertical",
                        )
                    }
                },
                _ => return err(req_id, "invalid_params", "direction must be a string"),
            };
            commands::SocketCommand::SurfaceSplit {
                req_id: req_id.clone(),
                // Upstream's CLI names the split surface `surface_id`; this fork's used `id`.
                id: target.or_else(|| {
                    params
                        .get("surface_id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                }),
                workspace: creation.workspace,
                caller: creation.caller,
                pane: creation.pane,
                direction,
                launch: creation.launch,
                // `surface.split` kept focus before `focus` existed; upstream's CLI sends it.
                focus: creation.focus.unwrap_or(method == "surface.split"),
                resp_tx,
            }
        }
        "surface.create" => {
            let creation = match creation_params(&params) {
                Ok(creation) => creation,
                Err(message) => return err(req_id, "invalid_params", message),
            };
            commands::SocketCommand::SurfaceCreate {
                req_id: req_id.clone(),
                workspace: creation.workspace,
                caller: creation.caller,
                pane: creation.pane,
                launch: creation.launch,
                focus: creation.focus.unwrap_or(false),
                resp_tx,
            }
        }
        "surface.focus" => commands::SocketCommand::SurfaceFocus {
            req_id: req_id.clone(),
            id: params
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            resp_tx,
        },
        "surface.close" => commands::SocketCommand::SurfaceClose {
            req_id: req_id.clone(),
            id: params
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            resp_tx,
        },
        "surface.move" => {
            let Some(id) = params
                .get("id")
                .or_else(|| params.get("surface_id"))
                .and_then(serde_json::Value::as_str)
            else {
                return err(req_id, "invalid_params", "id must be a surface UUID");
            };
            let pane = match params.get("pane").or_else(|| params.get("pane_id")) {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(value)) => Some(value.to_owned()),
                Some(_) => return err(req_id, "invalid_params", "pane must be a pane reference"),
            };
            let workspace = match params.get("workspace").or_else(|| params.get("workspace_id")) {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(value)) => Some(value.to_owned()),
                Some(_) => return err(req_id, "invalid_params", "workspace must be a UUID"),
            };
            // Upstream names the insertion slot `index`; this fork has always read `position`.
            let (position, before, after) =
                match placement(&params, ("position", "index"), "before_surface_id", "after_surface_id")
                {
                    Ok(placement) => placement,
                    Err(message) => return err(req_id, "invalid_params", &message),
                };
            if let Err(message) = only_main_window(&params) {
                return err(req_id, "invalid_params", &message);
            }
            commands::SocketCommand::SurfaceMove {
                req_id: req_id.clone(),
                id: id.to_owned(),
                workspace,
                pane,
                position,
                before,
                after,
                focus: params
                    .get("focus")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true),
                resp_tx,
            }
        }
        "surface.reorder" => {
            let Some(id) = params
                .get("id")
                .or_else(|| params.get("surface_id"))
                .and_then(|value| value.as_str())
            else {
                return err(req_id, "invalid_params", "id must be a surface UUID");
            };
            let (position, before, after) = match placement(
                &params,
                ("position", "index"),
                "before_surface_id",
                "after_surface_id",
            ) {
                Ok(placement) => placement,
                Err(message) => return err(req_id, "invalid_params", &message),
            };
            if position.is_none() && before.is_none() && after.is_none() {
                return err(req_id, "invalid_params", &placement_required("position"));
            }
            if let Err(message) = only_main_window(&params) {
                return err(req_id, "invalid_params", &message);
            }
            // Reordering keeps the current selection unless the caller asks for focus.
            let focus = match params.get("focus") {
                None | Some(serde_json::Value::Null) => false,
                Some(serde_json::Value::Bool(value)) => *value,
                Some(_) => return err(req_id, "invalid_params", "focus must be a boolean"),
            };
            commands::SocketCommand::SurfaceReorder {
                req_id: req_id.clone(),
                id: id.to_owned(),
                position,
                before,
                after,
                focus,
                resp_tx,
            }
        }
        "tab.action" => {
            let Some(action) = params.get("action").and_then(serde_json::Value::as_str) else {
                return err(req_id, "invalid_params", "action must be a string");
            };
            let surface = match optional_text(&params, "surface_id", "surface_id") {
                Ok(surface) => surface,
                Err(message) => return err(req_id, "invalid_params", &message),
            };
            let workspace = match optional_text(&params, "workspace_id", "workspace_id") {
                Ok(workspace) => workspace,
                Err(message) => return err(req_id, "invalid_params", &message),
            };
            if let Err(message) = only_main_window(&params) {
                return err(req_id, "invalid_params", &message);
            }
            // `url` only reaches the upstream actions this build rejects first.
            let title = match optional_text(&params, "title", "title") {
                Ok(title) => title,
                Err(message) => return err(req_id, "invalid_params", &message),
            };
            let focus = match params.get("focus") {
                None | Some(serde_json::Value::Null) => false,
                Some(serde_json::Value::Bool(focus)) => *focus,
                Some(_) => return err(req_id, "invalid_params", "focus must be a boolean"),
            };
            commands::SocketCommand::TabAction {
                req_id: req_id.clone(),
                action: action.to_owned(),
                surface,
                workspace,
                title,
                focus,
                resp_tx,
            }
        }
"surface.drag_to_split" => {
            let Some(id) = params.get("id").and_then(serde_json::Value::as_str) else {
                return err(req_id, "invalid_params", "id must be a surface UUID");
            };
            let Some(target_pane) = params.get("pane").and_then(serde_json::Value::as_str) else {
                return err(req_id, "invalid_params", "pane must be a pane reference");
            };
            let direction = match params.get("direction").and_then(serde_json::Value::as_str) {
                Some("left") => crate::split_engine::FocusDirection::Left,
                Some("right") => crate::split_engine::FocusDirection::Right,
                Some("up") => crate::split_engine::FocusDirection::Up,
                Some("down") => crate::split_engine::FocusDirection::Down,
                _ => {
                    return err(
                        req_id,
                        "invalid_params",
                        "direction must be left, right, up, or down",
                    )
                }
            };
            commands::SocketCommand::SurfaceDragToSplit {
                req_id: req_id.clone(),
                id: id.to_owned(),
                target_pane: target_pane.to_owned(),
                direction,
                resp_tx,
            }
        }
        "surface.split_off" => {
            let Some(id) = params
                .get("surface_id")
                .or_else(|| params.get("id"))
                .and_then(|value| value.as_str())
            else {
                return err(req_id, "invalid_params", "surface_id must be a surface UUID");
            };
            let direction = match params.get("direction").and_then(|value| value.as_str()) {
                Some("left") => crate::split_engine::FocusDirection::Left,
                Some("right") => crate::split_engine::FocusDirection::Right,
                Some("up") => crate::split_engine::FocusDirection::Up,
                Some("down") => crate::split_engine::FocusDirection::Down,
                _ => {
                    return err(
                        req_id,
                        "invalid_params",
                        "direction must be left, right, up, or down",
                    )
                }
            };
            let focus = match params.get("focus") {
                None | Some(serde_json::Value::Null) => false,
                Some(serde_json::Value::Bool(focus)) => *focus,
                Some(_) => return err(req_id, "invalid_params", "focus must be a boolean"),
            };
            if let Err(message) = only_main_window(&params) {
                return err(req_id, "invalid_params", &message);
            }
            commands::SocketCommand::SurfaceSplitOff {
                req_id: req_id.clone(),
                id: id.to_owned(),
                direction,
                focus,
                resp_tx,
            }
        }
        "surface.trigger_flash" => {
            let surface = match optional_text(&params, "surface_id", "surface_id") {
                Ok(surface) => surface,
                Err(message) => return err(req_id, "invalid_params", &message),
            };
            let workspace = match optional_text(&params, "workspace_id", "workspace_id") {
                Ok(workspace) => workspace,
                Err(message) => return err(req_id, "invalid_params", &message),
            };
            if let Err(message) = only_main_window(&params) {
                return err(req_id, "invalid_params", &message);
            }
            commands::SocketCommand::SurfaceTriggerFlash {
                req_id: req_id.clone(),
                surface,
                workspace,
                resp_tx,
            }
        }
        "surface.send_text" | "surface.send_key" | "debug.type" => {
            let field = if method == "surface.send_key" {
                "key"
            } else {
                "text"
            };
            let Some(input) = params.get(field).and_then(serde_json::Value::as_str) else {
                return err(
                    req_id,
                    "invalid_params",
                    &format!("{field} must be a string"),
                );
            };
            let input = input.to_owned();
            match method.as_str() {
                "surface.send_text" => commands::SocketCommand::SurfaceSendText {
                    req_id: req_id.clone(),
                    id: target,
                    text: input,
                    resp_tx,
                },
                "surface.send_key" => commands::SocketCommand::SurfaceSendKey {
                    req_id: req_id.clone(),
                    id: target,
                    key: input,
                    resp_tx,
                },
                _ => commands::SocketCommand::DebugType {
                    req_id: req_id.clone(),
                    text: input,
                    resp_tx,
                },
            }
        }
        "surface.read_text" | "surface.read_scrollback" => {
            commands::SocketCommand::SurfaceReadText {
                scrollback: method == "surface.read_scrollback",
                req_id: req_id.clone(),
                id: target,
                resp_tx,
            }
        }
        "surface.health" => {
            let workspace = match optional_text(&params, "workspace_id", "workspace_id") {
                Ok(workspace) => workspace,
                Err(message) => return err(req_id, "invalid_params", &message),
            };
            if target.is_some() && workspace.is_some() {
                return err(
                    req_id,
                    "invalid_params",
                    "pass either a surface id or a workspace, not both",
                );
            }
            if let Err(message) = only_main_window(&params) {
                return err(req_id, "invalid_params", &message);
            }
            commands::SocketCommand::SurfaceHealth {
                req_id: req_id.clone(),
                id: target,
                workspace,
                resp_tx,
            }
        }
        "surface.refresh" => commands::SocketCommand::SurfaceRefresh {
            req_id: req_id.clone(),
            id: target,
            resp_tx,
        },

        "pane.list" => {
            let workspace = match params.get("workspace_id").filter(|value| !value.is_null()) {
                None => None,
                Some(value) => match value
                    .as_str()
                    .and_then(|value| uuid::Uuid::parse_str(value).ok())
                {
                    Some(id) => Some(id),
                    None => return err(req_id, "invalid_params", "invalid workspace UUID"),
                },
            };
            commands::SocketCommand::PaneList {
                req_id: req_id.clone(),
                workspace,
                resp_tx,
            }
        }
        "pane.focus" => commands::SocketCommand::PaneFocus {
            req_id: req_id.clone(),
            id: target,
            resp_tx,
        },
        "pane.last" => commands::SocketCommand::PaneLast {
            req_id: req_id.clone(),
            resp_tx,
        },

        "window.list" => commands::SocketCommand::WindowList {
            req_id: req_id.clone(),
            resp_tx,
        },
        "window.current" => commands::SocketCommand::WindowCurrent {
            req_id: req_id.clone(),
            resp_tx,
        },

        "debug.layout" => commands::SocketCommand::DebugLayout {
            req_id: req_id.clone(),
            resp_tx,
        },

        "project.actions.run" => {
            let workspace = match params.get("workspace_id").filter(|value| !value.is_null()) {
                None => None,
                Some(value) => match value
                    .as_str()
                    .and_then(|value| uuid::Uuid::parse_str(value).ok())
                {
                    Some(id) => Some(id),
                    None => return err(req_id, "invalid_params", "invalid workspace UUID"),
                },
            };
            let Some(action_id) = params
                .get("action_id")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty() && value.len() <= 128)
            else {
                return err(req_id, "invalid_params", "invalid action ID");
            };
            let Some(fingerprint) = params
                .get("fingerprint")
                .and_then(serde_json::Value::as_str)
                .filter(|value| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
            else {
                return err(req_id, "invalid_params", "review fingerprint required");
            };
            let confirmed = match params.get("confirmed").filter(|value| !value.is_null()) {
                None => false,
                Some(serde_json::Value::Bool(value)) => *value,
                Some(_) => return err(req_id, "invalid_params", "confirmed must be boolean"),
            };
            commands::SocketCommand::ProjectActionRun {
                req_id: req_id.clone(),
                workspace,
                action_id: action_id.into(),
                fingerprint: fingerprint.into(),
                confirmed,
                resp_tx,
            }
        }
        "project.actions.list" => {
            let workspace = match params.get("workspace_id").filter(|value| !value.is_null()) {
                None => None,
                Some(value) => match value
                    .as_str()
                    .and_then(|value| uuid::Uuid::parse_str(value).ok())
                {
                    Some(id) => Some(id),
                    None => return err(req_id, "invalid_params", "invalid workspace UUID"),
                },
            };
            commands::SocketCommand::ProjectActionsList {
                req_id: req_id.clone(),
                workspace,
                resp_tx,
            }
        }
        "ports.list" => {
            let parse = |name: &str| -> Result<Option<uuid::Uuid>, &'static str> {
                params
                    .get(name)
                    .filter(|value| !value.is_null())
                    .map(|value| {
                        value
                            .as_str()
                            .and_then(|id| uuid::Uuid::parse_str(id).ok())
                            .ok_or("invalid scope UUID")
                    })
                    .transpose()
            };
            let workspace = match parse("workspace_id") {
                Ok(id) => id,
                Err(message) => return err(req_id, "invalid_params", message),
            };
            let surface = match parse("surface_id") {
                Ok(id) => id,
                Err(message) => return err(req_id, "invalid_params", message),
            };
            commands::SocketCommand::PortsList {
                req_id: req_id.clone(),
                workspace,
                surface,
                resp_tx,
            }
        }
        "sidebar.metadata"
        | "sidebar.set_status"
        | "sidebar.clear_status"
        | "sidebar.set_progress"
        | "sidebar.clear_progress"
        | "sidebar.report_meta_block"
        | "sidebar.clear_meta_block"
        | "sidebar.log"
        | "sidebar.clear_log"
        | "sidebar.state" => {
            let workspace = match params.get("workspace_id").filter(|value| !value.is_null()) {
                Some(value) => match value.as_str().and_then(|id| uuid::Uuid::parse_str(id).ok()) {
                    Some(id) => Some(id),
                    None => return err(req_id, "invalid_params", "invalid workspace UUID"),
                },
                None => None,
            };
            let action = match crate::workspace_metadata::parse(&method, &params) {
                Ok(action) => action,
                Err(message) => return err(req_id, "invalid_params", message),
            };
            commands::SocketCommand::WorkspaceMetadata {
                req_id: req_id.clone(),
                workspace,
                action,
                resp_tx,
            }
        }
        "notification.list" => commands::SocketCommand::NotificationList {
            req_id: req_id.clone(),
            resp_tx,
        },
        "notification.create"
        | "notification.create_for_surface"
        | "notification.create_for_caller"
        | "notification.create_for_target"
        | "notification.clear"
        | "notification.mark_read"
        | "notification.dismiss"
        | "notification.open"
        | "notification.jump_to_unread" => {
            let action = match crate::inbox::parse(&method, &params) {
                Ok(action) => action,
                Err(message) => return err(req_id, "invalid_params", message),
            };
            commands::SocketCommand::Inbox {
                req_id: req_id.clone(),
                action,
                resp_tx,
            }
        }

        "browser.open" => {
            let workspace = match params.get("workspace").filter(|value| !value.is_null()) {
                Some(value) => match value
                    .as_str()
                    .filter(|value| uuid::Uuid::parse_str(value).is_ok())
                {
                    Some(value) => Some(value.to_owned()),
                    None => return err(req_id, "invalid_params", "invalid workspace UUID"),
                },
                None => None,
            };
            commands::SocketCommand::BrowserOpen {
                req_id: req_id.clone(),
                url: params
                    .get("url")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_owned(),
                workspace,
                profile: match params.get("profile").filter(|value| !value.is_null()) {
                    Some(value) => {
                        match value.as_str().and_then(crate::browser::profile_selector) {
                            Some(value) => Some(value),
                            None => {
                                return err(
                                    req_id,
                                    "invalid_params",
                                    "invalid browser profile selector",
                                )
                            }
                        }
                    }
                    None => None,
                },
                resp_tx,
            }
        }
        "browser.stream.enable" => commands::SocketCommand::BrowserStreamEnable {
            req_id: req_id.clone(),
            resp_tx,
        },
        "browser.stream.disable" => commands::SocketCommand::BrowserStreamDisable {
            req_id: req_id.clone(),
            resp_tx,
        },
        "browser.list" => commands::SocketCommand::BrowserList {
            req_id: req_id.clone(),
            resp_tx,
        },

        // Route all other browser.* methods to the generic proxy (P0/P1 parity)
        _ if method.starts_with("browser.") => {
            let action = method.strip_prefix("browser.").unwrap().to_string();
            let surface_ref = params
                .get("surface_ref")
                .or_else(|| params.get("surface_id"))
                .and_then(|v| v.as_str())
                .map(String::from);
            commands::SocketCommand::BrowserAction {
                req_id: req_id.clone(),
                action,
                params: params.take(),
                surface_ref,
                resp_tx,
            }
        }

        _ => {
            return err(
                req_id,
                "not_implemented",
                &format!("{method} is not implemented"),
            )
        }
    };

    drop(params);
    drop(method);
    let observed = commands::SocketCommand::Observed {
        command: Box::new(cmd),
        trace_id: operation.id,
        queued_at: std::time::Instant::now(),
    };
    if let Err(error) = cmd_tx.try_send(observed) {
        let (code, message) = match error {
            tokio::sync::mpsc::error::TrySendError::Full(_) => {
                ("overloaded", "GTK command queue is full")
            }
            tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                ("internal_error", "handler channel closed")
            }
        };
        crate::diagnostics::record(
            "rpc.queue.rejected",
            serde_json::json!({
                "trace_id": operation.id, "code": code, "capacity": cmd_tx.max_capacity(),
            }),
        );
        return err(req_id, code, message);
    }

    operation.pending();
    let response = resp_rx
        .await
        .unwrap_or_else(|_| err(req_id, "internal_error", "handler dropped response"));
    operation.finish(
        response
            .get("ok")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
    );
    response
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod tests;
