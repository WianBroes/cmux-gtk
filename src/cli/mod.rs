//! cmux CLI — clap-based argument parser and command dispatch.
//!
//! This module is entirely independent of GTK4 and the GUI app.
//! It connects to the cmux-app via Unix socket JSON-RPC.

#[path = "../bounded_json.rs"]
mod bounded_json;
#[path = "../browser_address.rs"]
mod browser_address;
mod comments;
mod events;
use cmux_platform::discovery;
#[cfg(test)]
#[path = "../agent_resume.rs"]
mod agent_resume;
pub mod format;
mod hooks;
mod local_tmux;
#[path = "../local_tmux.rs"]
#[allow(dead_code)]
mod local_tmux_shared;
mod project;
#[path = "../project_config.rs"]
mod project_config;
#[path = "../resume.rs"]
#[allow(dead_code)]
mod resume;
pub mod socket_client;
mod teams;
mod tree;
#[path = "../updater.rs"]
#[allow(dead_code)]
mod updater;

pub use socket_client::CliError;

mod args;
pub mod browser_argv;
mod diff;
mod handles;
pub use args::{BrowserCommand, Cli, Commands};
use std::io::Write;
use std::time::Duration;

/// Replace this CLI in its owning local terminal with an explicitly requested saved command.
/// Validate identity/checkpoint/location before exec; never inject text into a foreground application.
fn restore_terminal(
    mut client: socket_client::SocketClient,
    surface: Option<&str>,
    checkpoint: Option<&str>,
    automatic: bool,
) -> Result<(), CliError> {
    use std::io::IsTerminal;
    let current = std::env::var("CMUX_SURFACE_ID").ok();
    let surface = surface
        .ok_or_else(|| CliError::Command("restore requires a cmux terminal surface".into()))?;
    if current.as_deref() != Some(surface) || !std::io::stdin().is_terminal() {
        return Err(CliError::Command(
            "run restore inside the target cmux terminal".into(),
        ));
    }
    let response = client.call(
        "surface.resume.show",
        serde_json::json!({"surface_id": surface}),
    )?;
    if response
        .get("execution_location")
        .and_then(|value| value.as_str())
        != Some("local")
    {
        return Err(CliError::Command(
            "remote resume must run through its remote workspace transport".into(),
        ));
    }
    if automatic
        && response
            .get("auto_resume")
            .and_then(|value| value.as_bool())
            != Some(true)
    {
        return Err(CliError::Command(
            "automatic resume requires a current approval in Preferences".into(),
        ));
    }
    let mut binding: resume::ResumeBinding =
        serde_json::from_value(response.get("resume_binding").cloned().unwrap_or_default())
            .map_err(|_| CliError::Command("terminal has no usable resume binding".into()))?;
    binding
        .validate()
        .map_err(|error| CliError::Command(error.into()))?;
    binding.sanitize_environment();
    if checkpoint.is_some() && checkpoint != binding.checkpoint_id.as_deref() {
        return Err(CliError::Command("checkpoint mismatch".into()));
    }
    let mut command = std::process::Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(&binding.command)
        .envs(&binding.environment);
    if let Some(cwd) = binding.cwd.as_ref().filter(|cwd| !cwd.is_empty()) {
        command.current_dir(cwd);
    }
    drop(client);
    Err(CliError::Command(format!(
        "resume launch failed: {}",
        cmux_platform::process::replace_current(&mut command)
    )))
}

/// Pick the first socket candidate that is not empty after trimming.
fn first_non_blank<'a>(candidates: impl IntoIterator<Item = Option<&'a str>>) -> Option<String> {
    candidates
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_owned)
}

/// Resolve the explicit socket override before discovery.
///
/// `--socket` wins, then `CMUX_SOCKET` (also read by the parser for `--help`), then
/// `CMUX_SOCKET_PATH` for clients that only know the modern name. Blank exports fall
/// through to discovery instead of failing to connect to an empty path.
fn socket_override(cli: &Cli) -> Option<String> {
    let modern = std::env::var("CMUX_SOCKET").ok();
    let legacy = std::env::var("CMUX_SOCKET_PATH").ok();
    first_non_blank([cli.socket.as_deref(), modern.as_deref(), legacy.as_deref()])
}

