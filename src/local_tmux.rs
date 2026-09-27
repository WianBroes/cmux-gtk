//! Local tmux persistence shared by the CLI and the restore policy.
//! Follows upstream `LocalTmuxCommandBuilder`: the only command cmux persists is an attach
//! guarded by the tmux server's in-memory identity, so a restarted server is never reattached.

pub const RESUME_KIND: &str = "local-tmux";
pub const SERVER_OPTION: &str = "@cmux_local_server_id";
pub const MISMATCH_COMMAND: &str = "run-shell false";

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The persisted reattach command. `env` clears an outer `TMUX` and carries the restore marker.
pub fn attach_command(tmux: &str, socket: &str, server_id: &str, session_id: &str) -> String {
    let condition = format!("#{{==:#{{{SERVER_OPTION}}},{server_id}}}");
    let action = ["attach-session", "-t", session_id]
        .map(shell_quote)
        .join(" ");
    format!(
        "/usr/bin/env TMUX= CMUX_LOCAL_TMUX=1 {} -S {} if-shell -F {} {} {}",
        shell_quote(tmux),
        shell_quote(socket),
        shell_quote(&condition),
        shell_quote(&action),
        shell_quote(MISMATCH_COMMAND)
    )
}

/// Session names reach tmux's target parser, so keep them to upstream's safe alphabet.
pub fn valid_name(name: &str) -> bool {
    (1..=128).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// tmux's immutable session identity, `$N`.
pub fn valid_session_id(id: &str) -> bool {
    id.len() >= 2 && id.starts_with('$') && id[1..].bytes().all(|b| b.is_ascii_digit())
}

/// Lowercase hyphenated UUID, as written into the server option.
pub fn valid_server_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
        })
}

/// Recognize exactly the attach command built above (literal words already parsed).
pub fn restorable(command: &str, words: &[String]) -> bool {
    let [env, tmux_var, marker, tmux, socket_flag, socket, if_shell, format_flag, condition, action, mismatch] =
        words
    else {
        return false;
    };
    let prefix = format!("#{{==:#{{{SERVER_OPTION}}},");
    let Some(server_id) = condition
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_suffix('}'))
    else {
        return false;
    };
    let Some(session_id) = action
        .strip_prefix("'attach-session' '-t' '")
        .and_then(|rest| rest.strip_suffix('\''))
    else {
        return false;
    };
    env == "/usr/bin/env"
        && tmux_var == "TMUX="
        && marker == "CMUX_LOCAL_TMUX=1"
        && tmux.starts_with('/')
        && socket_flag == "-S"
        && socket.starts_with('/')
        && socket.ends_with("/server.sock")
        && if_shell == "if-shell"
        && format_flag == "-F"
        && mismatch == MISMATCH_COMMAND
        && valid_server_id(server_id)
        && valid_session_id(session_id)
        && command == attach_command(tmux, socket, server_id, session_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Words as a POSIX shell would split the generated command.
    fn words(command: &str) -> Vec<String> {
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "for w in {command}; do printf '%s\\0' \"$w\"; done"
            ))
            .output()
            .unwrap();
        String::from_utf8(out.stdout)
            .unwrap()
            .split_terminator('\0')
            .map(str::to_owned)
            .collect()
    }

    /// The generated attach is recognized; any altered part is not.
    #[test]
    fn restorable_accepts_only_generated_attach() {
        let server = "0b8f3a52-8c1e-4a3e-9f0e-2d5c6b7a8e91";
        let command = attach_command(
            "/usr/bin/tmux",
            "/home/u/.local/state/cmux/local-tmux/server.sock",
            server,
            "$3",
        );
        assert!(restorable(&command, &words(&command)));
        for altered in [
            command.replace("'$3'", "'$3; rm'"),
            command.replace("/usr/bin/env", "/bin/sh"),
            command.replace("server.sock", "other.sock"),
            command.replace(server, "not-a-uuid"),
            command.replace("run-shell false", "run-shell true"),
            format!("{command} ; true"),
        ] {
            assert!(!restorable(&altered, &words(&altered)), "{altered}");
        }
        assert!(valid_name("work_1-a") && !valid_name("a:b") && !valid_name(""));
    }
}
