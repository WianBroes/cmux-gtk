//! Preferences section mirroring upstream Settings → Terminal → Keep Local Sessions Alive.
//! Like upstream, it lists sessions and starts/attaches them through the bundled
//! `cmux local-tmux` CLI; it never enables tmux for ordinary terminals.
use crate::app_state::AppStateRef;
use gtk4::prelude::*;

/// One row of `cmux local-tmux list --json`.
#[derive(serde::Deserialize)]
struct Session {
    session_name: String,
    cwd: Option<String>,
    clients: Option<u32>,
}

#[derive(serde::Deserialize)]
struct Listing {
    sessions: Vec<Session>,
}

fn cli() -> Option<String> {
    let path = std::env::current_exe().ok()?.with_file_name("cmux");
    Some(path.to_str()?.to_owned())
}

/// Ask the CLI for live sessions; None when it cannot answer.
fn sessions() -> Option<Vec<Session>> {
    let output = std::process::Command::new(cli()?)
        .args(["local-tmux", "list", "--json"])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let mut listing: Listing = serde_json::from_slice(&output.stdout).ok()?;
    listing
        .sessions
        .sort_by_key(|session| session.session_name.to_lowercase());
    Some(listing.sessions)
}

#[derive(serde::Deserialize)]
struct Status {
    clients: u32,
    attach_command: Option<String>,
}

/// Like upstream attach: when a surface of the active workspace is already bound to this
/// session's guarded attach and a client is attached, focus it instead of adding another client.
fn focus_existing(state: &AppStateRef, name: &str) -> bool {
    let Some(status) = cli()
        .and_then(|cli| {
            std::process::Command::new(cli)
                .args(["local-tmux", "status", name, "--json"])
                .stdin(std::process::Stdio::null())
                .output()
                .ok()
        })
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice::<Status>(&output.stdout).ok())
    else {
        return false;
    };
    let Some(expected) = status.attach_command.filter(|_| status.clients > 0) else {
        return false;
    };
    let mut state = state.borrow_mut();
    let index = state.active_index;
    let Some(engine) = state.split_engines.get_mut(index) else {
        return false;
    };
    let bound = engine.all_panes().into_iter().find_map(|(uuid, _, _)| {
        let uuid = uuid.to_string();
        let binding = engine
            .resume_action(&uuid, &crate::resume::ResumeAction::Show)
            .ok()??;
        (binding.command == expected).then_some(uuid)
    });
    bound.is_some_and(|uuid| engine.focus_surface(&uuid))
}

/// Run `cmux local-tmux <action> <name>` in a new tab of the active workspace, where the CLI
/// records the guarded attach as that tab's resume binding.
fn run_in_new_tab(state: &AppStateRef, action: &str, name: &str) -> Result<(), &'static str> {
    if !crate::local_tmux::valid_name(name) {
        return Err("Session names use letters, numbers, underscore or dash (1–128).");
    }
    let cli = cli().ok_or("The cmux CLI was not found next to the application.")?;
    let command = format!(
        "{} local-tmux {action} {}",
        crate::local_tmux::shell_quote(&cli),
        crate::local_tmux::shell_quote(name)
    );
    let mut state = state.borrow_mut();
    let index = state.active_index;
    let directory = state
        .workspaces
        .get(index)
        .and_then(|workspace| workspace.working_directory.clone())
        .or_else(|| std::env::var_os("HOME").map(Into::into))
        .ok_or("No workspace directory.")?;
    state
        .split_engines
        .get_mut(index)
        .and_then(|engine| engine.new_project_command(&command, directory))
        .map(drop)
        .ok_or("Local sessions need a local workspace.")
}

