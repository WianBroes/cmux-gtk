//! Handle canonicalization and normalization for target-taking CLI commands.
//!
//! Wherever a window, workspace, pane or surface is named, the CLI accepts a UUID,
//! a `kind:N` reference or a bare index. References travel untouched because the
//! server resolves them; indexes are resolved here against the list that carries
//! the `index` field. A value that is none of the three keeps its literal form so
//! the server keeps the last word, and its documented rejection replaces the server
//! error when the dispatch fails.

use super::args::{self, Commands};
use super::socket_client::{CliError, SocketClient};
use super::Cli;
use serde_json::Value;

/// One of the four handle kinds a socket argument can designate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HandleKind {
    Window,
    Workspace,
    Pane,
    Surface,
}

impl HandleKind {
    /// Lower-case handle name used in messages and as the `kind:` prefix.
    fn name(self) -> &'static str {
        match self {
            HandleKind::Window => "window",
            HandleKind::Workspace => "workspace",
            HandleKind::Pane => "pane",
            HandleKind::Surface => "surface",
        }
    }

    /// Capitalized handle name used as the subject of an error message.
    fn label(self) -> &'static str {
        match self {
            HandleKind::Window => "Window",
            HandleKind::Workspace => "Workspace",
            HandleKind::Pane => "Pane",
            HandleKind::Surface => "Surface",
        }
    }

    /// List method whose records carry the `index` field used for bare index resolution.
    fn list_method(self) -> &'static str {
        match self {
            HandleKind::Window => "window.list",
            HandleKind::Workspace => "workspace.list",
            HandleKind::Pane => "pane.list",
            HandleKind::Surface => "surface.list",
        }
    }

    /// Field holding that method's records in its response.
    fn list_field(self) -> &'static str {
        match self {
            HandleKind::Window => "windows",
            HandleKind::Workspace => "workspaces",
            HandleKind::Pane => "panes",
            HandleKind::Surface => "surfaces",
        }
    }
}

/// How a raw handle behaves before any server round trip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Classification {
    /// A UUID or a reference of the expected kind: pass it through unchanged.
    Preserved,
    /// A bare index that the matching list must resolve.
    Index(i64),
    /// Neither of those: keep the literal value and report it after the dispatch.
    Invalid,
}

/// True for a canonical `8-4-4-4-12` UUID or its unhyphenated 32-digit form.
fn is_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    match bytes.len() {
        36 => bytes
            .iter()
            .enumerate()
            .all(|(position, byte)| match position {
                8 | 13 | 18 | 23 => *byte == b'-',
                _ => byte.is_ascii_hexdigit(),
            }),
        32 => bytes.iter().all(u8::is_ascii_hexdigit),
        _ => false,
    }
}

/// Parse `kind:N` with a known kind and a decimal index, mirroring the macOS `isHandleRef`.
fn ref_kind(value: &str) -> Option<HandleKind> {
    let (kind, index) = value.split_once(':')?;
    if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match kind.to_ascii_lowercase().as_str() {
        "window" => Some(HandleKind::Window),
        "workspace" => Some(HandleKind::Workspace),
        "pane" => Some(HandleKind::Pane),
        "surface" => Some(HandleKind::Surface),
        _ => None,
    }
}

/// Classify a raw handle without contacting the server.
fn classify(kind: HandleKind, raw: &str) -> Classification {
    let trimmed = raw.trim();
    if is_uuid(trimmed) {
        return Classification::Preserved;
    }
    match ref_kind(trimmed) {
        Some(ref_kind) if ref_kind == kind => return Classification::Preserved,
        Some(_) => return Classification::Invalid,
        None => {}
    }
    match trimmed.parse::<i64>() {
        Ok(index) => Classification::Index(index),
        Err(_) => Classification::Invalid,
    }
}

/// The documented rejection for a value that is neither a UUID, a reference nor an index.
fn invalid_handle_message(kind: HandleKind, value: &str) -> String {
    format!(
        "Invalid {} handle: {} (expected UUID, ref like {}:{}, or index)",
        kind.name(),
        value,
        kind.name(),
        1
    )
}

