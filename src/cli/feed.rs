//! Claude Code's Feed bridge (upstream `docs/feed.md`): a `PermissionRequest` hook parks its
//! request in the app and prints the human's decision in Claude's hook format. No decision
//! (timeout, "In Terminal", app unavailable) prints nothing, so Claude shows its own prompt.
use super::socket_client::SocketClient;
use super::CliError;
use serde_json::{json, Value};

/// Matches the app's soft wait; the CLI allows a few extra seconds for the response.
pub const WAIT_SECONDS: u64 = 120;

pub fn claude_permission(
    client: &mut SocketClient,
    payload: &Value,
    session: &str,
    surface: &str,
) -> Result<(), CliError> {
    let tool = payload["tool_name"].as_str().unwrap_or_default();
    let kind = match tool {
        "AskUserQuestion" => "question",
        "ExitPlanMode" => "plan",
        _ => "permission",
    };
    let detail = match &payload["tool_input"] {
        Value::Object(input) => Value::Object(input.clone()),
        _ => json!({}),
    };
    let result = client.call(
        "feed.push",
        json!({"kind": kind, "source": "claude", "tool_name": tool, "detail": detail,
            "surface_id": surface, "session_id": session}),
    );
    // The Feed is advisory: any failure leaves Claude's own prompt in charge.
    let Ok(result) = result else {
        return Ok(());
    };
    if let Some(decision) = claude_decision(payload, &result["decision"]) {
        let output = json!({"hookSpecificOutput": {
            "hookEventName": "PermissionRequest", "decision": decision}});
        println!("{output}");
    }
    Ok(())
}

/// Translate a Feed choice into Claude's `PermissionRequest` decision object.
pub fn claude_decision(payload: &Value, decision: &Value) -> Option<Value> {
    let suggestions = payload["permission_suggestions"]
        .as_array()
        .filter(|suggestions| !suggestions.is_empty());
    match decision["choice"].as_str()? {
        "once" | "manual" => Some(json!({"behavior": "allow"})),
        // Upstream "Always": apply the rule Claude suggested, when it suggested one.
        "always" => Some(match suggestions {
            Some(suggestions) => json!({"behavior": "allow", "updatedPermissions": suggestions}),
            None => json!({"behavior": "allow"}),
        }),
        "auto" => Some(json!({"behavior": "allow", "updatedPermissions": [
            {"type": "setMode", "mode": "auto", "destination": "session"}]})),
        "deny" => Some(json!({"behavior": "deny", "message": "Denied from the cmux Feed."})),
        // AskUserQuestion: echo the questions and add the chosen labels.
        "submit" => {
            let mut input = payload["tool_input"].clone();
            input.as_object_mut()?.insert("answers".into(), decision["answers"].clone());
            Some(json!({"behavior": "allow", "updatedInput": input}))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_map_to_claude_decisions() {
        let bash = json!({"tool_name": "Bash", "tool_input": {"command": "ls"},
            "permission_suggestions": [{"type": "addRules", "behavior": "allow",
                "destination": "localSettings", "rules": [{"toolName": "Bash", "ruleContent": "ls"}]}]});
        assert_eq!(claude_decision(&bash, &json!({"choice": "once"})), Some(json!({"behavior": "allow"})));
        let always = claude_decision(&bash, &json!({"choice": "always"})).unwrap();
        assert_eq!(always["updatedPermissions"][0]["type"], "addRules");
        assert_eq!(claude_decision(&bash, &json!({"choice": "deny"})).unwrap()["behavior"], "deny");
        assert_eq!(claude_decision(&bash, &Value::Null), None);
        let auto = claude_decision(&bash, &json!({"choice": "auto"})).unwrap();
        assert_eq!(auto["updatedPermissions"][0]["mode"], "auto");
    }

    #[test]
    fn question_answers_echo_the_input() {
        let ask = json!({"tool_name": "AskUserQuestion", "tool_input": {"questions": [
            {"question": "Which?", "options": [{"label": "A"}, {"label": "B"}]}]}});
        let decision = claude_decision(&ask, &json!({"choice": "submit", "answers": {"Which?": "B"}})).unwrap();
        assert_eq!(decision["updatedInput"]["answers"]["Which?"], "B");
        assert_eq!(decision["updatedInput"]["questions"][0]["question"], "Which?");
    }
}