/// Run the CLI with the parsed arguments.
pub fn run(mut cli: Cli) -> Result<(), CliError> {
    let explicit_socket = socket_override(&cli);
    if let Commands::ClaudeTeams { args } = &cli.command {
        return teams::launch(args);
    }
    if let Commands::TmuxCompat { args } = &cli.command {
        return teams::tmux_compat(args, explicit_socket.as_deref());
    }
    if let Commands::Comments { command } = &cli.command {
        return comments::run(command, cli.json);
    }
    if let Commands::LocalTmux { command } = &cli.command {
        return local_tmux::run(command, explicit_socket.as_deref());
    }
    if let Commands::Tmux {
        command: args::TmuxAliasCommands::Attach { name, headless },
    } = &cli.command
    {
        let command = args::LocalTmuxCommands::Attach {
            name: name.clone(),
            headless: *headless,
        };
        return local_tmux::run(&command, explicit_socket.as_deref());
    }
    if let Commands::ProjectActions {
        directory,
        workspace: None,
    } = &cli.command
    {
        let global = project_config::global_path();
        let directory = directory
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("."));
        let resolved =
            project_config::resolve(directory, global.as_deref()).map_err(CliError::Command)?;
        let output = serde_json::to_string_pretty(&resolved)
            .map_err(|error| CliError::Command(error.to_string()))?;
        println!("{output}");
        return Ok(());
    }

    if let Commands::Hooks {
        command: args::HookCommands::Setup { agent, agent_flag },
    } = &cli.command
    {
        return hooks::setup(agent.as_deref().or(agent_flag.as_deref()));
    }
    let agent_hook = matches!(
        &cli.command,
        Commands::Hooks {
            command: args::HookCommands::Claude { .. }
                | args::HookCommands::Codex { .. }
                | args::HookCommands::Grok { .. }
                | args::HookCommands::Gemini { .. }
                | args::HookCommands::Kiro { .. }
                | args::HookCommands::Antigravity { .. }
                | args::HookCommands::HermesAgent { .. }
                | args::HookCommands::Kimi { .. }
                | args::HookCommands::Copilot { .. }
                | args::HookCommands::Codebuddy { .. }
                | args::HookCommands::Factory { .. }
                | args::HookCommands::Qoder { .. }
                | args::HookCommands::Opencode { .. }
                | args::HookCommands::Cursor { .. }
                | args::HookCommands::Pi { .. }
                | args::HookCommands::Omp { .. }
                | args::HookCommands::Campfire { .. }
                | args::HookCommands::Amp { .. }
                | args::HookCommands::Rovodev { .. }
        }
    );
    // Global agent settings also run outside cmux; only implicit, context-free hooks are skipped.
    // An explicit socket is still contacted; a dead one fails open at connect time below.
    if agent_hook && std::env::var_os("CMUX_SURFACE_ID").is_none() && explicit_socket.is_none() {
        return Ok(());
    }
    if matches!(cli.command, Commands::Update) {
        updater::manual_update().map_err(|e| CliError::Command(format!("{e:#}")))?;
        return Ok(());
    }

    // `send` joins its words, so an empty payload is a usage error, not an RPC.
    if let Commands::Send { text, .. } = &cli.command {
        if send_text(text).is_empty() {
            return Err(CliError::Command("send requires text".into()));
        }
    }

    // Browser argument combinations the socket cannot express are usage errors too.
    if let Commands::Browser(command) = &cli.command {
        validate_browser_command(command)?;
    }

    let prepared_diff = match &cli.command {
        Commands::Diff {
            input,
            source,
            unstaged,
            staged,
            branch,
            last_turn,
            cwd,
            base,
            surface,
            session,
            title,
            layout,
            font_size,
            ..
        } => Some(diff::prepare(diff::PrepareRequest {
            input: input.as_deref(),
            source: *source,
            unstaged: *unstaged,
            staged: *staged,
            branch: *branch,
            last_turn: *last_turn,
            cwd: cwd.as_deref(),
            base: base.as_deref(),
            surface: surface.as_deref(),
            session: session.as_deref(),
            title: title.as_deref(),
            layout: *layout,
            font_size: *font_size,
        })?),
        _ => None,
    };
    let prepared_project = match &cli.command {
        Commands::Project { path, .. } => Some(project::prepare(path)?),
        _ => None,
    };

    // Resolve socket path: explicit override > discovery > error
    let socket_path = if let Some(path) = explicit_socket {
        path
    } else {
        discovery::discover_socket().ok_or_else(|| {
            CliError::Connection("no cmux socket found (is cmux-app running?)".into())
        })?
    };

    // Keep the CLI alive through the daemon operation and both response boundaries.
    let timeout = match &cli.command {
        Commands::Browser(BrowserCommand::Wait { timeout_ms, .. }) => {
            let (_, client) = crate::browser_timeout::wait_budgets(*timeout_ms);
            client
        }
        Commands::Browser(BrowserCommand::Open { .. }) => Duration::from_secs(30),
        Commands::Project { .. } => Duration::from_secs(30),
        Commands::ProjectActions { .. } => Duration::from_secs(7),
        Commands::ProjectRun { .. } => Duration::from_secs(30),
        _ => Duration::from_secs(5),
    };

    if let Commands::Events {
        after,
        cursor_file,
        name,
        category,
        reconnect,
        limit,
        no_ack,
        no_heartbeat,
    } = &cli.command
    {
        return events::run(
            &socket_path,
            events::Options {
                after: *after,
                cursor_file: cursor_file.clone(),
                names: name.clone(),
                categories: category.clone(),
                reconnect: *reconnect,
                limit: *limit,
                ack: !*no_ack,
                heartbeats: !*no_heartbeat,
            },
        );
    }

    let mut client = match socket_client::SocketClient::connect(&socket_path, timeout) {
        // Like macOS, an installed hook fails open when the app is gone: a closed cmux never
        // blocks the agent (Claude treats exit code 2 from UserPromptSubmit as a refusal).
        Err(CliError::Connection(_)) if agent_hook => return Ok(()),
        result => result?,
    };
    if let (
        Some(prepared),
        Commands::Diff {
            workspace,
            surface,
            focus,
            no_focus,
            ..
        },
    ) = (prepared_diff, &cli.command)
    {
        return diff::open(
            &mut client,
            prepared,
            workspace.as_deref(),
            surface.as_deref(),
            *focus && !*no_focus,
            cli.json,
        );
    }
    if let (
        Some(prepared),
        Commands::Project {
            workspace,
            surface,
            focus,
            no_focus,
            ..
        },
    ) = (prepared_project, &cli.command)
    {
        return diff::open_document(
            &mut client,
            prepared.document,
            workspace.as_deref(),
            surface.as_deref(),
            *focus && !*no_focus,
            cli.json,
            "project",
        );
    }
    if let Commands::Hooks {
        command: args::HookCommands::Claude { event },
    } = &cli.command
    {
        return hooks::claude_event(&mut client, *event);
    }
    if let Commands::Hooks {
        command: args::HookCommands::Codex { event },
    } = &cli.command
    {
        return hooks::codex_event(&mut client, *event);
    }
    if let Commands::Hooks { command } = &cli.command {
        if let args::HookCommands::Rovodev { event } = command {
            return hooks::rovodev_event(&mut client, *event);
        }
        let provider_event = match command {
            args::HookCommands::Grok { event } => Some(("grok", *event)),
            args::HookCommands::Gemini { event } => Some(("gemini", *event)),
            args::HookCommands::Kiro { event } => Some(("kiro", *event)),
            args::HookCommands::Antigravity { event } => Some(("antigravity", *event)),
            args::HookCommands::HermesAgent { event } => Some(("hermes-agent", *event)),
            args::HookCommands::Kimi { event } => Some(("kimi", *event)),
            args::HookCommands::Copilot { event } => Some(("copilot", *event)),
            args::HookCommands::Codebuddy { event } => Some(("codebuddy", *event)),
            args::HookCommands::Factory { event } => Some(("factory", *event)),
            args::HookCommands::Qoder { event } => Some(("qoder", *event)),
            args::HookCommands::Opencode { event } => Some(("opencode", *event)),
            args::HookCommands::Cursor { event } => Some(("cursor", *event)),
            args::HookCommands::Pi { event } => Some(("pi", *event)),
            args::HookCommands::Omp { event } => Some(("omp", *event)),
            args::HookCommands::Campfire { event } => Some(("campfire", *event)),
            args::HookCommands::Amp { event } => Some(("amp", *event)),
            _ => None,
        };
        if let Some((provider, event)) = provider_event {
            return hooks::json_provider_event(&mut client, provider, event);
        }
    }

    if let Commands::Restore {
        surface,
        checkpoint,
        automatic,
    } = &cli.command
    {
        return restore_terminal(
            client,
            surface.as_deref(),
            checkpoint.as_deref(),
            *automatic,
        );
    }

    if cli.verbose {
        eprintln!("Connected to {}", socket_path);
    }

    // Canonicalize flag and positional targets, resolve indexes, and keep an
    // invalid handle's rejection ready to replace the server error it provokes.
    let invalid_handle = handles::normalize_command(&mut cli, &mut client)?;

    let use_color = format::use_color(cli.color.as_deref().unwrap_or("auto"));

    let started = std::time::Instant::now();
    // Handle Raw command separately (dynamic method name)
    let (method_name, result) = if let Commands::Raw {
        ref method,
        ref params,
    } = cli.command
    {
        let params_val: serde_json::Value = serde_json::from_str(params)
            .map_err(|e| CliError::Protocol(format!("invalid JSON params: {}", e)))?;
        let result = client.call(method, params_val);
        (method.clone(), result)
    } else if let Commands::Tree {
        all,
        ref workspace,
        ref window,
    } = cli.command
    {
        let result = tree::build(&mut client, all, workspace.as_deref(), window.as_deref());
        ("tree".to_string(), result)
    } else if let Commands::ListLog {
        limit,
        ref workspace,
    } = cli.command
    {
        let result = client
            .call("sidebar.metadata", serde_json::json!({"workspace_id":workspace}))
            .map(|metadata| log_listing(&metadata, limit));
        ("sidebar.list_log".to_string(), result)
    } else if let Commands::ListPaneSurfaces { ref pane } = cli.command {
        let result = tree::pane_surfaces(&mut client, pane.as_deref());
        ("pane.surfaces".to_string(), result)
    } else if let Commands::Browser(BrowserCommand::Identify { surface }) = &cli.command {
        let result = identify_browser(&mut client, surface.as_deref());
        ("system.identify".to_string(), result)
    } else {
        let (method, params) = command_to_rpc(&cli.command);
        let result = client.call(method, params);
        (method.to_string(), result)
    };

    if cli.verbose {
        if let Some(trace_id) = client.last_trace_id() {
            eprintln!(
                "trace_id={trace_id} round_trip_us={}",
                started.elapsed().as_micros()
            );
        }
    }

    let mut result = result.map_err(|error| invalid_handle.unwrap_or(error))?;

    // `read-screen --lines N` trims the rows it just read, on the CLI side.
    if let Commands::ReadScreen {
        lines: Some(lines), ..
    } = &cli.command
    {
        keep_last_lines(&mut result, *lines);
    }

    // Browser commands default to JSON; everything else defaults to human-readable
    let json_mode = match &cli.command {
        Commands::Browser(_) => !cli.no_json,
        _ => cli.json,
    };

    // `--id-format` shapes JSON output only, and only when it was requested.
    if json_mode {
        if let Some(mode) = cli.id_format {
            format::format_ids(&mut result, mode);
        }
    }

    // Output formatted result
    let output = format::format_response(&method_name, &result, json_mode, use_color);
    if !output.is_empty() {
        // A downstream consumer may finish early (for example, head). Flush
        // explicitly so other output failures reach the normal CLI error path.
        let mut stdout = std::io::stdout().lock();
        if let Err(error) = writeln!(stdout, "{output}").and_then(|()| stdout.flush()) {
            if error.kind() != std::io::ErrorKind::BrokenPipe {
                return Err(CliError::Output(format!("cannot write stdout: {error}")));
            }
        }
    }

    Ok(())
}

