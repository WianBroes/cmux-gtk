//! Sidebar agent activity, as upstream's lifecycle reducer: a turn start marks the terminal's
//! agent running; a turn end, permission request, error or session end stops it.

use gtk4::glib;
use gtk4::prelude::*;
use std::rc::Rc;

/// Upstream `AgentSemanticEventMapper.semanticKey`: lowercase ASCII letters and digits only.
fn semantic_key(raw: &str) -> String {
    raw.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// One agent's state in its terminal, ordered by urgency (upstream `setClaudeStatus` values).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AgentState {
    Idle,
    Running,
    NeedsInput,
}

/// The state a native hook event moves its agent to; `Some(None)` ends the agent's session.
/// Event sets from upstream's `AgentSemanticEventMapper` and `AgentLifecycleReducer`.
pub fn state(event: &str) -> Option<Option<AgentState>> {
    const STARTS: &[&str] = &[
        "userpromptsubmit",
        "beforesubmitprompt",
        "beforeagent",
        "prellmcall",
        "preinvocation",
        "agentstart",
        "turnstart",
        "beforeagentstart",
    ];
    const IDLE: &[&str] = &[
        "sessionstart",
        // Turn completion (OpenCode reports it as session idle).
        "stop",
        "afteragent",
        "afteragentresponse",
        "postllmcall",
        "oncomplete",
        "turncompletion",
        "agentend",
        "taskcompleted",
        "turnend",
        "agentsettled",
        "sessionidle",
        // Errors.
        "stopfailure",
        "onerror",
        "error",
        "posttoolusefailure",
    ];
    // Waiting for the user: approvals, and Claude-style attention notifications.
    const NEEDS_INPUT: &[&str] = &[
        "permissionrequest",
        "permissionasked",
        "preapprovalrequest",
        "ontoolpermission",
        "notification",
    ];
    const ENDS: &[&str] = &[
        "sessionend",
        "onsessionend",
        "onsessionfinalize",
        "sessionshutdown",
    ];
    let key = semantic_key(event);
    let key = key.as_str();
    if STARTS.contains(&key) {
        Some(Some(AgentState::Running))
    } else if IDLE.contains(&key) {
        Some(Some(AgentState::Idle))
    } else if NEEDS_INPUT.contains(&key) {
        Some(Some(AgentState::NeedsInput))
    } else if ENDS.contains(&key) {
        Some(None)
    } else {
        None
    }
}

/// Upstream status key per agent (`claude_code` for Claude); other sources keep their own name.
fn status_key(source: &str) -> String {
    match source {
        "claude" => "claude_code".into(),
        other => other.into(),
    }
}

/// One status line per agent kind, from its most urgent live terminal, as upstream's
/// `mostUrgentPanelStatusEntry`: Running (bolt, blue), Needs input (bell, blue), Idle (pause, gray).
pub fn status_entries<'a>(
    agents: impl IntoIterator<Item = (&'a (String, String), &'a AgentState)>,
) -> std::collections::BTreeMap<String, crate::workspace_metadata::Status> {
    let mut urgent = std::collections::BTreeMap::<String, AgentState>::new();
    for ((_, source), state) in agents {
        let entry = urgent.entry(status_key(source)).or_insert(*state);
        *entry = (*entry).max(*state);
    }
    urgent
        .into_iter()
        .map(|(key, state)| {
            let (value, icon, color) = match state {
                AgentState::Running => ("Running", "thunderbolt-symbolic", "#4C8DFF"),
                AgentState::NeedsInput => (
                    "Needs input",
                    "preferences-system-notifications-symbolic",
                    "#4C8DFF",
                ),
                AgentState::Idle => ("Idle", "media-playback-pause-symbolic", "#8E8E93"),
            };
            let status = crate::workspace_metadata::Status {
                value: value.into(),
                icon: Some(icon.into()),
                color: Some(color.into()),
                priority: 0,
                format: Default::default(),
                url: None,
            };
            (key, status)
        })
        .collect()
}

/// Follow `agent.hook.*` events on the process bus and update the owning workspace's spinner.
pub fn start(state: &crate::app_state::AppStateRef, window: &gtk4::ApplicationWindow) {
    let filter = crate::events::Filter {
        names: Vec::new(),
        categories: vec!["agent".into()],
    };
    let (_, _, mut receiver) = crate::events::subscribe(None, &filter, false);
    let weak = Rc::downgrade(state);
    let task = glib::MainContext::default().spawn_local(async move {
        loop {
            let event = match receiver.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            let Some(next) = event
                .name
                .strip_prefix("agent.hook.")
                .and_then(crate::agent_activity::state)
            else {
                continue;
            };
            let Ok(frame) = serde_json::from_str::<serde_json::Value>(&event.line) else {
                continue;
            };
            let (Some(surface), Some(source)) =
                (frame["surface_id"].as_str(), frame["source"].as_str())
            else {
                continue;
            };
            let Some(state) = weak.upgrade() else {
                break;
            };
            state.borrow_mut().set_agent_state(surface, source, next);
        }
    });
    window.connect_destroy(move |_| task.abort());
}

#[cfg(test)]
mod tests {
    use super::{state, status_entries, AgentState};

    #[test]
    /// Native names from each hook family map like upstream, ignoring case and separators.
    fn hook_events_move_agent_state() {
        for start in [
            "UserPromptSubmit",
            "before_agent_start",
            "BeforeSubmitPrompt",
        ] {
            assert_eq!(state(start), Some(Some(AgentState::Running)), "{start}");
        }
        for idle in [
            "SessionStart",
            "Stop",
            "agent_end",
            "session.idle",
            "StopFailure",
        ] {
            assert_eq!(state(idle), Some(Some(AgentState::Idle)), "{idle}");
        }
        for waits in ["Notification", "PermissionRequest"] {
            assert_eq!(state(waits), Some(Some(AgentState::NeedsInput)), "{waits}");
        }
        assert_eq!(state("SessionEnd"), Some(None));
        for other in ["PreToolUse", "SubagentStop"] {
            assert_eq!(state(other), None, "{other}");
        }
    }

    #[test]
    /// Two Claude terminals give one `claude_code` line showing the most urgent; pi gets its own.
    fn one_line_per_agent_kind_most_urgent_wins() {
        let agents = std::collections::HashMap::from([
            (("a".to_owned(), "claude".to_owned()), AgentState::Idle),
            (
                ("b".to_owned(), "claude".to_owned()),
                AgentState::NeedsInput,
            ),
            (("c".to_owned(), "pi".to_owned()), AgentState::Running),
        ]);
        let lines = status_entries(&agents);
        let summary: Vec<_> = lines
            .iter()
            .map(|(key, status)| (key.as_str(), status.value.as_str()))
            .collect();
        assert_eq!(summary, [("claude_code", "Needs input"), ("pi", "Running")]);
    }
}