/// Read one record's `index`, accepting JSON numbers and decimal strings.
fn record_index(record: &Value, field: &str) -> Option<i64> {
    let value = record.get(field)?;
    value
        .as_i64()
        .or_else(|| value.as_str()?.trim().parse::<i64>().ok())
}

/// Find the record whose `index` matches, preferring its UUID over its reference.
fn record_at_index(list: &Value, field: &str, index: i64) -> Option<String> {
    let records = list.get(field)?.as_array()?;
    let record = records
        .iter()
        .find(|record| record_index(record, "index") == Some(index))?;
    record
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| record.get("ref").and_then(Value::as_str))
        .map(str::to_owned)
}

/// Resolve a bare index through the list that carries `index`.
///
/// Errors when the server rejects the list call or when no record carries the index.
fn resolve_index(
    client: &mut SocketClient,
    kind: HandleKind,
    index: i64,
) -> Result<String, CliError> {
    let params = match kind {
        // Pane and surface indexes count within one workspace: like macOS, a bare index
        // designates the current (focused) workspace.
        HandleKind::Pane | HandleKind::Surface => {
            let identify = client.call("system.identify", serde_json::json!({}))?;
            match identify.pointer("/focused/workspace_id").and_then(Value::as_str) {
                Some(workspace) => serde_json::json!({"workspace_id": workspace}),
                None => serde_json::json!({}),
            }
        }
        HandleKind::Window | HandleKind::Workspace => serde_json::json!({}),
    };
    let list = client.call(kind.list_method(), params)?;
    record_at_index(&list, kind.list_field(), index)
        .ok_or_else(|| CliError::Command(format!("{} index not found", kind.label())))
}

/// Move a flag-supplied handle into the slot the RPC parameters read.
///
/// Reports both spellings together, which clap already refuses for the commands that
/// declare the conflict, so a parser change cannot silently drop one of them.
fn fold_flag(
    flag: &mut Option<String>,
    slot: &mut Option<String>,
    command: &str,
) -> Result<(), CliError> {
    let Some(value) = flag.take() else {
        return Ok(());
    };
    if slot.is_some() {
        return Err(CliError::Command(format!(
            "{command} takes either the positional handle or its flag, not both"
        )));
    }
    *slot = Some(value);
    Ok(())
}

/// Claim the value clap stored in the leading positional when a handle flag is present.
///
/// Positionals are filled in declaration order, so `rename-workspace --workspace
/// workspace:2 -- "name"` stores the name in the handle slot. Returns that value for
/// the caller to place in the trailing field, and refuses a second positional together
/// with the flag.
fn shift_trailing(
    flag: &Option<String>,
    leading: &mut Option<String>,
    trailing_given: bool,
    command: &str,
) -> Result<Option<String>, CliError> {
    if flag.is_none() {
        return Ok(None);
    }
    if trailing_given {
        return Err(CliError::Command(format!(
            "{command} takes either the positional handle or its flag, not both"
        )));
    }
    Ok(leading.take())
}

/// Parse the zero-based position that follows the handle in a flagged invocation.
fn parse_position(raw: &str) -> Result<usize, CliError> {
    raw.parse::<usize>().map_err(|_| {
        CliError::Command(format!(
            "Invalid position: {} (expected a non-negative integer)",
            raw
        ))
    })
}

/// Fill the `identify` caller anchors from the cmux-spawned environment.
///
/// An explicit window or workspace keeps the identification anchored to what the caller
/// named; without flags the ambient workspace and surface describe the calling terminal.
/// No TTY lookup happens here, so a pipe makes no claim on a terminal.
fn identify_caller(
    workspace: &mut Option<String>,
    surface: &mut Option<String>,
    window: Option<&str>,
    env_workspace: Option<String>,
    env_surface: Option<String>,
) {
    if window.is_some() {
        return;
    }
    let workspace_named = workspace.is_some();
    if !workspace_named {
        *workspace = env_workspace;
    }
    if !workspace_named && surface.is_none() {
        *surface = env_surface;
    }
}