/// The surface this CLI runs in, if any: creation commands default to its workspace.
fn caller_surface() -> Option<String> {
    std::env::var("CMUX_SURFACE_ID")
        .ok()
        .filter(|value| !value.is_empty())
}

/// Scroll steps `browser scroll` passes through to the socket as `direction`.
const SCROLL_DIRECTIONS: [&str; 4] = ["up", "down", "left", "right"];

/// Whether the bare `scroll` positional names one of the stepped directions.
fn is_scroll_direction(direction: &Option<String>) -> bool {
    direction
        .as_deref()
        .is_some_and(|value| SCROLL_DIRECTIONS.contains(&value))
}

/// The vertical offset a bare `scroll` positional carries when it is written as an integer.
fn scroll_offset(direction: &Option<String>) -> Option<i32> {
    direction.as_deref()?.parse().ok()
}

/// Resolve a path argument against the current directory before it reaches the socket.
fn absolute_path(path: &str) -> String {
    std::path::absolute(path).map_or_else(
        |_| path.to_string(),
        |resolved| resolved.to_string_lossy().into_owned(),
    )
}

/// Refuse browser argument combinations the socket cannot express, before any connection.
fn validate_browser_command(command: &BrowserCommand) -> Result<(), CliError> {
    match command {
        BrowserCommand::Get {
            command: args::BrowserGetCommand::Attr { name, attr, .. },
        } if name.is_none() && attr.is_none() => Err(CliError::Command(
            "browser get attr requires --attr <name>".into(),
        )),
        BrowserCommand::Scroll {
            direction,
            selector,
            dx,
            dy,
            ..
        } if !is_scroll_direction(direction)
            && scroll_offset(direction).is_none()
            && selector.is_none()
            && dx.is_none()
            && dy.is_none() =>
        {
            Err(CliError::Command(
                "browser scroll requires a direction (up/down/left/right) or --dx/--dy".into(),
            ))
        }
        _ => Ok(()),
    }
}

