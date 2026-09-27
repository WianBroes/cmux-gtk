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

struct Profile {
    tmux: String,
    socket: String,
}

fn error(message: impl Into<String>) -> CliError {
    CliError::Command(message.into())
}

impl Profile {
    /// One user-owned server under `$XDG_STATE_HOME/cmux/local-tmux`, directory mode 0700.
    fn open() -> Result<Self, CliError> {
        let tmux = std::env::var_os("CMUX_LOCAL_TMUX_BIN")
            .map(PathBuf::from)
            .or_else(|| cmux_platform::paths::find_command_on_path("tmux"))
            .or_else(|| Some(PathBuf::from("/usr/bin/tmux")).filter(|path| path.is_file()))
            .and_then(|path| std::path::absolute(path).ok())
            .ok_or_else(|| error("local-tmux requires tmux on PATH (or CMUX_LOCAL_TMUX_BIN)"))?;
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".local/state")))
            .ok_or_else(|| error("cannot resolve the state directory"))?
            .join("cmux/local-tmux");
        cmux_platform::filesystem::create_private_directory(&state)
            .map_err(|e| error(e.to_string()))?;
        Ok(Self {
            tmux: tmux.to_string_lossy().into_owned(),
            socket: state.join("server.sock").to_string_lossy().into_owned(),
        })
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(&self.tmux);
        command
            .env_remove("TMUX")
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
    client.arg("-c").arg(&command);
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
            let cwd = match cwd {
                Some(cwd) => std::path::absolute(cwd).map_err(|e| error(e.to_string()))?,
                None => std::env::current_dir().map_err(|e| error(e.to_string()))?,
            };
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
            profile.guarded(
                &binding,
                &[
                    "set-option",
                    "-t",
                    &binding.session_id,
                    "history-limit",
                    "10000",
                ],
            )?;
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
        C::List => {
            let format = "#{session_name}\t#{session_windows} windows\t#{session_attached} attached\t#{session_path}";
            match profile.run(&["list-sessions", "-F", format]) {
                Ok(listing) => print!("{listing}"),
                // No server yet means no sessions, not an error.
                Err(_) => println!("No local-tmux sessions"),
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