/// Resolve flag and positional spellings into the single handle slot each command dispatches.
///
/// Runs before any socket traffic, so a target given twice or a missing trailing value is
/// reported without opening a connection.
pub(super) fn canonicalize(cli: &mut Cli) -> Result<(), CliError> {
    match &mut cli.command {
        Commands::SelectWorkspace { id, workspace }
        | Commands::CloseWorkspace { id, workspace }
        | Commands::ClearDescription { id, workspace } => {
            fold_flag(workspace, id, "workspace target")?
        }
        Commands::FocusSurface { id, surface } | Commands::CloseSurface { id, surface } => {
            fold_flag(surface, id, "surface target")?
        }
        Commands::MoveSurface { id, surface, .. }
        | Commands::DragSurfaceToSplit { id, surface, .. } => {
            fold_flag(surface, id, "surface target")?
        }
        Commands::FocusPane { id, pane } => fold_flag(pane, id, "pane target")?,
        Commands::RenameWorkspace {
            id,
            name,
            workspace,
        } => {
            if let Some(trailing) =
                shift_trailing(workspace, id, name.is_some(), "rename-workspace")?
            {
                *name = Some(trailing);
            }
            if name.is_none() {
                return Err(CliError::Command("rename-workspace requires a name".into()));
            }
            fold_flag(workspace, id, "rename-workspace")?;
        }
        Commands::SetDescription {
            id,
            description,
            workspace,
        } => {
            if let Some(trailing) =
                shift_trailing(workspace, id, description.is_some(), "set-description")?
            {
                *description = Some(trailing);
            }
            if description.is_none() {
                return Err(CliError::Command(
                    "set-description requires a description".into(),
                ));
            }
            fold_flag(workspace, id, "set-description")?;
        }
        Commands::ReorderWorkspace {
            id,
            position,
            workspace,
            index,
            before,
            after,
            ..
        } => {
            if let Some(trailing) = shift_trailing(workspace, id, position.is_some(), "reorder-workspace")? {
                if index.is_some() {
                    return Err(CliError::Command(
                        "reorder-workspace takes either the trailing position or --index, not both".into(),
                    ));
                }
                *position = Some(parse_position(&trailing)?);
            }
            if let Some(index) = index.take() {
                *position = Some(index);
            }
            if position.is_none() && before.is_none() && after.is_none() {
                return Err(CliError::Command(
                    "reorder-workspace requires a position or --before/--after".into(),
                ));
            }
            fold_flag(workspace, id, "reorder-workspace")?;
        }
        Commands::ReorderSurface {
            id,
            position,
            surface,
            index,
            before,
            after,
            ..
        } => {
            if let Some(trailing) = shift_trailing(surface, id, position.is_some(), "reorder-surface")? {
                if index.is_some() {
                    return Err(CliError::Command(
                        "reorder-surface takes either the trailing position or --index, not both".into(),
                    ));
                }
                *position = Some(parse_position(&trailing)?);
            }
            if let Some(index) = index.take() {
                *position = Some(index);
            }
            if position.is_none() && before.is_none() && after.is_none() {
                return Err(CliError::Command(
                    "reorder-surface requires a position or --before/--after".into(),
                ));
            }
            fold_flag(surface, id, "reorder-surface")?;
        }
        Commands::Identify {
            workspace,
            surface,
            window,
            no_caller: false,
        } => {
            let env_workspace = std::env::var("CMUX_WORKSPACE_ID")
                .ok()
                .filter(|value| !value.is_empty());
            let env_surface = std::env::var("CMUX_SURFACE_ID")
                .ok()
                .filter(|value| !value.is_empty());
            identify_caller(
                workspace,
                surface,
                window.as_deref(),
                env_workspace,
                env_surface,
            );
        }
        Commands::Identify { .. } => {}
        _ => {}
    }
    Ok(())
}