/// Read one page field (`url` or `title`) from a browser RPC response.
fn browser_page_field(response: &serde_json::Value, field: &str) -> String {
    response
        .pointer(&format!("/data/{field}"))
        .or_else(|| response.get(field))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Identify the cmux topology, then the page a named browser surface shows.
fn identify_browser(
    client: &mut socket_client::SocketClient,
    surface: Option<&str>,
) -> Result<serde_json::Value, CliError> {
    let mut result = client.call("system.identify", serde_json::json!({}))?;
    let Some(surface) = surface else {
        return Ok(result);
    };
    let url = client.call(
        "browser.get.url",
        serde_json::json!({"surface_ref": surface}),
    )?;
    let title = client.call(
        "browser.get.title",
        serde_json::json!({"surface_ref": surface}),
    )?;
    let browser = serde_json::json!({
        "surface": surface,
        "url": browser_page_field(&url, "url"),
        "title": browser_page_field(&title, "title"),
    });
    if let serde_json::Value::Object(fields) = &mut result {
        fields.insert("browser".to_string(), browser);
    }
    Ok(result)
}

/// Map a BrowserCommand variant to its JSON-RPC method and params.
fn browser_command_to_rpc(cmd: &BrowserCommand) -> (&'static str, serde_json::Value) {
    use serde_json::json;
    match cmd {
        BrowserCommand::Open {
            url,
            workspace,
            profile,
        } => {
            let url = browser_address::normalize(url);
            (
                "browser.open",
                json!({"url": url, "workspace": workspace, "profile": profile}),
            )
        }
        BrowserCommand::List => ("browser.list", json!({})),
        BrowserCommand::Close { surface } => ("browser.close", json!({"surface_ref": surface})),
        BrowserCommand::Snapshot {
            surface,
            interactive,
            compact,
            max_depth,
        } => (
            "browser.snapshot",
            json!({
                "surface_ref": surface,
                "interactive": interactive,
                "compact": compact,
                "max_depth": max_depth
            }),
        ),
        BrowserCommand::Click {
            surface,
            target,
            snapshot_after,
        } => (
            "browser.click",
            json!({
                "surface_ref": surface,
                "target": target,
                "snapshot_after": snapshot_after
            }),
        ),
        BrowserCommand::Fill {
            surface,
            target,
            text,
            snapshot_after,
        } => (
            "browser.fill",
            json!({
                "surface_ref": surface,
                "target": target,
                "text": text,
                "snapshot_after": snapshot_after
            }),
        ),
        BrowserCommand::BrowserType {
            surface,
            selector,
            text,
        } => (
            "browser.type",
            json!({
                "surface_ref": surface,
                "selector": selector,
                "text": text
            }),
        ),
        BrowserCommand::Press { surface, key } => {
            ("browser.press", json!({"surface_ref": surface, "key": key}))
        }
        BrowserCommand::Hover { surface, selector } => (
            "browser.hover",
            json!({"surface_ref": surface, "selector": selector}),
        ),
        BrowserCommand::Scroll {
            surface,
            direction,
            amount,
            selector,
            dx,
            dy,
        } => {
            if is_scroll_direction(direction) {
                (
                    "browser.scroll",
                    json!({
                        "surface_ref": surface,
                        "direction": direction,
                        "amount": amount
                    }),
                )
            } else {
                // A bare integer positional scrolls vertically; an explicit `--dy` wins over it.
                let offset = (*dy).or_else(|| scroll_offset(direction));
                if selector.is_none() && dx.is_none() && offset.is_none() {
                    unreachable!("browser scroll is validated before dispatch");
                }
                (
                    "browser.scroll",
                    json!({
                        "surface_ref": surface,
                        "selector": selector,
                        "dx": dx,
                        "dy": offset
                    }),
                )
            }
        }
        BrowserCommand::Select {
            surface,
            selector,
            value,
        } => (
            "browser.select",
            json!({
                "surface_ref": surface,
                "selector": selector,
                "value": value
            }),
        ),
        BrowserCommand::Eval {
            surface,
            expression,
        } => (
            "browser.eval",
            json!({"surface_ref": surface, "script": expression}),
        ),
        BrowserCommand::Wait {
            surface,
            selector,
            text,
            url_contains,
            load_state,
            function,
            timeout_ms,
        } => (
            "browser.wait",
            json!({
                "surface_ref": surface,
                "selector": selector,
                "text": text,
                "url_contains": url_contains,
                "load_state": load_state,
                "function": function,
                "timeout_ms": timeout_ms
            }),
        ),
        BrowserCommand::Goto { surface, url } => (
            "browser.goto",
            json!({"surface_ref": surface, "url": browser_address::normalize(url)}),
        ),
        BrowserCommand::Back { surface } => ("browser.back", json!({"surface_ref": surface})),
        BrowserCommand::Forward { surface } => ("browser.forward", json!({"surface_ref": surface})),
        BrowserCommand::Reload { surface } => ("browser.reload", json!({"surface_ref": surface})),
        BrowserCommand::GetUrl { surface } => ("browser.url", json!({"surface_ref": surface})),
        BrowserCommand::GetTitle { surface } => ("browser.title", json!({"surface_ref": surface})),
        BrowserCommand::GetText { surface, selector } => (
            "browser.gettext",
            json!({"surface_ref": surface, "selector": selector}),
        ),
        BrowserCommand::GetHtml { surface, selector } => (
            "browser.gethtml",
            json!({"surface_ref": surface, "selector": selector}),
        ),
        BrowserCommand::Screenshot { surface, out } => (
            "browser.screenshot",
            json!({"surface_ref": surface, "path": out.as_deref().map(absolute_path)}),
        ),
        BrowserCommand::StreamEnable => ("browser.stream.enable", json!({})),
        BrowserCommand::StreamDisable => ("browser.stream.disable", json!({})),
        BrowserCommand::Identify { .. } => {
            unreachable!("browser identify is assembled in run()")
        }
        BrowserCommand::Dblclick { surface, selector } => (
            "browser.dblclick",
            json!({"surface_ref": surface, "selector": selector}),
        ),
        BrowserCommand::Focus { surface, selector } => (
            "browser.focus",
            json!({"surface_ref": surface, "selector": selector}),
        ),
        BrowserCommand::Check { surface, selector } => (
            "browser.check",
            json!({"surface_ref": surface, "selector": selector}),
        ),
        BrowserCommand::Uncheck { surface, selector } => (
            "browser.uncheck",
            json!({"surface_ref": surface, "selector": selector}),
        ),
        BrowserCommand::Keydown { surface, key } => (
            "browser.keydown",
            json!({"surface_ref": surface, "key": key}),
        ),
        BrowserCommand::Keyup { surface, key } => {
            ("browser.keyup", json!({"surface_ref": surface, "key": key}))
        }
        BrowserCommand::Highlight { surface, selector } => (
            "browser.highlight",
            json!({"surface_ref": surface, "selector": selector}),
        ),
        BrowserCommand::Frame { surface, target } => {
            if target == "main" {
                ("browser.frame.main", json!({"surface_ref": surface}))
            } else {
                (
                    "browser.frame.select",
                    json!({"surface_ref": surface, "selector": target}),
                )
            }
        }
        BrowserCommand::Console { surface, action } => match action.as_str() {
            "clear" => ("browser.console.clear", json!({"surface_ref": surface})),
            _ => ("browser.console.list", json!({"surface_ref": surface})),
        },
        BrowserCommand::Errors { surface, action } => match action.as_str() {
            "clear" => ("browser.errors.clear", json!({"surface_ref": surface})),
            _ => ("browser.errors.list", json!({"surface_ref": surface})),
        },
        BrowserCommand::Get { command } => match command {
            args::BrowserGetCommand::Url { surface } => {
                ("browser.get.url", json!({"surface_ref": surface}))
            }
            args::BrowserGetCommand::Title { surface } => {
                ("browser.get.title", json!({"surface_ref": surface}))
            }
            args::BrowserGetCommand::Text { surface, selector } => (
                "browser.get.text",
                json!({"surface_ref": surface, "selector": selector}),
            ),
            args::BrowserGetCommand::Html { surface, selector } => (
                "browser.get.html",
                json!({"surface_ref": surface, "selector": selector}),
            ),
            args::BrowserGetCommand::Value { surface, selector } => (
                "browser.get.value",
                json!({"surface_ref": surface, "selector": selector}),
            ),
            args::BrowserGetCommand::Attr {
                surface,
                selector,
                name,
                attr,
            } => {
                // `--attr` wins over the positional name; run() refuses an empty one.
                let attr = attr.as_deref().or(name.as_deref()).unwrap_or_default();
                (
                    "browser.get.attr",
                    json!({"surface_ref": surface, "selector": selector, "attr": attr}),
                )
            }
            args::BrowserGetCommand::Count { surface, selector } => (
                "browser.get.count",
                json!({"surface_ref": surface, "selector": selector}),
            ),
            args::BrowserGetCommand::Box { surface, selector } => (
                "browser.get.box",
                json!({"surface_ref": surface, "selector": selector}),
            ),
            args::BrowserGetCommand::Styles {
                surface,
                selector,
                property,
            } => (
                "browser.get.styles",
                json!({"surface_ref": surface, "selector": selector, "property": property}),
            ),
        },
        BrowserCommand::Is { command } => match command {
            args::BrowserIsCommand::Visible { surface, selector } => (
                "browser.is.visible",
                json!({"surface_ref": surface, "selector": selector}),
            ),
            args::BrowserIsCommand::Enabled { surface, selector } => (
                "browser.is.enabled",
                json!({"surface_ref": surface, "selector": selector}),
            ),
            args::BrowserIsCommand::Checked { surface, selector } => (
                "browser.is.checked",
                json!({"surface_ref": surface, "selector": selector}),
            ),
        },
        BrowserCommand::Dialog { command } => match command {
            args::BrowserDialogCommand::Accept { surface, text } => {
                let text = (!text.is_empty()).then(|| text.join(" "));
                (
                    "browser.dialog.accept",
                    json!({"surface_ref": surface, "text": text}),
                )
            }
            args::BrowserDialogCommand::Dismiss { surface } => {
                ("browser.dialog.dismiss", json!({"surface_ref": surface}))
            }
        },
        BrowserCommand::State { command } => match command {
            args::BrowserStateCommand::Save { surface, path } => (
                "browser.state.save",
                json!({"surface_ref": surface, "path": absolute_path(path)}),
            ),
            args::BrowserStateCommand::Load { surface, path } => (
                "browser.state.load",
                json!({"surface_ref": surface, "path": absolute_path(path)}),
            ),
        },
    }
}

/// Convert a CLI command to a JSON-RPC method and params.
/// Raw is handled separately in run() — panics if called with Raw.
fn command_to_rpc(cmd: &Commands) -> (&'static str, serde_json::Value) {
    use args::{ResumeCommands, SurfaceCommands};
    use serde_json::{json, Value};
    match cmd {
        Commands::ProjectRun {
            action,
            fingerprint,
            workspace,
            confirm,
        } => (
            "project.actions.run",
            serde_json::json!({"action_id":action,"fingerprint":fingerprint,"workspace_id":workspace,"confirmed":confirm}),
        ),
        Commands::ProjectActions { workspace, .. } => (
            "project.actions.list",
            serde_json::json!({"workspace_id":workspace}),
        ),
        Commands::Update => unreachable!("update is handled before socket discovery"),
        Commands::Diff { .. } => unreachable!("diff is prepared before socket dispatch"),
        Commands::Project { .. } => unreachable!("project is prepared before socket dispatch"),
        Commands::Comments { .. } => unreachable!("comments run without socket dispatch"),
        Commands::ClaudeTeams { .. } | Commands::TmuxCompat { .. } => {
            unreachable!("team launch commands are handled before socket discovery")
        }
        Commands::Ping => ("system.ping", json!({})),
        Commands::Identify {
            workspace,
            surface,
            window,
            no_caller,
        } => {
            let mut params = serde_json::Map::new();
            if let Some(window) = window {
                params.insert("window_id".into(), json!(window));
            }
            if !no_caller {
                let mut caller = serde_json::Map::new();
                if let Some(workspace) = workspace {
                    caller.insert("workspace_id".into(), json!(workspace));
                }
                if let Some(surface) = surface {
                    caller.insert("surface_id".into(), json!(surface));
                }
                if !caller.is_empty() {
                    params.insert("caller".into(), Value::Object(caller));
                }
            }
            ("system.identify", Value::Object(params))
        }
        Commands::Capabilities => ("system.capabilities", json!({})),
        Commands::Diagnostics => ("system.diagnostics", json!({})),
        Commands::Events { .. } => unreachable!("events stream on their own connection"),
        Commands::ListWorkspaces => ("workspace.list", json!({})),
        Commands::CurrentWorkspace => ("workspace.current", json!({})),

        Commands::Raw { .. } => unreachable!("Raw handled separately"),

        Commands::NewWorkspace { name, cwd, command } => {
            let mut params = serde_json::Map::new();
            if let Some(name) = name {
                params.insert("name".into(), json!(name));
            }
            if let Some(cwd) = cwd {
                params.insert("working_directory".into(), json!(cwd));
            }
            if let Some(command) = command {
                params.insert("initial_input".into(), json!(command));
            }
            ("workspace.create", Value::Object(params))
        }
        Commands::Ssh {
            destination,
            transport,
            name,
            directory,
        } => (
            "workspace.create",
            json!({"remote_target":destination,"terminal_transport":transport,
                "name":name,"remote_directory":directory}),
        ),
        Commands::Mosh {
            destination,
            name,
            directory,
        } => (
            "workspace.create",
            json!({"remote_target":destination,"terminal_transport":"mosh",
                "name":name,"remote_directory":directory}),
        ),
        Commands::MoshTmux {
            destination,
            session,
            name,
            directory,
        } => (
            "workspace.create",
            json!({"remote_target":destination,"terminal_transport":"mosh",
                "terminal_profile":"tmux","terminal_tmux_session":session,
                "name":name,"remote_directory":directory}),
        ),
        Commands::SelectWorkspace { id, .. } => ("workspace.select", json!({"id": id})),
        Commands::CloseWorkspace { id, .. } => ("workspace.close", json!({"id": id})),
        Commands::RenameWorkspace { id, name, .. } => {
            ("workspace.rename", json!({"id": id, "name": name}))
        }
        Commands::SetDescription {
            id, description, ..
        } => (
            "workspace.set_description",
            json!({"id": id, "description": description}),
        ),
        Commands::ClearDescription { id, .. } => ("workspace.clear_description", json!({"id": id})),
        Commands::NextWorkspace => ("workspace.next", json!({})),
        Commands::PrevWorkspace => ("workspace.previous", json!({})),
        Commands::LastWorkspace => ("workspace.last", json!({})),
        Commands::ReorderWorkspace { id, position, .. } => {
            ("workspace.reorder", json!({"id": id, "position": position}))
        }
        Commands::ReorderWorkspaces { order, dry_run } => (
            "workspace.reorder_many",
            json!({"workspace_ids":order,"dry_run":dry_run}),
        ),
        Commands::ListWorkspaceGroups => ("workspace.group.list", json!({})),
        Commands::CreateWorkspaceGroup { name, color } => {
            ("workspace.group.create", json!({"name":name,"color":color}))
        }
        Commands::UpdateWorkspaceGroup {
            id,
            name,
            color,
            clear_color,
            collapsed,
            position,
        } => {
            let mut params = json!({"id":id});
            if let Some(name) = name {
                params["name"] = json!(name);
            }
            if let Some(collapsed) = collapsed {
                params["collapsed"] = json!(collapsed);
            }
            if let Some(position) = position {
                params["position"] = json!(position);
            }
            if *clear_color {
                params["color"] = serde_json::Value::Null;
            } else if let Some(color) = color {
                params["color"] = json!(color);
            }
            ("workspace.group.update", params)
        }
        Commands::AssignWorkspaceGroup { group, workspaces } => (
            "workspace.group.assign",
            json!({"id":group,"workspace_ids":workspaces}),
        ),
        Commands::DeleteWorkspaceGroup { id } => ("workspace.group.delete", json!({"id":id})),

        Commands::Surface {
            command: SurfaceCommands::Resume { command },
        } => match command {
            ResumeCommands::Set {
                surface,
                shell,
                kind,
                checkpoint,
                cwd,
                name,
            } => (
                "surface.resume.set",
                json!({"surface_id": surface, "command": shell,
                    "kind": kind, "checkpoint_id": checkpoint, "cwd": cwd, "name": name}),
            ),
            ResumeCommands::Show { surface } => {
                ("surface.resume.show", json!({"surface_id": surface}))
            }
            ResumeCommands::Clear {
                surface,
                checkpoint,
            } => (
                "surface.resume.clear",
                json!({"surface_id": surface, "checkpoint_id": checkpoint}),
            ),
        },
        Commands::Hooks { .. } => unreachable!("hooks are handled before ordinary RPC dispatch"),
        Commands::Restore { .. } => unreachable!("restore executes in the caller terminal"),
        Commands::LocalTmux { .. } | Commands::Tmux { .. } => {
            unreachable!("local-tmux runs before socket dispatch")
        }
        Commands::ListSurfaces { workspace } => {
            ("surface.list", json!({"workspace_id": workspace}))
        }
        Commands::ListPaneSurfaces { .. } | Commands::Tree { .. } => {
            unreachable!("pane surfaces and tree are assembled client-side")
        }
        Commands::NewSplit {
            direction,
            surface,
            workspace,
            command,
            focus,
        } => (
            "surface.split",
            // Upstream splits the caller's own surface unless a target is named.
            json!({"direction": direction, "workspace_id": workspace,
                "surface_id": surface.clone().or_else(|| workspace.is_none().then(caller_surface).flatten()),
                "initial_input": command, "focus": focus.unwrap_or(false)}),
        ),
        Commands::NewPane {
            r#type,
            direction,
            workspace,
            command,
            focus,
        } => (
            "pane.create",
            json!({"type": r#type, "direction": direction, "workspace_id": workspace,
                "caller_surface_id": caller_surface(),
                "initial_input": command, "focus": focus.unwrap_or(false)}),
        ),
        Commands::NewSurface {
            r#type,
            pane,
            workspace,
            working_directory,
            command,
            focus,
        } => (
            "surface.create",
            json!({"type": r#type, "pane_id": pane, "workspace_id": workspace,
                "caller_surface_id": caller_surface(),
                "working_directory": working_directory.as_deref().map(|path| {
                    std::env::current_dir().map_or_else(|_| path.into(), |cwd| cwd.join(path))
                }),
                "initial_input": command, "focus": focus.unwrap_or(false)}),
        ),
        Commands::Split { direction, id } => {
            let mut p = serde_json::Map::new();
            p.insert("direction".into(), json!(direction));
            if let Some(ref id) = id {
                p.insert("id".into(), json!(id));
            }
            ("surface.split", Value::Object(p))
        }
        Commands::FocusSurface { id, .. } => ("surface.focus", json!({"id": id})),
        Commands::CloseSurface { id, .. } => ("surface.close", json!({"id": id})),
        Commands::MoveSurface {
            id,
            pane,
            workspace,
            window,
            before,
            after,
            position,
            no_focus,
            focus,
            ..
        } => {
            let mut params = serde_json::Map::new();
            params.insert("id".into(), json!(id));
            // `--focus` is upstream's spelling; without either flag the moved surface keeps
            // the historical focus-the-move behavior.
            params.insert(
                "focus".into(),
                json!(focus.as_ref().copied().unwrap_or(!*no_focus)),
            );
            for (key, value) in [
                ("workspace", workspace),
                ("pane", pane),
                ("window", window),
                ("before", before),
                ("after", after),
            ] {
                if let Some(value) = value {
                    params.insert(key.into(), json!(value));
                }
            }
            if let Some(position) = position {
                params.insert("position".into(), json!(position));
            }
            ("surface.move", Value::Object(params))
        }
        Commands::ReorderSurface { id, position, .. } => {
            ("surface.reorder", json!({"id": id, "position": position}))
        }
        Commands::DragSurfaceToSplit {
            id,
            pane,
            direction,
            ..
        } => (
            "surface.drag_to_split",
            json!({"id": id, "pane": pane, "direction": direction}),
        ),
        Commands::SendText { text, id } => {
            let mut p = serde_json::Map::new();
            p.insert("text".into(), json!(text));
            if let Some(ref id) = id {
                p.insert("id".into(), json!(id));
            }
            ("surface.send_text", Value::Object(p))
        }
        Commands::Send { text, id } => {
            let mut p = serde_json::Map::new();
            p.insert("text".into(), json!(send_text(text)));
            if let Some(ref id) = id {
                p.insert("id".into(), json!(id));
            }
            ("surface.send_text", Value::Object(p))
        }
        Commands::SendKey { key, id } => {
            let mut p = serde_json::Map::new();
            p.insert("key".into(), json!(key));
            if let Some(ref id) = id {
                p.insert("id".into(), json!(id));
            }
            ("surface.send_key", Value::Object(p))
        }
        Commands::ReadText { id } => {
            let mut p = serde_json::Map::new();
            if let Some(ref id) = id {
                p.insert("id".into(), json!(id));
            }
            ("surface.read_text", Value::Object(p))
        }
        Commands::ReadScreen {
            id,
            scrollback,
            lines,
        } => {
            let mut p = serde_json::Map::new();
            if let Some(ref id) = id {
                p.insert("id".into(), json!(id));
            }
            // `--lines` reads the scrollback too; the trailing rows are trimmed after the read.
            let method = if *scrollback || lines.is_some() {
                "surface.read_scrollback"
            } else {
                "surface.read_text"
            };
            (method, Value::Object(p))
        }
        Commands::ReadScrollback { id } => ("surface.read_scrollback", json!({"id": id})),
        Commands::Health { id } => {
            let mut p = serde_json::Map::new();
            if let Some(ref id) = id {
                p.insert("id".into(), json!(id));
            }
            ("surface.health", Value::Object(p))
        }
        Commands::Refresh { id } => {
            let mut p = serde_json::Map::new();
            if let Some(ref id) = id {
                p.insert("id".into(), json!(id));
            }
            ("surface.refresh", Value::Object(p))
        }

        Commands::ListPanes { workspace } => ("pane.list", json!({"workspace_id": workspace})),
        Commands::FocusPane { id, .. } => {
            let mut p = serde_json::Map::new();
            if let Some(ref id) = id {
                p.insert("id".into(), json!(id));
            }
            ("pane.focus", Value::Object(p))
        }
        Commands::LastPane => ("pane.last", json!({})),

        Commands::ListWindows => ("window.list", json!({})),
        Commands::CurrentWindow => ("window.current", json!({})),

        Commands::Layout => ("debug.layout", json!({})),
        Commands::Type { text } => ("debug.type", json!({"text": text})),

        Commands::SetStatus {
            key,
            value,
            icon,
            color,
            priority,
            format,
            url,
            workspace,
        } => (
            "sidebar.set_status",
            json!({"key":key,"value":value,"icon":icon,"color":color,"priority":priority,"format":format,"url":url,"workspace_id":workspace}),
        ),
        Commands::ClearStatus { key, workspace } => (
            "sidebar.clear_status",
            json!({"key":key,"workspace_id":workspace}),
        ),
        Commands::ReportMetaBlock {
            key,
            markdown,
            priority,
            workspace,
        } => (
            "sidebar.report_meta_block",
            json!({"key":key,"markdown":markdown,"priority":priority,"workspace_id":workspace}),
        ),
        Commands::ClearMetaBlock { key, workspace } => (
            "sidebar.clear_meta_block",
            json!({"key":key,"workspace_id":workspace}),
        ),
        Commands::ListMetaBlocks { workspace } => {
            ("sidebar.metadata", json!({"workspace_id":workspace}))
        }
        Commands::Ports { workspace, surface } => (
            "ports.list",
            json!({"workspace_id":workspace,"surface_id":surface}),
        ),
        Commands::ListStatus { workspace } => {
            ("sidebar.metadata", json!({"workspace_id":workspace}))
        }
        Commands::SetProgress {
            value,
            label,
            workspace,
        } => (
            "sidebar.set_progress",
            json!({"value":value,"label":label,"workspace_id":workspace}),
        ),
        Commands::ClearProgress { workspace } => {
            ("sidebar.clear_progress", json!({"workspace_id":workspace}))
        }
        Commands::Log {
            level,
            source,
            workspace,
            message,
        } => (
            "sidebar.log",
            json!({"message":message.join(" "),"level":level,"source":source,"workspace_id":workspace}),
        ),
        Commands::ListLog { workspace, .. } => {
            ("sidebar.metadata", json!({"workspace_id":workspace}))
        }
        Commands::ClearLog { workspace } => {
            ("sidebar.clear_log", json!({"workspace_id":workspace}))
        }
        Commands::SidebarState { workspace } => {
            ("sidebar.state", json!({"workspace_id":workspace}))
        }
        Commands::ListNotifications => ("notification.list", json!({})),
        Commands::Notify {
            workspace,
            surface,
            clear: true,
            ..
        } => {
            if workspace.is_some() || surface.is_some() {
                (
                    "notification.clear",
                    json!({"workspace_id":workspace,"surface_id":surface}),
                )
            } else {
                let mut params = notification_caller_params();
                params["caller"] = json!(true);
                ("notification.clear", params)
            }
        }
        Commands::Notify {
            title,
            subtitle,
            body,
            workspace,
            surface,
            message,
            ..
        } => {
            let body = message.as_ref().unwrap_or(body);
            if workspace.is_some() || surface.is_some() {
                (
                    "notification.create",
                    json!({"title":title,"subtitle":subtitle,"body":body,
                    "workspace_id":workspace,"surface_id":surface}),
                )
            } else {
                let mut params = notification_caller_params();
                params["title"] = json!(title);
                params["subtitle"] = json!(subtitle);
                params["body"] = json!(body);
                ("notification.create_for_caller", params)
            }
        }
        Commands::Notifications { command } => match command {
            args::NotificationCommands::List => ("notification.list", json!({})),
            args::NotificationCommands::Clear {
                caller,
                workspace,
                surface,
            } => {
                let params = if *caller {
                    let mut params = notification_caller_params();
                    params["caller"] = json!(true);
                    params
                } else {
                    json!({"workspace_id":workspace,"surface_id":surface})
                };
                ("notification.clear", params)
            }
            args::NotificationCommands::MarkRead {
                id,
                workspace,
                surface,
                all,
            } => (
                "notification.mark_read",
                json!({"id":id,"workspace_id":workspace,"surface_id":surface,"all":all}),
            ),
            args::NotificationCommands::Dismiss { id, all_read } => {
                ("notification.dismiss", json!({"id":id,"all_read":all_read}))
            }
            args::NotificationCommands::Open { id } => ("notification.open", json!({"id":id})),
            args::NotificationCommands::JumpToUnread => ("notification.jump_to_unread", json!({})),
        },
        Commands::DismissNotification { id, all_read } => {
            ("notification.dismiss", json!({"id":id,"all_read":all_read}))
        }
        Commands::ClearNotifications { workspace, surface } => (
            "notification.clear",
            json!({"workspace_id":workspace,"surface_id":surface}),
        ),
        Commands::ClearNotification { id } => ("notification.clear", json!({"id": id})),

        Commands::Browser(cmd) => browser_command_to_rpc(cmd),
    }
}

/// Keep the last `limit` log entries of a `sidebar.metadata` reply (upstream `list_log --limit`).
fn log_listing(metadata: &serde_json::Value, limit: Option<usize>) -> serde_json::Value {
    let logs = metadata
        .get("logs")
        .and_then(|logs| logs.as_array())
        .cloned()
        .unwrap_or_default();
    let skip = limit.map_or(0, |limit| logs.len().saturating_sub(limit));
    serde_json::json!({"workspace_id":metadata.get("workspace_id"),"logs":logs[skip..]})
}

/// Expand the `send` escape sequences: `\n` and `\r` become a carriage return,
/// `\t` a tabulation, and any other backslash sequence stays literal.
fn unescape_send_text(text: &str) -> String {
    let mut expanded = String::with_capacity(text.len());
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            expanded.push(character);
            continue;
        }
        match characters.next() {
            Some('n') | Some('r') => expanded.push('\r'),
            Some('t') => expanded.push('\t'),
            Some(other) => {
                expanded.push('\\');
                expanded.push(other);
            }
            None => expanded.push('\\'),
        }
    }
    expanded
}

