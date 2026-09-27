//! `cmux local-tmux`: processes owned by a private tmux server that outlives cmux.
//! Minimal port of upstream's local-tmux profile; see `docs/local-tmux.md` upstream.
use super::local_tmux_shared as shared;
use super::socket_client::SocketClient;
use super::CliError;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Live identity of one session on the profile server.
struct Binding {
    session_id: String,
    server_id: String,
    path: String,
}

/// One row of `list-sessions`.
struct Session {
    name: String,
    id: String,
    clients: u32,
    path: String,
}

struct Profile {
    tmux: String,
    socket: String,
}

fn error(message: impl Into<String>) -> CliError {
    CliError::Command(message.into())
}

impl Profile {
    /// One user-owned server under `~/.cmux/local-tmux` (or `CMUX_LOCAL_TMUX_STATE_DIR`), mode 0700.
    fn open() -> Result<Self, CliError> {
        let tmux = std::env::var_os("CMUX_LOCAL_TMUX_BIN")
            .map(PathBuf::from)
            .or_else(|| cmux_platform::paths::find_command_on_path("tmux"))
            .or_else(|| Some(PathBuf::from("/usr/bin/tmux")).filter(|path| path.is_file()))
            .and_then(|path| std::path::absolute(path).ok())
            .ok_or_else(|| error("local-tmux requires tmux on PATH (or CMUX_LOCAL_TMUX_BIN)"))?;
        let state = std::env::var_os("CMUX_LOCAL_TMUX_STATE_DIR")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| Path::new(&home).join(".cmux/local-tmux"))
            })
            .and_then(|dir| std::path::absolute(dir).ok())
            .ok_or_else(|| error("cannot resolve the local-tmux state directory"))?;
        let socket = state.join("server.sock").to_string_lossy().into_owned();
        if socket.len() >= 100 {
            return Err(error(format!(
                "local-tmux state directory is too long for a Unix socket: {socket}"
            )));
        }
        cmux_platform::filesystem::create_private_directory(&state)
            .map_err(|e| error(e.to_string()))?;
        let profile = Self {
            tmux: tmux.to_string_lossy().into_owned(),
            socket,
        };
        profile.validate_socket(&state)?;
        Ok(profile)
    }

    /// Refuse a socket left by another user or open to group/world (the private state
    /// directory, just created or checked by us, gives the owner to compare with).
    fn validate_socket(&self, state: &Path) -> Result<(), CliError> {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let Ok(info) = std::fs::symlink_metadata(&self.socket) else {
            return Ok(());
        };
        let owner = std::fs::metadata(state).map(|dir| dir.uid()).ok();
        if !info.file_type().is_socket() || info.mode() & 0o077 != 0 || Some(info.uid()) != owner {
            return Err(error(format!(
                "insecure local-tmux socket: {}",
                self.socket
            )));
        }
        Ok(())
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(&self.tmux);
        without_cmux_identity(&mut command)
            .arg("-S")
            .arg(&self.socket)
            .args(arguments);
        command
    }

    /// Run tmux and return stdout, turning a failure into its stderr.
    fn run(&self, arguments: &[&str]) -> Result<String, CliError> {
        let output = self
            .command(arguments)
            .output()
            .map_err(|e| error(format!("tmux: {e}")))?;
        if !output.status.success() {
            return Err(error(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Give a fresh server its identity once (`-o` keeps an existing one) and keep it alive
    /// without clients even if the user's config sets `exit-unattached`.
    fn ensure_identity(&self) -> Result<(), CliError> {
        let candidate = uuid::Uuid::new_v4().to_string();
        self.run(&[
            "set-option",
            "-soq",
            shared::SERVER_OPTION,
            &candidate,
            ";",
            "set-option",
            "-s",
            "exit-unattached",
            "off",
        ])
        .map(drop)
    }

    /// Resolve a name to its immutable session id, exact-match only (`=name`).
    fn binding(&self, name: &str) -> Result<Binding, CliError> {
        let target = format!("={name}:");
        let format = format!(
            "#{{session_id}}\t#{{{}}}\t#{{session_path}}",
            shared::SERVER_OPTION
        );
        let line = self
            .run(&["display-message", "-p", "-t", &target, &format])
            .map_err(|_| error(format!("no local-tmux session named {name}")))?;
        let mut fields = line.trim_end_matches('\n').splitn(3, '\t');
        let (Some(session_id), Some(server_id), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            return Err(error("unexpected tmux output"));
        };
        if !shared::valid_session_id(session_id) || !shared::valid_server_id(server_id) {
            return Err(error("local-tmux server has no cmux identity"));
        }
        Ok(Binding {
            session_id: session_id.into(),
            server_id: server_id.into(),
            path: path.into(),
        })
    }

    /// Live sessions on the profile server; no server yet means none.
    fn sessions(&self) -> Vec<Session> {
        let format = "#{session_name}\t#{session_id}\t#{session_attached}\t#{session_path}";
        let Ok(listing) = self.run(&["list-sessions", "-F", format]) else {
            return Vec::new();
        };
        listing
            .lines()
            .filter_map(|line| {
                let mut fields = line.splitn(4, '\t');
                Some(Session {
                    name: fields.next()?.into(),
                    id: fields.next()?.into(),
                    clients: fields.next()?.parse().ok()?,
                    path: fields.next()?.into(),
                })
            })
            .collect()
    }

    /// Commands for one session, executed only if the server is still the same incarnation.
    fn guarded(&self, binding: &Binding, action: &[&str]) -> Result<(), CliError> {
        let condition = format!(
            "#{{==:#{{{}}},{}}}",
            shared::SERVER_OPTION,
            binding.server_id
        );
        let action = action
            .iter()
            .map(|argument| shared::shell_quote(argument))
            .collect::<Vec<_>>()
            .join(" ");
        self.run(&[
            "if-shell",
            "-F",
            &condition,
            &action,
            shared::MISMATCH_COMMAND,
        ])
        .map(drop)
    }
}

/// Like upstream: the detached server and its clients must not inherit this pane's
/// surface/workspace identity or socket credentials, so agent hooks inside tmux stay inert.
fn without_cmux_identity(command: &mut Command) -> &mut Command {
    command.env_remove("TMUX");
    for (key, _) in std::env::vars_os() {
        let key = key.to_string_lossy();
        if key.starts_with("CMUX_") || key.starts_with("CMUXD_") {
            command.env_remove(key.as_ref());
        }
    }
    command
}

fn checked_name(name: &str) -> Result<&str, CliError> {
    if shared::valid_name(name) {
        Ok(name)
    } else {
        Err(error(
            "local-tmux session names must contain only letters, numbers, underscore, or dash (1-128 characters)",
        ))
    }
}

/// Attach in place: inside a cmux terminal, record the guarded attach as this surface's resume
/// binding first so cmux reattaches after a restart; then replace this CLI with the tmux client.
fn attach(
    profile: &Profile,
    binding: &Binding,
    socket: Option<&str>,
    headless: bool,
) -> Result<(), CliError> {
    let command = shared::attach_command(
        &profile.tmux,
        &profile.socket,
        &binding.server_id,
        &binding.session_id,
    );
    if let (false, Ok(surface)) = (headless, std::env::var("CMUX_SURFACE_ID")) {
        let socket_path = socket
            .map(str::to_owned)
            .or_else(cmux_platform::discovery::discover_socket)
            .ok_or_else(|| error("no cmux socket found (is cmux-app running?)"))?;
        let mut client = SocketClient::connect(&socket_path, Duration::from_secs(5))?;
        client.call(
            "surface.resume.set",
            json!({"surface_id": surface, "kind": shared::RESUME_KIND,
                "name": format!("tmux {}", binding.session_id),
                "command": command, "checkpoint_id": binding.session_id,
                "cwd": binding.path, "environment": {}}),
        )?;
    }
    let mut client = Command::new("/bin/sh");
    without_cmux_identity(&mut client).arg("-c").arg(&command);
    Err(error(format!(
        "tmux attach failed: {}",
        cmux_platform::process::replace_current(&mut client)
    )))
}

pub fn run(command: &super::args::LocalTmuxCommands, socket: Option<&str>) -> Result<(), CliError> {
    use super::args::LocalTmuxCommands as C;
    let profile = Profile::open()?;
    match command {
        C::Start {
            name,
            cwd,
            command,
            detached,
        } => {
            let name = checked_name(name)?;
            let requested = cwd
                .as_deref()
                .map(|cwd| {
                    std::fs::canonicalize(cwd).map_err(|e| error(format!("{}: {e}", cwd.display())))
                })
                .transpose()?;
            // Like upstream: an existing session is reused, never recreated or retargeted.
            if let Ok(binding) = profile.binding(name) {
                if requested
                    .as_deref()
                    .is_some_and(|cwd| cwd != Path::new(&binding.path))
                {
                    return Err(error("local-tmux session already exists with a different working directory; use attach or close it first"));
                }
                if command.as_deref().is_some_and(|c| !c.trim().is_empty()) {
                    return Err(error("local-tmux session already exists; use attach or close it before supplying a new command"));
                }
                if *detached {
                    return Ok(());
                }
                return attach(&profile, &binding, socket, false);
            }
            let cwd = match requested {
                Some(cwd) => cwd,
                None => std::env::current_dir().map_err(|e| error(e.to_string()))?,
            };
            if !cwd.is_dir() {
                return Err(error(format!("not a directory: {}", cwd.display())));
            }
            let cwd = cwd.to_string_lossy();
            let mut arguments = vec![
                "set-option",
                "-s",
                "exit-unattached",
                "off",
                ";",
                "new-session",
                "-d",
                "-s",
                name,
                "-c",
                &cwd,
            ];
            if let Some(command) = command.as_deref().filter(|c| !c.trim().is_empty()) {
                arguments.extend(["/bin/sh", "-lc", command]);
            }
            profile.run(&arguments)?;
            profile.ensure_identity()?;
            let binding = profile.binding(name)?;
            // tmux owns scrollback for this profile; keep a useful bounded history.
            let _ = profile.guarded(
                &binding,
                &[
                    "set-window-option",
                    "-t",
                    &binding.session_id,
                    "history-limit",
                    "10000",
                ],
            );
            if *detached {
                println!("Started local-tmux session {name}");
                return Ok(());
            }
            attach(&profile, &binding, socket, false)
        }
        C::Attach { name, headless } => {
            let binding = profile.binding(checked_name(name)?)?;
            attach(&profile, &binding, socket, *headless)
        }
        C::List { json } => {
            let sessions = profile.sessions();
            if *json {
                // Upstream's `list --json` contract; without a registry every row is unmanaged.
                let rows: Vec<_> = sessions
                    .iter()
                    .map(|s| {
                        json!({"id": null, "session_name": s.name, "cwd": s.path,
                            "clients": s.clients, "managed": false, "live": true})
                    })
                    .collect();
                println!("{}", json!({ "sessions": rows }));
            } else if sessions.is_empty() {
                println!("No local-tmux sessions");
            } else {
                for s in &sessions {
                    println!("{}\t{} attached\t{}", s.name, s.clients, s.path);
                }
            }
            Ok(())
        }
        C::Status { name, json } => {
            let name = checked_name(name)?;
            let session = profile
                .sessions()
                .into_iter()
                .find(|s| s.name == name)
                .ok_or_else(|| error(format!("no local-tmux session named {name}")))?;
            if *json {
                println!(
                    "{}",
                    json!({"session_name": session.name, "tmux_session_id": session.id,
                        "cwd": session.path, "clients": session.clients, "live": true,
                        "socket_path": profile.socket})
                );
            } else {
                println!(
                    "{}\t{}\t{} attached\t{}",
                    session.name, session.id, session.clients, session.path
                );
            }
            Ok(())
        }
        C::Detach { name } => {
            let binding = profile.binding(checked_name(name)?)?;
            profile.guarded(&binding, &["detach-client", "-s", &binding.session_id])
        }
        C::Close { name } => {
            let binding = profile.binding(checked_name(name)?)?;
            profile.guarded(&binding, &["kill-session", "-t", &binding.session_id])
        }
    }
}
