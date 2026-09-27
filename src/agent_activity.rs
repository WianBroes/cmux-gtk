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

/// Whether a native hook event starts (`Some(true)`) or ends (`Some(false)`) a running turn;
/// event sets from upstream's `AgentSemanticEventMapper` and `AgentLifecycleReducer`.
pub fn running(event: &str) -> Option<bool> {
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
    const ENDS: &[&str] = &[
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
        // Waiting for the user: approvals, and Claude-style attention notifications.
        "permissionrequest",
        "permissionasked",
        "preapprovalrequest",
        "ontoolpermission",
        "notification",
        // Errors and session end.
        "stopfailure",
        "onerror",
        "error",
        "posttoolusefailure",
        "sessionend",
        "onsessionend",
        "onsessionfinalize",
        "sessionshutdown",
    ];
    let key = semantic_key(event);
    if STARTS.contains(&key.as_str()) {
        Some(true)
    } else if ENDS.contains(&key.as_str()) {
        Some(false)
    } else {
        None
    }
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
            let Some(running) = event.name.strip_prefix("agent.hook.").and_then(running) else {
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
            state
                .borrow_mut()
                .set_agent_running(surface, source, running);
        }
    });
    window.connect_destroy(move |_| task.abort());
}

#[cfg(test)]
mod tests {
    use super::running;

    #[test]
    /// Native names from each hook family map like upstream, ignoring case and separators.
    fn turn_boundaries() {
        for start in [
            "UserPromptSubmit",
            "before_agent_start",
            "BeforeSubmitPrompt",
        ] {
            assert_eq!(running(start), Some(true), "{start}");
        }
        for end in [
            "Stop",
            "agent_end",
            "session.idle",
            "Notification",
            "SessionEnd",
        ] {
            assert_eq!(running(end), Some(false), "{end}");
        }
        for other in ["SessionStart", "PreToolUse", "SubagentStop"] {
            assert_eq!(running(other), None, "{other}");
        }
    }
}