/// Join `send`'s positional words and expand their escapes into terminal text.
fn send_text(text: &[String]) -> String {
    unescape_send_text(&text.join(" "))
}

/// Keep only the trailing `lines` rows of a `read-screen --lines` text field.
fn keep_last_lines(result: &mut serde_json::Value, lines: usize) {
    let Some(text) = result.get("text").and_then(serde_json::Value::as_str) else {
        return;
    };
    let rows: Vec<&str> = text.split('\n').collect();
    let trimmed = rows[rows.len().saturating_sub(lines)..].join("\n");
    if let Some(field) = result.get_mut("text") {
        *field = serde_json::Value::String(trimmed);
    }
}

/// Preserve ambient caller identity separately from explicit command flags; pipes provide no TTY claim.
fn notification_caller_params() -> serde_json::Value {
    serde_json::json!({
        "preferred_workspace_id": std::env::var("CMUX_WORKSPACE_ID").ok().filter(|value| !value.is_empty()),
        "preferred_surface_id": std::env::var("CMUX_SURFACE_ID").ok().filter(|value| !value.is_empty()),
        "preferred_workspace_is_explicit": false,
        "caller_tty": cmux_platform::terminal::caller_tty(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// Preserve explicit workspace naming and directory arguments in the outgoing RPC parameters.
    #[test]
    fn new_workspace_cli_sends_name_and_directory() {
        let cli = Cli::try_parse_from([
            "cmux",
            "new-workspace",
            "--name",
            "Project Alpha",
            "--cwd",
            "/tmp/project-alpha",
        ])
        .expect("workspace arguments should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "workspace.create");
        assert_eq!(params["name"], "Project Alpha");
        assert_eq!(params["working_directory"], "/tmp/project-alpha");
    }

    /// Optional move selectors must be absent rather than JSON null so backend defaults apply.
    #[test]
    fn move_surface_omits_unspecified_destination_fields() {
        let cli = Cli::try_parse_from([
            "cmux",
            "move-surface",
            "20000000-0000-4000-8000-000000000002",
            "--workspace",
            "30000000-0000-4000-8000-000000000003",
            "--no-focus",
        ])
        .expect("surface move arguments should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "surface.move");
        assert!(params.get("pane").is_none());
        assert!(params.get("position").is_none());
        assert_eq!(params["focus"], false);
    }

    /// Relative placement, the window and `--focus` reach `surface.move` as their parameters.
    #[test]
    fn move_surface_placement_flags_map_to_parameters() {
        let cli = Cli::try_parse_from([
            "cmux",
            "move-surface",
            "surface:3",
            "--before",
            "surface:1",
            "--window",
            "window:1",
            "--focus",
            "false",
        ])
        .expect("surface move arguments should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "surface.move");
        assert_eq!(params["id"], "surface:3");
        assert_eq!(params["before"], "surface:1");
        assert_eq!(params["window"], "window:1");
        assert_eq!(params["focus"], false);
        assert!(params.get("after").is_none());
        assert!(params.get("position").is_none());
    }

    /// The description commands reach the workspace by uuid, set and clear apart.
    #[test]
    fn description_commands_map_to_their_socket_methods() {
        let cli = Cli::try_parse_from([
            "cmux",
            "set-description",
            "20000000-0000-4000-8000-000000000002",
            "Refreshing investor metrics",
        ])
        .expect("set-description arguments should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "workspace.set_description");
        assert_eq!(params["id"], "20000000-0000-4000-8000-000000000002");
        assert_eq!(params["description"], "Refreshing investor metrics");

        let cli = Cli::try_parse_from([
            "cmux",
            "clear-description",
            "20000000-0000-4000-8000-000000000002",
        ])
        .expect("clear-description arguments should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "workspace.clear_description");
        assert!(params.get("description").is_none());
    }

    /// An explicit socket wins, then `CMUX_SOCKET`, then `CMUX_SOCKET_PATH`, ignoring blanks.
    #[test]
    fn socket_override_follows_flag_then_modern_then_legacy() {
        assert_eq!(
            first_non_blank([Some("/flag"), Some("/modern"), Some("/legacy")]).as_deref(),
            Some("/flag")
        );
        assert_eq!(
            first_non_blank([None, Some("/modern"), Some("/legacy")]).as_deref(),
            Some("/modern")
        );
        assert_eq!(
            first_non_blank([None, None, Some("/legacy")]).as_deref(),
            Some("/legacy")
        );
        assert_eq!(first_non_blank([None, Some("  "), Some("")]), None);
        assert_eq!(first_non_blank([None, None, None]), None);
    }

    /// Identify carries the caller anchors and the window the command named.
    #[test]
    fn identify_sends_window_and_caller() {
        let cli = Cli::try_parse_from([
            "cmux",
            "identify",
            "--workspace",
            "workspace:2",
            "--surface",
            "surface:8",
            "--window",
            "window:1",
        ])
        .expect("identify arguments should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "system.identify");
        assert_eq!(params["window_id"], "window:1");
        assert_eq!(params["caller"]["workspace_id"], "workspace:2");
        assert_eq!(params["caller"]["surface_id"], "surface:8");
    }

    /// `--no-caller` keeps the window and drops the caller anchor entirely.
    #[test]
    fn identify_without_caller_keeps_only_the_window() {
        let cli = Cli::try_parse_from(["cmux", "identify", "--window", "window:1", "--no-caller"])
            .expect("identify arguments should parse");
        let (_, params) = command_to_rpc(&cli.command);
        assert_eq!(params["window_id"], "window:1");
        assert!(params.get("caller").is_none());
    }

    /// An identification without any anchor asks only for the focused topology.
    #[test]
    fn identify_without_anchors_sends_no_caller() {
        let cli = Cli::try_parse_from(["cmux", "identify"]).expect("identify should parse");
        let (_, params) = command_to_rpc(&cli.command);
        assert_eq!(params, serde_json::json!({}));
    }

    /// `send` joins its words and expands only `\n`, `\r` and `\t`.
    #[test]
    fn send_joins_words_and_expands_escapes() {
        assert_eq!(unescape_send_text("a\\nb"), "a\rb");
        assert_eq!(unescape_send_text("a\\rb"), "a\rb");
        assert_eq!(unescape_send_text("x\\ty"), "x\ty");
        assert_eq!(unescape_send_text("no escapes"), "no escapes");
        assert_eq!(
            send_text(&["echo".to_string(), "hi\\n".to_string()]),
            "echo hi\r"
        );
    }

    /// `send` and `read-screen` reach the socket methods their options select.
    #[test]
    fn send_and_read_screen_map_to_their_socket_methods() {
        let cli = Cli::try_parse_from(["cmux", "send", "--surface", "surface:2", "echo", "hi\\n"])
            .expect("send arguments should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "surface.send_text");
        assert_eq!(params["text"], "echo hi\r");
        assert_eq!(params["id"], "surface:2");

        let cli = Cli::try_parse_from(["cmux", "read-screen"]).expect("read-screen should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "surface.read_text");
        assert_eq!(params, serde_json::json!({}));

        for arguments in [
            &["cmux", "read-screen", "--scrollback"][..],
            &["cmux", "read-screen", "--lines", "20"][..],
            &[
                "cmux",
                "read-screen",
                "--surface",
                "surface:3",
                "--lines",
                "5",
            ][..],
        ] {
            let cli = Cli::try_parse_from(arguments).expect("read-screen should parse");
            let (method, params) = command_to_rpc(&cli.command);
            assert_eq!(method, "surface.read_scrollback", "{arguments:?}");
            let _ = params;
        }
    }

    /// `--lines N` keeps the trailing rows of the text field it was given.
    #[test]
    fn read_screen_lines_keep_the_tail() {
        let mut result = serde_json::json!({"text": "a\nb\nc"});
        keep_last_lines(&mut result, 2);
        assert_eq!(result["text"], "b\nc");
        // A response without a text field is left untouched.
        let mut empty = serde_json::json!({"id": "surface:3"});
        keep_last_lines(&mut empty, 1);
        assert_eq!(empty, serde_json::json!({"id": "surface:3"}));
    }

    /// `browser get` reads values through its nested verbs, surface first in the subcommand.
    #[test]
    fn browser_get_group_maps_to_read_methods() {
        let cli = Cli::try_parse_from([
            "cmux",
            "browser",
            "get",
            "attr",
            "surface:3",
            "#link",
            "href",
        ])
        .expect("positional attribute name should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "browser.get.attr");
        assert_eq!(params["surface_ref"], "surface:3");
        assert_eq!(params["selector"], "#link");
        assert_eq!(params["attr"], "href");

        let cli = Cli::try_parse_from([
            "cmux",
            "browser",
            "get",
            "attr",
            "surface:3",
            "#link",
            "href",
            "--attr",
            "id",
        ])
        .expect("--attr should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "browser.get.attr");
        // `--attr` wins over the positional name.
        assert_eq!(params["attr"], "id");

        let cli = Cli::try_parse_from([
            "cmux",
            "browser",
            "get",
            "styles",
            "surface:3",
            "#box",
            "--property",
            "color",
        ])
        .expect("get styles should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "browser.get.styles");
        assert_eq!(params["surface_ref"], "surface:3");
        assert_eq!(params["selector"], "#box");
        assert_eq!(params["property"], "color");
    }

    /// `browser is` probes element state with the surface first.
    #[test]
    fn browser_is_group_maps_to_state_probes() {
        let cli = Cli::try_parse_from(["cmux", "browser", "is", "checked", "surface:3", "#agree"])
            .expect("is checked should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "browser.is.checked");
        assert_eq!(params["surface_ref"], "surface:3");
        assert_eq!(params["selector"], "#agree");
    }

    /// `frame` selects the main frame or an element by selector.
    #[test]
    fn browser_frame_selects_main_or_an_element() {
        let cli = Cli::try_parse_from(["cmux", "browser", "frame", "surface:3", "main"])
            .expect("frame main should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "browser.frame.main");
        assert_eq!(params["surface_ref"], "surface:3");
        assert!(params.get("selector").is_none());

        let cli = Cli::try_parse_from(["cmux", "browser", "frame", "surface:3", "#f"])
            .expect("frame selector should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "browser.frame.select");
        assert_eq!(params["selector"], "#f");
    }

    /// `dialog accept` joins its words into one response text.
    #[test]
    fn browser_dialog_accept_joins_its_words() {
        let cli = Cli::try_parse_from([
            "cmux",
            "browser",
            "dialog",
            "accept",
            "surface:3",
            "bonjour",
            "toi",
        ])
        .expect("dialog accept should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "browser.dialog.accept");
        assert_eq!(params["surface_ref"], "surface:3");
        assert_eq!(params["text"], "bonjour toi");
    }

    /// `console` and `errors` default to listing and honour an explicit `clear`.
    #[test]
    fn browser_console_and_errors_pick_their_action() {
        let cli = Cli::try_parse_from(["cmux", "browser", "console", "surface:3"])
            .expect("console should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "browser.console.list");
        assert_eq!(params["surface_ref"], "surface:3");

        let cli = Cli::try_parse_from(["cmux", "browser", "errors", "surface:3", "clear"])
            .expect("errors clear should parse");
        let (method, _) = command_to_rpc(&cli.command);
        assert_eq!(method, "browser.errors.clear");
    }

    /// `scroll` keeps its direction form and adds the upstream offset forms.
    #[test]
    fn browser_scroll_keeps_direction_and_adds_offsets() {
        let cli = Cli::try_parse_from([
            "cmux",
            "browser",
            "scroll",
            "surface:3",
            "--selector",
            "#t",
            "--dy",
            "200",
        ])
        .expect("selector scroll should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "browser.scroll");
        assert_eq!(params["surface_ref"], "surface:3");
        assert_eq!(params["selector"], "#t");
        assert_eq!(params["dy"], 200);
        assert!(params.get("direction").is_none());

        let cli = Cli::try_parse_from(["cmux", "browser", "scroll", "surface:3", "-120"])
            .expect("integer scroll should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "browser.scroll");
        assert_eq!(params["dy"], -120);

        let cli = Cli::try_parse_from(["cmux", "browser", "scroll", "surface:3", "down"])
            .expect("directional scroll should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "browser.scroll");
        assert_eq!(params["direction"], "down");
        assert_eq!(params["amount"], 300);
    }

    /// `fill` without text clears the field instead of failing to parse.
    #[test]
    fn fill_without_text_defaults_to_empty() {
        let cli = Cli::try_parse_from(["cmux", "browser", "fill", "surface:3", "#i"])
            .expect("fill without text should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "browser.fill");
        assert_eq!(params["text"], "");
    }

    /// The upstream aliases `key`, `navigate` and `url` reach the same methods.
    #[test]
    fn browser_aliases_reach_their_methods() {
        let cli = Cli::try_parse_from(["cmux", "browser", "key", "surface:3", "Enter"])
            .expect("key alias should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "browser.press");
        assert_eq!(params["key"], "Enter");

        let cli = Cli::try_parse_from([
            "cmux",
            "browser",
            "navigate",
            "surface:3",
            "https://example.com",
        ])
        .expect("navigate alias should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "browser.goto");
        assert_eq!(params["url"], "https://example.com");

        let cli = Cli::try_parse_from(["cmux", "browser", "url", "surface:3"])
            .expect("url alias should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "browser.url");
        assert_eq!(params["surface_ref"], "surface:3");
    }

    /// `snapshot S -i` is the short form of `--interactive`.
    #[test]
    fn snapshot_accepts_the_short_interactive_flag() {
        let cli = Cli::try_parse_from(["cmux", "browser", "snapshot", "surface:3", "-i"])
            .expect("snapshot -i should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "browser.snapshot");
        assert_eq!(params["interactive"], true);
        assert_eq!(params["surface_ref"], "surface:3");
    }

    /// The simple action verbs each reach their own socket method.
    #[test]
    fn browser_action_verbs_map_to_their_methods() {
        for (verb, method) in [
            ("dblclick", "browser.dblclick"),
            ("focus", "browser.focus"),
            ("check", "browser.check"),
            ("uncheck", "browser.uncheck"),
            ("highlight", "browser.highlight"),
        ] {
            let cli = Cli::try_parse_from(["cmux", "browser", verb, "surface:3", "#b"])
                .expect("verb should parse");
            let (actual, params) = command_to_rpc(&cli.command);
            assert_eq!(actual, method, "{verb}");
            assert_eq!(params["surface_ref"], "surface:3");
            assert_eq!(params["selector"], "#b");
        }
        for (verb, method) in [("keydown", "browser.keydown"), ("keyup", "browser.keyup")] {
            let cli = Cli::try_parse_from(["cmux", "browser", verb, "surface:3", "Enter"])
                .expect("verb should parse");
            let (actual, params) = command_to_rpc(&cli.command);
            assert_eq!(actual, method, "{verb}");
            assert_eq!(params["key"], "Enter");
        }
    }

    /// `state` and `screenshot --out` deliver absolute paths, `--out` omitted stays null.
    #[test]
    fn browser_state_and_screenshot_paths_are_absolute() {
        let cli = Cli::try_parse_from([
            "cmux",
            "browser",
            "state",
            "save",
            "surface:3",
            "/tmp/state.json",
        ])
        .expect("state save should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "browser.state.save");
        assert_eq!(params["path"], "/tmp/state.json");

        let cli = Cli::try_parse_from([
            "cmux",
            "browser",
            "screenshot",
            "surface:3",
            "--out",
            "/tmp/shot.png",
        ])
        .expect("screenshot --out should parse");
        let (method, params) = command_to_rpc(&cli.command);
        assert_eq!(method, "browser.screenshot");
        assert_eq!(params["path"], "/tmp/shot.png");

        let cli = Cli::try_parse_from(["cmux", "browser", "screenshot", "surface:3"])
            .expect("screenshot should parse");
        let (_, params) = command_to_rpc(&cli.command);
        assert!(params["path"].is_null());
    }

    /// Browser usage errors are command errors reported before any connection.
    #[test]
    fn browser_usage_errors_are_reported_before_connecting() {
        let cli = Cli::try_parse_from(["cmux", "browser", "get", "attr", "surface:3", "#i"])
            .expect("get attr should parse");
        let message = match run(cli) {
            Err(CliError::Command(message)) => message,
            _ => panic!("get attr without a name must fail"),
        };
        assert_eq!(message, "browser get attr requires --attr <name>");

        let cli = Cli::try_parse_from(["cmux", "browser", "scroll", "surface:3"])
            .expect("scroll should parse");
        let message = match run(cli) {
            Err(CliError::Command(message)) => message,
            _ => panic!("scroll without a direction must fail"),
        };
        assert_eq!(
            message,
            "browser scroll requires a direction (up/down/left/right) or --dx/--dy"
        );
    }

    /// Browser page fields are read from `data` first, then from the top level.
    #[test]
    fn browser_page_fields_fall_back_to_the_top_level() {
        assert_eq!(
            browser_page_field(
                &serde_json::json!({"success": true, "data": {"url": "https://x"}}),
                "url"
            ),
            "https://x"
        );
        assert_eq!(
            browser_page_field(&serde_json::json!({"title": "Page"}), "title"),
            "Page"
        );
        assert_eq!(browser_page_field(&serde_json::json!({}), "url"), "");
    }
}