/// Append the section to Preferences; actions close the dialog so the new tab is visible.
pub fn append(content: &gtk4::Box, state: &AppStateRef, dialog: &gtk4::Dialog) {
    let title = gtk4::Label::new(None);
    title.set_markup("<b>Keep Local Sessions Alive</b>");
    title.set_xalign(0.0);
    title.set_margin_top(8);
    content.append(&title);
    let subtitle = gtk4::Label::new(Some("Named local-tmux sessions keep processes and scrollback alive across cmux quit, crashes, and updates. Ordinary terminals keep their current behavior."));
    subtitle.set_wrap(true);
    subtitle.set_xalign(0.0);
    content.append(&subtitle);

    let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let status = gtk4::Label::new(Some("Checking…"));
    status.set_hexpand(true);
    status.set_xalign(0.0);
    let refresh = gtk4::Button::with_label("Refresh");
    let details = gtk4::LinkButton::with_label(
        "https://github.com/manaflow-ai/cmux/blob/main/docs/local-tmux.md",
        "Details",
    );
    header.append(&status);
    header.append(&refresh);
    header.append(&details);
    content.append(&header);
    let error = gtk4::Label::new(None);
    error.set_wrap(true);
    error.set_xalign(0.0);

    // Upstream order: header, Start Persistent Session, status/error, then one row per session.
    let start_title = gtk4::Label::new(Some("Start Persistent Session"));
    start_title.set_xalign(0.0);
    content.append(&start_title);
    let start_help = gtk4::Label::new(Some("Creates a named local-tmux session in the selected workspace directory and attaches it to cmux."));
    start_help.set_wrap(true);
    start_help.set_xalign(0.0);
    content.append(&start_help);
    let start_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let name = gtk4::Entry::builder()
        .placeholder_text("Session name")
        .hexpand(true)
        .build();
    let start = gtk4::Button::with_label("Start");
    start.set_sensitive(false);
    name.connect_changed({
        let start = start.clone();
        move |name| start.set_sensitive(!name.text().trim().is_empty())
    });
    start.connect_clicked({
        let (state, dialog, error, name) =
            (state.clone(), dialog.clone(), error.clone(), name.clone());
        move |_| match run_in_new_tab(&state, "start", name.text().trim()) {
            Ok(()) => dialog.close(),
            Err(message) => error.set_text(message),
        }
    });
    start_row.append(&name);
    start_row.append(&start);
    content.append(&start_row);
    content.append(&error);
    let rows = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    content.append(&rows);

    let fill = {
        let (state, dialog, rows, status, error) = (
            state.clone(),
            dialog.clone(),
            rows.clone(),
            status.clone(),
            error.clone(),
        );
        move || {
            while let Some(child) = rows.first_child() {
                rows.remove(&child);
            }
            let Some(sessions) = sessions() else {
                status.set_text("Local sessions are unavailable (is tmux installed?).");
                return;
            };
            status.set_text(&if sessions.is_empty() {
                "No live sessions".to_owned()
            } else {
                format!("Live: {}", sessions.len())
            });
            for session in sessions {
                let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
                let text = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
                text.set_hexpand(true);
                let title = gtk4::Label::new(Some(&session.session_name));
                title.set_xalign(0.0);
                // Upstream subtitle: "Unmanaged · Clients: N · cwd" (no registry: always unmanaged).
                let mut parts = vec!["Unmanaged".to_owned()];
                if let Some(clients) = session.clients.filter(|clients| *clients > 0) {
                    parts.push(format!("Clients: {clients}"));
                }
                if let Some(cwd) = session.cwd.as_deref().filter(|cwd| !cwd.is_empty()) {
                    parts.push(cwd.to_owned());
                }
                let label = gtk4::Label::new(Some(&parts.join(" · ")));
                label.set_xalign(0.0);
                label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
                label.add_css_class("dim-label");
                text.append(&title);
                text.append(&label);
                let attach = gtk4::Button::with_label("Attach");
                attach.connect_clicked({
                    let (state, dialog, error) = (state.clone(), dialog.clone(), error.clone());
                    let name = session.session_name.clone();
                    move |_| {
                        if focus_existing(&state, &name) {
                            dialog.close();
                            return;
                        }
                        match run_in_new_tab(&state, "attach", &name) {
                            Ok(()) => dialog.close(),
                            Err(message) => error.set_text(message),
                        }
                    }
                });
                row.append(&text);
                row.append(&attach);
                rows.append(&row);
            }
        }
    };
    fill();
    refresh.connect_clicked(move |_| fill());
}
