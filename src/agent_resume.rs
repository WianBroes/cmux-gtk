//! Resume command shapes written by the built-in agent hooks, trusted for automatic restore.
//! Mirrors the providers in `cli/hooks.rs`; a test there keeps both tables aligned.

/// One hook-written resume shape: `<absolute path to binary> <prefix...> <session id>`.
pub struct AgentResume {
    pub kind: &'static str,
    pub binary: &'static str,
    pub prefix: &'static [&'static str],
    pub environment: &'static [&'static str],
}

pub const AGENTS: &[AgentResume] = &[
    AgentResume {
        kind: "claude",
        binary: "claude",
        prefix: &["--resume"],
        environment: &["CLAUDE_CONFIG_DIR", "CLAUDE_SECURESTORAGE_CONFIG_DIR"],
    },
    AgentResume {
        kind: "codex",
        binary: "codex",
        prefix: &["resume"],
        environment: &["CODEX_HOME"],
    },
    AgentResume {
        kind: "grok",
        binary: "grok",
        prefix: &["-r"],
        environment: &["GROK_HOME"],
    },
    AgentResume {
        kind: "gemini",
        binary: "gemini",
        prefix: &["--resume"],
        environment: &[],
    },
    AgentResume {
        kind: "kiro",
        binary: "kiro-cli",
        prefix: &["chat", "--resume-id"],
        environment: &["KIRO_HOME"],
    },
    AgentResume {
        kind: "antigravity",
        binary: "agy",
        prefix: &["--conversation"],
        environment: &[],
    },
    AgentResume {
        kind: "hermes-agent",
        binary: "hermes",
        prefix: &["--resume"],
        environment: &["HERMES_HOME"],
    },
    AgentResume {
        kind: "kimi",
        binary: "kimi",
        prefix: &["--resume"],
        environment: &["KIMI_CODE_HOME", "KIMI_SHARE_DIR"],
    },
    AgentResume {
        kind: "copilot",
        binary: "copilot",
        prefix: &["--resume"],
        environment: &["COPILOT_HOME"],
    },
    AgentResume {
        kind: "codebuddy",
        binary: "codebuddy",
        prefix: &["--resume"],
        environment: &["CODEBUDDY_CONFIG_DIR"],
    },
    AgentResume {
        kind: "factory",
        binary: "droid",
        prefix: &["--resume"],
        environment: &[],
    },
    AgentResume {
        kind: "qoder",
        binary: "qodercli",
        prefix: &["--resume"],
        environment: &["QODER_CONFIG_DIR"],
    },
    AgentResume {
        kind: "opencode",
        binary: "opencode",
        prefix: &["--session"],
        environment: &["OPENCODE_CONFIG_DIR"],
    },
    AgentResume {
        kind: "cursor",
        binary: "cursor-agent",
        prefix: &["--resume"],
        environment: &[],
    },
    AgentResume {
        kind: "pi",
        binary: "pi",
        prefix: &["--session"],
        environment: &["PI_CODING_AGENT_DIR"],
    },
    AgentResume {
        kind: "omp",
        binary: "omp",
        prefix: &["--session"],
        environment: &["PI_CODING_AGENT_DIR", "PI_CONFIG_DIR"],
    },
    AgentResume {
        kind: "campfire",
        binary: "campfire",
        prefix: &["--session"],
        environment: &["CAMPFIRE_CODING_AGENT_DIR"],
    },
    AgentResume {
        kind: "amp",
        binary: "amp",
        prefix: &["threads", "continue"],
        environment: &[],
    },
    AgentResume {
        kind: "rovodev",
        binary: "acli",
        prefix: &["rovodev", "run", "--restore"],
        environment: &["CMUX_ROVODEV_SESSIONS_DIR"],
    },
];

/// Whether literal arguments and environment keys match exactly what a built-in hook writes.
/// The session id must be a plain token so no option or path can ride along with it.
pub fn trusted<'a>(
    kind: &str,
    arguments: &[String],
    mut environment: impl Iterator<Item = &'a String>,
) -> bool {
    let Some(agent) = AGENTS.iter().find(|agent| agent.kind == kind) else {
        return false;
    };
    let [binary, rest @ ..] = arguments else {
        return false;
    };
    let Some((id, prefix)) = rest.split_last() else {
        return false;
    };
    let binary = std::path::Path::new(binary);
    binary.is_absolute()
        && binary.file_name().and_then(|name| name.to_str()) == Some(agent.binary)
        && prefix
            .iter()
            .map(String::as_str)
            .eq(agent.prefix.iter().copied())
        && !id.is_empty()
        && id.len() <= 256
        && !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
        && environment.all(|key| agent.environment.contains(&key.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    /// Only the exact hook shape is trusted; any extra, missing or altered argument is not.
    #[test]
    fn trusts_only_hook_shapes() {
        let none = Vec::<String>::new();
        let id = "a9eca671-8619-400a-9988-ebc317914eca";
        assert!(trusted(
            "claude",
            &args(&["/usr/bin/claude", "--resume", id]),
            none.iter()
        ));
        assert!(trusted(
            "amp",
            &args(&["/bin/amp", "threads", "continue", "T-1"]),
            none.iter()
        ));
        for bad in [
            args(&["claude", "--resume", id]),
            args(&["/tmp/sh", "--resume", id]),
            args(&["/usr/bin/claude", "--resume"]),
            args(&["/usr/bin/claude", "--resume", id, "--extra"]),
            args(&["/usr/bin/claude", "--continue", id]),
            args(&[
                "/usr/bin/claude",
                "--resume",
                "--dangerously-skip-permissions",
            ]),
            args(&["/usr/bin/claude", "--resume", "../x;y"]),
        ] {
            assert!(!trusted("claude", &bad, none.iter()), "{bad:?}");
        }
        assert!(!trusted(
            "tmux",
            &args(&["/usr/bin/tmux", "attach"]),
            none.iter()
        ));
        let env = vec!["CLAUDE_CONFIG_DIR".to_string()];
        assert!(trusted(
            "claude",
            &args(&["/usr/bin/claude", "--resume", id]),
            env.iter()
        ));
        let env = vec!["LD_PRELOAD".to_string()];
        assert!(!trusted(
            "claude",
            &args(&["/usr/bin/claude", "--resume", id]),
            env.iter()
        ));
    }
}