/// A handle slot collected from the command before normalization.
enum Target<'a> {
    /// An optional slot: absent means the server keeps its own default target.
    Optional(HandleKind, &'a mut Option<String>),
    /// A slot the parser always fills, such as a required `--pane`.
    Required(HandleKind, &'a mut String),
}

impl Target<'_> {
    /// Handle kind whose rules apply to this slot.
    fn kind(&self) -> HandleKind {
        match self {
            Target::Optional(kind, _) | Target::Required(kind, _) => *kind,
        }
    }

    /// Raw value as parsed, or `None` when the command omitted an optional target.
    fn raw(&self) -> Option<String> {
        match self {
            Target::Optional(_, slot) => slot.as_ref().cloned(),
            Target::Required(_, value) => Some(value.to_string()),
        }
    }

    /// Store a value resolved from a list back into the slot.
    fn store(&mut self, value: String) {
        match self {
            Target::Optional(_, slot) => **slot = Some(value),
            Target::Required(_, slot) => **slot = value,
        }
    }
}

/// Collect every handle slot of the command so normalization can rewrite them uniformly.
///
/// Commands dispatched outside socket routing (diff, project, browser, hooks, restore and
/// the resume bindings) are absent on purpose: they keep their current raw-value behavior.
fn collect_targets<'a>(command: &'a mut Commands, targets: &mut Vec<Target<'a>>) {
    match command {
        Commands::SelectWorkspace { id, .. }
        | Commands::CloseWorkspace { id, .. }
        | Commands::ClearDescription { id, .. }
        | Commands::RenameWorkspace { id, .. }
        | Commands::SetDescription { id, .. } => {
            targets.push(Target::Optional(HandleKind::Workspace, id))
        }

        Commands::ReorderWorkspace {
            id,
            before,
            after,
            window,
            ..
        } => {
            targets.push(Target::Optional(HandleKind::Workspace, id));
            targets.push(Target::Optional(HandleKind::Workspace, before));
            targets.push(Target::Optional(HandleKind::Workspace, after));
            targets.push(Target::Optional(HandleKind::Window, window));
        }

        Commands::ReorderSurface {
            id,
            before,
            after,
            window,
            ..
        } => {
            targets.push(Target::Optional(HandleKind::Surface, id));
            targets.push(Target::Optional(HandleKind::Surface, before));
            targets.push(Target::Optional(HandleKind::Surface, after));
            targets.push(Target::Optional(HandleKind::Window, window));
        }

        Commands::FocusSurface { id, .. }
        | Commands::CloseSurface { id, .. }
        | Commands::Split { id, .. }
        | Commands::SendText { id, .. }
        | Commands::Send { id, .. }
        | Commands::SendKey { id, .. }
        | Commands::ReadText { id }
        | Commands::ReadScreen { id, .. }
        | Commands::ReadScrollback { id }
        | Commands::Health { id }
        | Commands::Refresh { id } => targets.push(Target::Optional(HandleKind::Surface, id)),

        Commands::FocusPane { id, .. } => targets.push(Target::Optional(HandleKind::Pane, id)),

        Commands::NewSplit {
            surface, workspace, ..
        } => {
            targets.push(Target::Optional(HandleKind::Surface, surface));
            targets.push(Target::Optional(HandleKind::Workspace, workspace));
        }
        Commands::NewPane { workspace, .. } => {
            targets.push(Target::Optional(HandleKind::Workspace, workspace))
        }
        Commands::NewSurface {
            pane, workspace, ..
        } => {
            targets.push(Target::Optional(HandleKind::Pane, pane));
            targets.push(Target::Optional(HandleKind::Workspace, workspace));
        }

        Commands::DragSurfaceToSplit { id, pane, .. } => {
            targets.push(Target::Optional(HandleKind::Surface, id));
            targets.push(Target::Required(HandleKind::Pane, pane));
        }
        Commands::SplitOff { surface, .. } => {
            targets.push(Target::Required(HandleKind::Surface, surface))
        }
        Commands::MoveSurface {
            id,
            pane,
            workspace,
            window,
            before,
            after,
            ..
        } => {
            targets.push(Target::Optional(HandleKind::Surface, id));
            targets.push(Target::Optional(HandleKind::Pane, pane));
            targets.push(Target::Optional(HandleKind::Workspace, workspace));
            targets.push(Target::Optional(HandleKind::Window, window));
            targets.push(Target::Optional(HandleKind::Surface, before));
            targets.push(Target::Optional(HandleKind::Surface, after));
        }
        Commands::Notify {
            workspace, surface, ..
        }
        | Commands::Ports {
            workspace, surface, ..
        }
        | Commands::ClearNotifications { workspace, surface } => {
            targets.push(Target::Optional(HandleKind::Workspace, workspace));
            targets.push(Target::Optional(HandleKind::Surface, surface));
        }
        Commands::SetStatus { workspace, .. }
        | Commands::ReportMetaBlock { workspace, .. }
        | Commands::ClearMetaBlock { workspace, .. }
        | Commands::ListMetaBlocks { workspace, .. }
        | Commands::ClearStatus { workspace, .. }
        | Commands::ListStatus { workspace, .. }
        | Commands::SetProgress { workspace, .. }
        | Commands::ClearProgress { workspace, .. }
        | Commands::Log { workspace, .. }
        | Commands::ListLog { workspace, .. }
        | Commands::ClearLog { workspace, .. }
        | Commands::SidebarState { workspace, .. } => {
            targets.push(Target::Optional(HandleKind::Workspace, workspace))
        }

        Commands::Notifications {
            command:
                args::NotificationCommands::Clear {
                    workspace, surface, ..
                }
                | args::NotificationCommands::MarkRead {
                    workspace, surface, ..
                },
        } => {
            targets.push(Target::Optional(HandleKind::Workspace, workspace));
            targets.push(Target::Optional(HandleKind::Surface, surface));
        }

        Commands::ListSurfaces { workspace } | Commands::ListPanes { workspace } => {
            targets.push(Target::Optional(HandleKind::Workspace, workspace));
        }

        Commands::ListPaneSurfaces { pane } => {
            targets.push(Target::Optional(HandleKind::Pane, pane));
        }

        Commands::Tree {
            workspace, window, ..
        } => {
            targets.push(Target::Optional(HandleKind::Workspace, workspace));
            targets.push(Target::Optional(HandleKind::Window, window));
        }

        Commands::Identify {
            workspace,
            surface,
            window,
            ..
        } => {
            targets.push(Target::Optional(HandleKind::Workspace, workspace));
            targets.push(Target::Optional(HandleKind::Surface, surface));
            targets.push(Target::Optional(HandleKind::Window, window));
        }

        _ => {}
    }
}

/// Canonicalize the command and normalize every handle it dispatches.
///
/// Returns a deferred error for a structurally invalid handle: the raw value stays in the
/// request, and the documented message replaces the server error when that dispatch fails.
/// Resolving an index or reporting a malformed argument fails immediately, before the
/// command itself is sent.
pub(super) fn normalize_command(
    cli: &mut Cli,
    client: &mut SocketClient,
) -> Result<Option<CliError>, CliError> {
    canonicalize(cli)?;
    let mut targets: Vec<Target<'_>> = Vec::new();
    collect_targets(&mut cli.command, &mut targets);
    let mut deferred = None;
    for mut target in targets {
        let kind = target.kind();
        let Some(raw) = target.raw() else {
            continue;
        };
        match classify(kind, &raw) {
            Classification::Preserved => {}
            Classification::Index(index) => target.store(resolve_index(client, kind, index)?),
            Classification::Invalid => {
                if deferred.is_none() {
                    deferred = Some(CliError::Command(invalid_handle_message(kind, raw.trim())));
                }
            }
        }
    }
    Ok(deferred)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::io::{BufRead, BufReader, Write};
    use std::time::Duration;

    const WORKSPACE: &str = "20000000-0000-4000-8000-000000000002";
    const OTHER_WORKSPACE: &str = "30000000-0000-4000-8000-000000000003";

    /// Parse one invocation, reporting the parser diagnostics on failure.
    fn parse(arguments: &[&str]) -> Cli {
        let mut invocation = vec!["cmux"];
        invocation.extend_from_slice(arguments);
        Cli::try_parse_from(invocation)
            .unwrap_or_else(|error| panic!("{arguments:?} should parse: {error}"))
    }

    /// UUIDs of both spellings and matching references pass through untouched.
    #[test]
    fn uuids_and_references_are_preserved() {
        for raw in [
            WORKSPACE,
            "20000000-0000-4000-8000-000000000002",
            "20000000000040008000000000000002",
            "workspace:1",
            "WORKSPACE:1",
        ] {
            assert_eq!(
                classify(HandleKind::Workspace, raw),
                Classification::Preserved,
                "{raw}"
            );
        }
        assert_eq!(
            classify(HandleKind::Surface, "surface:8"),
            Classification::Preserved
        );
        assert_eq!(
            classify(HandleKind::Pane, "pane:0"),
            Classification::Preserved
        );
    }

    /// A reference of another kind and a non-handle string are structurally invalid.
    #[test]
    fn wrong_kind_references_and_text_are_invalid() {
        assert_eq!(
            classify(HandleKind::Workspace, "pane:1"),
            Classification::Invalid
        );
        assert_eq!(
            classify(HandleKind::Workspace, "surface:1"),
            Classification::Invalid
        );
        // A UUID names one object without saying which kind, so any target accepts it.
        assert_eq!(
            classify(HandleKind::Surface, WORKSPACE),
            Classification::Preserved
        );
        assert_eq!(
            classify(HandleKind::Workspace, "missing-workspace"),
            Classification::Invalid
        );
        assert_eq!(
            classify(HandleKind::Workspace, "workspace:x"),
            Classification::Invalid
        );
        assert_eq!(classify(HandleKind::Workspace, ""), Classification::Invalid);
        assert_eq!(
            classify(HandleKind::Workspace, "  "),
            Classification::Invalid
        );
    }

    /// Bare integers are indexes, including ones no list will ever contain.
    #[test]
    fn bare_integers_are_indexes() {
        assert_eq!(
            classify(HandleKind::Workspace, "2"),
            Classification::Index(2)
        );
        assert_eq!(
            classify(HandleKind::Workspace, "  3  "),
            Classification::Index(3)
        );
        assert_eq!(
            classify(HandleKind::Surface, "-1"),
            Classification::Index(-1)
        );
    }

    /// The documented message names the expected shapes of the requested kind.
    #[test]
    fn invalid_handle_message_documents_the_expected_shapes() {
        assert_eq!(
            invalid_handle_message(HandleKind::Workspace, "missing-workspace"),
            "Invalid workspace handle: missing-workspace (expected UUID, ref like workspace:1, or index)"
        );
        assert_eq!(
            invalid_handle_message(HandleKind::Pane, "pane:1"),
            "Invalid pane handle: pane:1 (expected UUID, ref like pane:1, or index)"
        );
    }

    /// Index lookup reads `index` and prefers the UUID, falling back to the reference.
    #[test]
    fn index_lookup_prefers_uuid_and_falls_back_to_ref() {
        let list = serde_json::json!({"workspaces": [
            {"index": 0, "id": WORKSPACE},
            {"index": "2", "ref": "workspace:2", "id": OTHER_WORKSPACE},
            {"index": 4, "ref": "workspace:4"}
        ]});
        assert_eq!(
            record_at_index(&list, "workspaces", 0).as_deref(),
            Some(WORKSPACE)
        );
        assert_eq!(
            record_at_index(&list, "workspaces", 2).as_deref(),
            Some(OTHER_WORKSPACE)
        );
        assert_eq!(
            record_at_index(&list, "workspaces", 4).as_deref(),
            Some("workspace:4")
        );
        assert!(record_at_index(&list, "workspaces", 9).is_none());
        assert!(record_at_index(&serde_json::json!({}), "workspaces", 0).is_none());
    }

    /// A flag handle claims the leading slot and the trailing value keeps its place.
    #[test]
    fn canonicalize_folds_flag_and_positional_spellings() {
        let mut cli = parse(&[
            "rename-workspace",
            "--workspace",
            "workspace:2",
            "--",
            "New name",
        ]);
        canonicalize(&mut cli).expect("flagged rename should canonicalize");
        let Commands::RenameWorkspace {
            id,
            name,
            workspace,
        } = cli.command
        else {
            panic!("wrong command");
        };
        assert_eq!(id.as_deref(), Some("workspace:2"));
        assert_eq!(name.as_deref(), Some("New name"));
        assert!(workspace.is_none());

        let mut cli = parse(&["rename-workspace", WORKSPACE, "New name"]);
        canonicalize(&mut cli).expect("positional rename should canonicalize");
        let Commands::RenameWorkspace {
            id,
            name,
            workspace,
        } = cli.command
        else {
            panic!("wrong command");
        };
        assert_eq!(id.as_deref(), Some(WORKSPACE));
        assert_eq!(name.as_deref(), Some("New name"));
        assert!(workspace.is_none());

        let mut cli = parse(&["reorder-workspace", "--workspace", "workspace:2", "4"]);
        canonicalize(&mut cli).expect("flagged reorder should canonicalize");
        let Commands::ReorderWorkspace {
            id,
            position,
            workspace,
            ..
        } = cli.command
        else {
            panic!("wrong command");
        };
        assert_eq!(id.as_deref(), Some("workspace:2"));
        assert_eq!(position, Some(4));
        assert!(workspace.is_none());

        let mut cli = parse(&["focus-pane", "--pane", "pane:3"]);
        canonicalize(&mut cli).expect("flagged focus-pane should canonicalize");
        let Commands::FocusPane { id, pane } = cli.command else {
            panic!("wrong command");
        };
        assert_eq!(id.as_deref(), Some("pane:3"));
        assert!(pane.is_none());
    }

    /// A flagged command without its trailing value fails before any connection.
    #[test]
    fn canonicalize_reports_missing_trailing_values() {
        let mut cli = parse(&["rename-workspace", "--workspace", "workspace:2"]);
        let error = canonicalize(&mut cli).expect_err("a name is required");
        assert_eq!(error.to_string(), "rename-workspace requires a name");

        let mut cli = parse(&["reorder-surface", "--surface", "surface:1"]);
        let error = canonicalize(&mut cli).expect_err("a position is required");
        assert_eq!(
            error.to_string(),
            "reorder-surface requires a position or --before/--after"
        );

        let mut cli = parse(&["reorder-workspace", "--workspace", "workspace:2", "later"]);
        let error = canonicalize(&mut cli).expect_err("a position must be a number");
        assert!(
            error.to_string().starts_with("Invalid position: later"),
            "{error}"
        );
    }

    /// A relative placement stands in for the trailing position the reorder used to require.
    #[test]
    fn canonicalize_accepts_relative_reorder_placement() {
        let mut cli = parse(&[
            "reorder-surface",
            "--surface",
            "surface:1",
            "--after",
            "surface:2",
        ]);
        canonicalize(&mut cli).expect("anchor reorder should canonicalize");
        let Commands::ReorderSurface {
            id,
            position,
            surface,
            after,
            ..
        } = cli.command
        else {
            panic!("wrong command");
        };
        // The flag handle folds into the slot the RPC reads.
        assert_eq!(id.as_deref(), Some("surface:1"));
        assert_eq!(after.as_deref(), Some("surface:2"));
        assert!(surface.is_none() && position.is_none());

        let mut cli = parse(&[
            "reorder-workspace",
            "--workspace",
            "workspace:2",
            "--index",
            "3",
        ]);
        canonicalize(&mut cli).expect("index flag should canonicalize");
        let Commands::ReorderWorkspace {
            id,
            position,
            workspace,
            ..
        } = cli.command
        else {
            panic!("wrong command");
        };
        assert_eq!(id.as_deref(), Some("workspace:2"));
        assert_eq!(position, Some(3));
        assert!(workspace.is_none());
    }

    /// The caller environment fills only the anchors the caller did not name.
    #[test]
    fn identify_caller_fills_unnamed_anchors() {
        let mut workspace = None;
        let mut surface = None;
        identify_caller(
            &mut workspace,
            &mut surface,
            None,
            Some(WORKSPACE.to_owned()),
            Some("surface:4".to_owned()),
        );
        assert_eq!(workspace.as_deref(), Some(WORKSPACE));
        assert_eq!(surface.as_deref(), Some("surface:4"));

        // An explicit anchor keeps its value and suppresses the ambient surface.
        let mut workspace = Some("workspace:2".to_owned());
        let mut surface = None;
        identify_caller(
            &mut workspace,
            &mut surface,
            None,
            Some(WORKSPACE.to_owned()),
            Some("surface:4".to_owned()),
        );
        assert_eq!(workspace.as_deref(), Some("workspace:2"));
        assert!(surface.is_none());

        // A named window keeps both ambient anchors out of the request.
        let mut workspace = None;
        let mut surface = None;
        identify_caller(
            &mut workspace,
            &mut surface,
            Some("window:1"),
            Some(WORKSPACE.to_owned()),
            Some("surface:4".to_owned()),
        );
        assert!(workspace.is_none() && surface.is_none());
    }

    /// Answer list requests on a private socket so index resolution runs real framing.
    fn list_server(socket: &std::path::Path, replies: Vec<String>) -> std::thread::JoinHandle<()> {
        let listener = std::os::unix::net::UnixListener::bind(socket).expect("bind fake server");
        std::thread::spawn(move || {
            for reply in replies {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut request = String::new();
                BufReader::new(&stream)
                    .read_line(&mut request)
                    .expect("read request");
                stream.write_all(reply.as_bytes()).expect("write reply");
            }
        })
    }

    /// A bare index is resolved through the list response before the command is sent.
    #[test]
    fn bare_index_resolves_through_the_list() {
        let directory = std::env::temp_dir().join(format!("cmux-handles-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).expect("create test directory");
        let socket = directory.join("server.sock");
        let server = list_server(
            &socket,
            vec![format!(
                "{{\"id\":1,\"ok\":true,\"result\":{{\"workspaces\":[{{\"index\":0,\"id\":\"{WORKSPACE}\"}}]}}}}\n"
            )],
        );
        let mut client = SocketClient::connect(
            socket.to_str().expect("socket path"),
            Duration::from_secs(2),
        )
        .expect("connect to fake server");
        assert_eq!(
            resolve_index(&mut client, HandleKind::Workspace, 0).expect("index resolves"),
            WORKSPACE
        );
        server.join().expect("fake server thread");
        let _ = std::fs::remove_dir_all(directory);
    }

    /// An index no list contains fails with the handle's own message.
    #[test]
    fn missing_index_reports_the_handle_kind() {
        let directory = std::env::temp_dir().join(format!("cmux-handles-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).expect("create test directory");
        let socket = directory.join("server.sock");
        let server = list_server(
            &socket,
            vec![format!(
                "{{\"id\":1,\"ok\":true,\"result\":{{\"workspaces\":[{{\"index\":0,\"id\":\"{WORKSPACE}\"}}]}}}}\n"
            )],
        );
        let mut client = SocketClient::connect(
            socket.to_str().expect("socket path"),
            Duration::from_secs(2),
        )
        .expect("connect to fake server");
        let error = resolve_index(&mut client, HandleKind::Workspace, 9)
            .expect_err("index 9 is not listed");
        assert_eq!(error.to_string(), "Workspace index not found");
        server.join().expect("fake server thread");
        let _ = std::fs::remove_dir_all(directory);
    }

    /// An invalid handle stays in the request and is reported instead of the server error.
    #[test]
    fn structurally_invalid_handle_is_kept_for_the_server() {
        let directory = std::env::temp_dir().join(format!("cmux-handles-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).expect("create test directory");
        let socket = directory.join("server.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind idle server");
        let mut client = SocketClient::connect(
            socket.to_str().expect("socket path"),
            Duration::from_secs(2),
        )
        .expect("connect to idle server");
        let mut cli = parse(&["select-workspace", "missing-workspace"]);
        let deferred = normalize_command(&mut cli, &mut client)
            .expect("classifying the handle needs no round trip");
        let error = deferred.expect("the invalid handle must be reported");
        assert_eq!(
            error.to_string(),
            "Invalid workspace handle: missing-workspace (expected UUID, ref like workspace:1, or index)"
        );
        let Commands::SelectWorkspace { id, workspace } = cli.command else {
            panic!("wrong command");
        };
        assert_eq!(id.as_deref(), Some("missing-workspace"));
        assert!(workspace.is_none());
        let _ = std::fs::remove_dir_all(directory);
    }
}
