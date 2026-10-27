use serde_json::{json, Map, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use tokio::sync::oneshot;
use uuid::Uuid;

const PREVIEW_LIMIT: usize = 8_000;
const STOP_REASON_LIMIT: usize = 2_000;
const CONTEXT_LIMIT: usize = 2_048;
const TEXT_PAGE_CHARS: usize = 16_000;

#[derive(Clone, Default)]
struct Context {
    last_user_message: Option<String>,
    assistant_preamble: Option<String>,
}

struct Item {
    id: String,
    workstream_id: String,
    source: String,
    kind: String,
    status: String,
    created_at_ms: u64,
    updated_at_ms: u64,
    cwd: Option<String>,
    title: Option<String>,
    workspace_id: Option<String>,
    surface_id: Option<String>,
    text: Option<String>,
    reason: Option<String>,
    tool_name: Option<String>,
    tool_input: Option<String>,
    tool_result: Option<String>,
    tool_result_is_error: Option<bool>,
    context: Context,
    full_text: String,
    /// Blocking decisions (`permissionRequest`, `exitPlan`, `question`) only.
    request_id: Option<String>,
    tool_input_value: Option<Value>,
    decision: Option<Value>,
    resolved_at_ms: Option<u64>,
}

impl Item {
    fn is_visible(&self) -> bool {
        match self.kind.as_str() {
            "permissionRequest" | "exitPlan" | "question" => true,
            "assistantMessage" => self
                .text
                .as_deref()
                .map(|text| !text.is_empty())
                .unwrap_or(false),
            "stop" => {
                self.reason
                    .as_deref()
                    .map(|reason| !reason.is_empty())
                    .unwrap_or(false)
                    || self
                        .context
                        .last_user_message
                        .as_deref()
                        .map(|text| !text.is_empty())
                        .unwrap_or(false)
                    || self
                        .context
                        .assistant_preamble
                        .as_deref()
                        .map(|text| !text.is_empty())
                        .unwrap_or(false)
            }
            "toolResult" => {
                self.tool_result_is_error == Some(true)
                    && (self
                        .tool_name
                        .as_deref()
                        .map(|name| !name.is_empty())
                        .unwrap_or(false)
                        || self
                            .tool_result
                            .as_deref()
                            .map(|result| !result.is_empty())
                            .unwrap_or(false))
            }
            _ => false,
        }
    }

    fn to_json(&self) -> Value {
        let mut row = Map::new();
        row.insert("id".to_string(), json!(self.id));
        row.insert("event_id".to_string(), json!(self.id));
        row.insert(
            "workstream_id".to_string(),
            json!(self.workstream_id),
        );
        row.insert("source".to_string(), json!(self.source));
        row.insert("kind".to_string(), json!(self.kind));
        row.insert("status".to_string(), json!(self.status));
        row.insert(
            "created_at".to_string(),
            json!(iso8601_utc(self.created_at_ms)),
        );
        row.insert(
            "updated_at".to_string(),
            json!(iso8601_utc(self.updated_at_ms)),
        );

        insert_optional(&mut row, "cwd", self.cwd.as_deref());
        insert_optional(&mut row, "title", self.title.as_deref());
        insert_optional(&mut row, "workspace_id", self.workspace_id.as_deref());
        insert_optional(&mut row, "surface_id", self.surface_id.as_deref());

        match self.kind.as_str() {
            "userPrompt" | "assistantMessage" => {
                insert_truncated(&mut row, "text", self.text.as_deref(), PREVIEW_LIMIT);
            }
            "stop" => {
                insert_truncated(
                    &mut row,
                    "reason",
                    self.reason.as_deref(),
                    STOP_REASON_LIMIT,
                );
            }
            "toolUse" => {
                insert_optional(&mut row, "tool_name", self.tool_name.as_deref());
                insert_truncated(
                    &mut row,
                    "tool_input",
                    self.tool_input.as_deref(),
                    PREVIEW_LIMIT,
                );
            }
            "toolResult" => {
                insert_optional(&mut row, "tool_name", self.tool_name.as_deref());
                insert_truncated(
                    &mut row,
                    "tool_result",
                    self.tool_result.as_deref(),
                    PREVIEW_LIMIT,
                );
                if let Some(is_error) = self.tool_result_is_error {
                    row.insert("tool_result_is_error".to_string(), json!(is_error));
                }
            }
            "permissionRequest" => {
                insert_optional(&mut row, "request_id", self.request_id.as_deref());
                insert_optional(&mut row, "tool_name", self.tool_name.as_deref());
                insert_truncated(
                    &mut row,
                    "tool_input",
                    self.tool_input.as_deref(),
                    PREVIEW_LIMIT,
                );
            }
            "exitPlan" => {
                insert_optional(&mut row, "request_id", self.request_id.as_deref());
                let plan = self
                    .tool_input_value
                    .as_ref()
                    .and_then(|input| input.get("plan"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                insert_truncated(&mut row, "plan", Some(plan), PREVIEW_LIMIT);
                if let Some(summary) = plan.lines().map(str::trim).find(|line| !line.is_empty()) {
                    row.insert("plan_summary".to_string(), json!(summary));
                }
                row.insert("default_mode".to_string(), json!("manual"));
            }
            "question" => {
                insert_optional(&mut row, "request_id", self.request_id.as_deref());
                let questions = question_prompts(self.tool_input_value.as_ref());
                if let Some(first) = questions.first() {
                    row.insert("question_prompt".to_string(), first["prompt"].clone());
                    row.insert(
                        "question_multi_select".to_string(),
                        first["multi_select"].clone(),
                    );
                    row.insert("question_options".to_string(), first["options"].clone());
                }
                row.insert("questions".to_string(), Value::Array(questions));
            }
            _ => {}
        }
        if let Some(decision) = &self.decision {
            row.insert("decision".to_string(), decision.clone());
        }
        if let Some(at) = self.resolved_at_ms {
            row.insert("resolved_at".to_string(), json!(iso8601_utc(at)));
        }

        let mut context = Map::new();
        insert_truncated(
            &mut context,
            "last_user_message",
            self.context.last_user_message.as_deref(),
            CONTEXT_LIMIT,
        );
        insert_truncated(
            &mut context,
            "assistant_preamble",
            self.context.assistant_preamble.as_deref(),
            CONTEXT_LIMIT,
        );
        if !context.is_empty() {
            row.insert("context".to_string(), Value::Object(context));
        }

        row.insert(
            "full_text_preview".to_string(),
            json!(truncate_chars(&self.full_text, PREVIEW_LIMIT)),
        );
        row.insert(
            "full_text_truncated".to_string(),
            json!(char_len(&self.full_text) > PREVIEW_LIMIT),
        );

        Value::Object(row)
    }
}

pub struct FeedStore {
    items: VecDeque<Item>,
    revision: u64,
    cap: usize,
    contexts: HashMap<String, Context>,
    /// Hooks blocked in `feed.push`, by request id; dropping a sender wakes its hook empty-handed.
    waiters: HashMap<String, oneshot::Sender<Value>>,
}

impl FeedStore {
    pub fn new(cap: usize) -> Self {
        Self {
            items: VecDeque::new(),
            revision: 0,
            cap,
            contexts: HashMap::new(),
            waiters: HashMap::new(),
        }
    }

    /// Register the hook that waits for a pending item's decision (macOS `ingestBlocking`).
    pub fn wait_for(&mut self, item_id: &str) -> Option<(String, oneshot::Receiver<Value>)> {
        let item = self.items.iter().find(|item| item.id == item_id)?;
        if item.status != "pending" {
            return None;
        }
        let request_id = item.request_id.clone()?;
        let (sender, receiver) = oneshot::channel();
        self.waiters.insert(request_id.clone(), sender);
        Some((request_id, receiver))
    }

    /// Resolve a pending request with a validated decision and wake its hook.
    pub fn reply(
        &mut self,
        request_id: &str,
        decision: Value,
        now_ms: u64,
    ) -> Result<(), &'static str> {
        let expected_kind = match decision.get("kind").and_then(Value::as_str) {
            Some("permission") => "permissionRequest",
            Some("exit_plan") => "exitPlan",
            Some("question") => "question",
            _ => return Err("invalid_params"),
        };
        let item = self
            .items
            .iter_mut()
            .find(|item| item.request_id.as_deref() == Some(request_id))
            .ok_or("not_found")?;
        if item.kind != expected_kind {
            return Err("invalid_params");
        }
        if item.status != "pending" {
            return Err("not_pending");
        }
        item.status = "resolved".to_string();
        item.decision = Some(decision.clone());
        item.updated_at_ms = now_ms;
        item.resolved_at_ms = Some(now_ms);
        self.revision += 1;
        if let Some(waiter) = self.waiters.remove(request_id) {
            let _ = waiter.send(decision);
        }
        Ok(())
    }

    /// The hook gave up (timeout): the agent falls back to its own prompt.
    pub fn expire(&mut self, request_id: &str, now_ms: u64) {
        self.waiters.remove(request_id);
        let pending = self.items.iter_mut().find(|item| {
            item.request_id.as_deref() == Some(request_id) && item.status == "pending"
        });
        if let Some(item) = pending {
            item.status = "expired".to_string();
            item.updated_at_ms = now_ms;
            item.resolved_at_ms = Some(now_ms);
            self.revision += 1;
        }
    }

    /// A later prompt, stop or session end proves the agent moved past its pending decisions
    /// (answered in the terminal: Claude keeps the abandoned hook waiting), as macOS
    /// `supersedesPendingDecisions`.
    fn expire_superseded(&mut self, workstream_id: &str, now_ms: u64) {
        let superseded: Vec<String> = self
            .items
            .iter()
            .filter(|item| item.workstream_id == workstream_id && item.status == "pending")
            .filter_map(|item| item.request_id.clone())
            .collect();
        for request_id in superseded {
            self.expire(&request_id, now_ms);
        }
    }

    /// Ingest a hook event. Returns the new item id, or None when ignored.
    pub fn ingest(&mut self, event: &Value, now_ms: u64) -> Option<String> {
        let hook = non_empty_str(event, "hook_event_name")?;
        let session_id = non_empty_str(event, "session_id")?;
        let source =
            non_empty_str(event, "_source").unwrap_or_else(|| "claude".to_string());
        let payload = decode_hook(event, &hook)?;
        let workstream_id = format!("{}-{}", source, session_id);
        if matches!(hook.as_str(), "UserPromptSubmit" | "Stop" | "SessionEnd") {
            self.expire_superseded(&workstream_id, now_ms);
        }

        let HookPayload {
            kind,
            text,
            reason,
            tool_name,
            tool_input,
            tool_result,
            tool_result_is_error,
        } = payload;

        let mut context = self
            .contexts
            .get(&workstream_id)
            .cloned()
            .unwrap_or_default();

        let event_last_user = if kind == "userPrompt" {
            text.clone()
                .or_else(|| context_last_user_message(event))
        } else {
            context_last_user_message(event)
        };
        if let Some(last_user) = event_last_user {
            context.last_user_message = Some(last_user);
        }
        if kind == "assistantMessage" {
            if let Some(preamble) = text.clone().filter(|value| !value.is_empty()) {
                context.assistant_preamble = Some(preamble);
            }
        }
        self.contexts
            .insert(workstream_id.clone(), context.clone());

        let full_text = match kind {
            "userPrompt" | "assistantMessage" => text.clone().unwrap_or_default(),
            "stop" => reason.clone().unwrap_or_default(),
            "toolUse" => tool_input.clone().unwrap_or_default(),
            "toolResult" => tool_result.clone().unwrap_or_default(),
            "permissionRequest" | "exitPlan" | "question" => tool_input.clone().unwrap_or_default(),
            _ => String::new(),
        };

        let blocking = matches!(kind, "permissionRequest" | "exitPlan" | "question");
        let request_id = if blocking {
            Some(
                non_empty_str(event, "_opencode_request_id")
                    .or_else(|| non_empty_str(event, "request_id"))
                    .unwrap_or_else(|| Uuid::new_v4().to_string()),
            )
        } else {
            None
        };
        let tool_input_value = if blocking {
            match event.get("tool_input") {
                Some(Value::String(text)) => serde_json::from_str(text).ok(),
                Some(value @ Value::Object(_)) => Some(value.clone()),
                _ => None,
            }
        } else {
            None
        };

        let id = Uuid::new_v4().to_string();
        self.revision += 1;

        self.items.push_back(Item {
            id: id.clone(),
            workstream_id,
            source,
            kind: kind.to_string(),
            status: if blocking { "pending" } else { "telemetry" }.to_string(),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            cwd: non_empty_str(event, "cwd"),
            title: tool_name.clone(),
            workspace_id: non_empty_str(event, "workspace_id"),
            surface_id: non_empty_str(event, "surface_id"),
            text,
            reason,
            tool_name,
            tool_input,
            tool_result,
            tool_result_is_error,
            context,
            full_text,
            request_id,
            tool_input_value,
            decision: None,
            resolved_at_ms: None,
        });

        while self.items.len() > self.cap {
            if let Some(dropped) = self.items.pop_front() {
                if let Some(request_id) = dropped.request_id {
                    self.waiters.remove(&request_id);
                }
            }
        }

        Some(id)
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Visible feed rows, newest first.
    pub fn list(&self, limit: usize) -> Value {
        let items: Vec<Value> = self
            .items
            .iter()
            .rev()
            .filter(|item| item.is_visible())
            .take(limit)
            .map(|item| item.to_json())
            .collect();

        json!({
            "revision": self.revision,
            "items": items,
        })
    }

    /// Paginated full text for one item.
    pub fn text(
        &self,
        id: &str,
        offset: usize,
        version: Option<u64>,
    ) -> Result<Value, &'static str> {
        let item = self
            .items
            .iter()
            .find(|item| item.id == id)
            .ok_or("not_found")?;

        let chars: Vec<char> = item.full_text.chars().collect();
        let total = chars.len();
        if offset > total {
            return Err("invalid_params");
        }
        if offset > 0 && version != Some(item.updated_at_ms) {
            return Err("stale_item");
        }

        let end = offset.saturating_add(TEXT_PAGE_CHARS).min(total);
        let text: String = chars[offset..end].iter().collect();

        let mut response = json!({
            "text": text,
            "version": item.updated_at_ms,
        });
        if end < total {
            response["next_offset"] = json!(end);
        }

        Ok(response)
    }
}

pub fn global() -> &'static Mutex<FeedStore> {
    static GLOBAL: OnceLock<Mutex<FeedStore>> = OnceLock::new();
    GLOBAL.get_or_init(|| Mutex::new(FeedStore::new(500)))
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

pub const REPLY_METHODS: [&str; 3] =
    ["feed.permission.reply", "feed.question.reply", "feed.exit_plan.reply"];

/// Validate one reply verb (macOS `v2FeedPermissionReply`…) into `(request_id, decision)`.
pub fn decision_from_reply(method: &str, params: &Value) -> Result<(String, Value), String> {
    let request_id = params
        .get("request_id")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{method} requires request_id"))?
        .to_string();
    let mode = params.get("mode").and_then(Value::as_str);
    let decision = match method {
        "feed.permission.reply" => match mode {
            Some(mode @ ("once" | "always" | "all" | "bypass" | "deny")) => {
                json!({"kind": "permission", "mode": mode})
            }
            _ => return Err(format!("{method} requires mode ∈ once|always|all|bypass|deny")),
        },
        "feed.question.reply" => {
            let selections = params
                .get("selections")
                .and_then(Value::as_array)
                .filter(|list| list.iter().all(Value::is_string))
                .ok_or_else(|| format!("{method} requires selections: [string]"))?;
            json!({"kind": "question", "selections": selections})
        }
        "feed.exit_plan.reply" => match mode {
            Some(mode @ ("ultraplan" | "bypassPermissions" | "autoAccept" | "manual" | "deny")) => {
                let mut decision = json!({"kind": "exit_plan", "mode": mode});
                if let Some(feedback) = params
                    .get("feedback")
                    .and_then(Value::as_str)
                    .filter(|text| !text.trim().is_empty())
                {
                    decision["feedback"] = json!(feedback);
                }
                decision
            }
            _ => {
                return Err(format!(
                    "{method} requires mode ∈ ultraplan|bypassPermissions|autoAccept|manual|deny"
                ))
            }
        },
        _ => return Err(format!("unknown reply method {method}")),
    };
    Ok((request_id, decision))
}

/// One reply verb against the global store: `Ok({"delivered": true})` or `(code, message)`.
pub fn reply_rpc(method: &str, params: &Value, now_ms: u64) -> Result<Value, (&'static str, String)> {
    let (request_id, decision) =
        decision_from_reply(method, params).map_err(|message| ("invalid_params", message))?;
    let mut store = global()
        .lock()
        .map_err(|_| ("internal_error", "feed store unavailable".to_string()))?;
    store
        .reply(&request_id, decision, now_ms)
        .map_err(|code| (code, format!("{method}: {code}")))?;
    Ok(json!({"delivered": true}))
}

/// AskUserQuestion prompts in the macOS `questionDict` shape; option ids are their labels,
/// which `feed.question.reply` sends back as selections.
fn question_prompts(input: Option<&Value>) -> Vec<Value> {
    let Some(questions) = input
        .and_then(|input| input.get("questions"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    questions
        .iter()
        .enumerate()
        .map(|(index, question)| {
            let options: Vec<Value> = question
                .get("options")
                .and_then(Value::as_array)
                .map(|options| {
                    options
                        .iter()
                        .filter_map(|option| {
                            let label = option.get("label").and_then(Value::as_str)?;
                            let mut row = json!({"id": label, "label": label});
                            if let Some(description) =
                                option.get("description").and_then(Value::as_str)
                            {
                                row["description"] = json!(description);
                            }
                            Some(row)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let mut prompt = json!({
                "id": index.to_string(),
                "prompt": question.get("question").and_then(Value::as_str).unwrap_or_default(),
                "multi_select": question.get("multiSelect").and_then(Value::as_bool).unwrap_or(false),
                "options": options,
            });
            if let Some(header) = question.get("header").and_then(Value::as_str) {
                prompt["header"] = json!(header);
            }
            prompt
        })
        .collect()
}

struct HookPayload {
    kind: &'static str,
    text: Option<String>,
    reason: Option<String>,
    tool_name: Option<String>,
    tool_input: Option<String>,
    tool_result: Option<String>,
    tool_result_is_error: Option<bool>,
}

fn decode_hook(event: &Value, hook: &str) -> Option<HookPayload> {
    let tool_name = non_empty_str(event, "tool_name");
    let message = non_empty_str(event, "message");

    match hook {
        "UserPromptSubmit" => Some(HookPayload {
            kind: "userPrompt",
            text: user_prompt_text(event),
            reason: None,
            tool_name,
            tool_input: None,
            tool_result: None,
            tool_result_is_error: None,
        }),
        "Stop" => Some(HookPayload {
            kind: "stop",
            text: None,
            reason: stop_reason(event),
            tool_name,
            tool_input: None,
            tool_result: None,
            tool_result_is_error: None,
        }),
        "SessionStart" => Some(HookPayload {
            kind: "sessionStart",
            text: None,
            reason: None,
            tool_name,
            tool_input: None,
            tool_result: None,
            tool_result_is_error: None,
        }),
        "SessionEnd" => Some(HookPayload {
            kind: "sessionEnd",
            text: None,
            reason: None,
            tool_name,
            tool_input: None,
            tool_result: None,
            tool_result_is_error: None,
        }),
        "PreToolUse" => Some(HookPayload {
            kind: "toolUse",
            text: None,
            reason: None,
            tool_name,
            tool_input: tool_input_json_string(event),
            tool_result: None,
            tool_result_is_error: None,
        }),
        "PostToolUse" => Some(HookPayload {
            kind: "toolResult",
            text: None,
            reason: None,
            tool_name,
            tool_input: None,
            tool_result: tool_result_value(event, &message),
            tool_result_is_error: Some(false),
        }),
        "PostToolUseFailure" => Some(HookPayload {
            kind: "toolResult",
            text: None,
            reason: None,
            tool_name,
            tool_input: None,
            tool_result: tool_result_value(event, &message),
            tool_result_is_error: Some(true),
        }),
        "Notification" => Some(HookPayload {
            kind: "toolResult",
            text: None,
            reason: None,
            tool_name: Some("notification".to_string()),
            tool_input: None,
            tool_result: tool_result_value(event, &message),
            tool_result_is_error: Some(false),
        }),
        "PermissionRequest" => Some(HookPayload {
            kind: match tool_name.as_deref() {
                Some("AskUserQuestion") => "question",
                Some("ExitPlanMode") => "exitPlan",
                _ => "permissionRequest",
            },
            text: None,
            reason: None,
            tool_name,
            tool_input: tool_input_json_string(event),
            tool_result: None,
            tool_result_is_error: None,
        }),
        "AssistantMessage" => Some(HookPayload {
            kind: "assistantMessage",
            text: assistant_text(event, &message),
            reason: None,
            tool_name,
            tool_input: None,
            tool_result: None,
            tool_result_is_error: None,
        }),
        _ => None,
    }
}

fn non_empty_str(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn nested_non_empty_str(value: &Value, outer: &str, inner: &str) -> Option<String> {
    value
        .get(outer)
        .and_then(|nested| nested.get(inner))
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn tool_input_field(event: &Value, key: &str) -> Option<String> {
    event
        .get("tool_input")
        .and_then(|input| input.get(key))
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn tool_input_json_string(event: &Value) -> Option<String> {
    match event.get("tool_input") {
        Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
        Some(value @ Value::Object(_)) => Some(value.to_string()),
        _ => None,
    }
}

fn user_prompt_text(event: &Value) -> Option<String> {
    tool_input_field(event, "prompt")
        .or_else(|| tool_input_field(event, "text"))
        .or_else(|| tool_input_field(event, "message"))
        .or_else(|| match event.get("tool_input") {
            Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
            _ => None,
        })
        .or_else(|| context_last_user_message(event))
        .or_else(|| non_empty_str(event, "message"))
}

fn stop_reason(event: &Value) -> Option<String> {
    tool_input_field(event, "reason")
        .or_else(|| tool_input_field(event, "last_assistant_message"))
        .or_else(|| non_empty_str(event, "message"))
}

fn assistant_text(event: &Value, message: &Option<String>) -> Option<String> {
    tool_input_field(event, "text").or_else(|| message.clone())
}

fn tool_result_value(event: &Value, message: &Option<String>) -> Option<String> {
    tool_input_json_string(event).or_else(|| message.clone())
}

fn context_last_user_message(event: &Value) -> Option<String> {
    nested_non_empty_str(event, "context", "lastUserMessage")
}

fn insert_optional(map: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(text) = value {
        map.insert(key.to_string(), json!(text));
    }
}

fn insert_truncated(
    map: &mut Map<String, Value>,
    key: &str,
    value: Option<&str>,
    limit: usize,
) {
    if let Some(text) = value {
        map.insert(key.to_string(), json!(truncate_chars(text, limit)));
    }
}

fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

fn char_len(text: &str) -> usize {
    text.chars().count()
}

fn iso8601_utc(now_ms: u64) -> String {
    let total_seconds = now_ms / 1_000;
    let days = (total_seconds / 86_400) as i64;
    let seconds_of_day = total_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year,
        month,
        day,
        seconds_of_day / 3_600,
        (seconds_of_day % 3_600) / 60,
        seconds_of_day % 60
    )
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year =
        day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };

    (year, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_row_carries_prompt_context() {
        let mut store = FeedStore::new(10);
        let prompt = json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "s1",
            "tool_input": { "prompt": "hello" }
        });
        assert!(store.ingest(&prompt, 1_000).is_some());

        let stop = json!({
            "hook_event_name": "Stop",
            "session_id": "s1",
            "tool_input": { "reason": "done" }
        });
        assert!(store.ingest(&stop, 2_000).is_some());

        let list = store.list(10);
        let items = list["items"].as_array().expect("items array");
        assert_eq!(items.len(), 1);
        let row = &items[0];
        assert_eq!(row["kind"], json!("stop"));
        assert_eq!(row["reason"], json!("done"));
        assert_eq!(row["context"]["last_user_message"], json!("hello"));
        assert_eq!(row["workstream_id"], json!("claude-s1"));
    }

    #[test]
    fn list_hides_prompts_and_session_markers() {
        let mut store = FeedStore::new(10);
        let prompt = json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "s1",
            "message": "hidden"
        });
        let start = json!({
            "hook_event_name": "SessionStart",
            "session_id": "s1"
        });
        assert!(store.ingest(&prompt, 1_000).is_some());
        assert!(store.ingest(&start, 2_000).is_some());

        let list = store.list(10);
        assert!(list["items"].as_array().expect("items array").is_empty());
    }

    #[test]
    fn revision_counts_created_items_only() {
        let mut store = FeedStore::new(10);
        assert_eq!(store.revision(), 0);

        let prompt = json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "s1",
            "message": "hello"
        });
        assert!(store.ingest(&prompt, 1_000).is_some());
        assert_eq!(store.revision(), 1);

        let ignored = json!({
            "hook_event_name": "UnknownEvent",
            "session_id": "s1"
        });
        assert!(store.ingest(&ignored, 2_000).is_none());
        assert_eq!(store.revision(), 1);

        let start = json!({
            "hook_event_name": "SessionStart",
            "session_id": "s1"
        });
        assert!(store.ingest(&start, 3_000).is_some());
        assert_eq!(store.revision(), 2);
    }

    #[test]
    fn cap_keeps_only_newest_items() {
        let mut store = FeedStore::new(3);
        for index in 0..5 {
            let event = json!({
                "hook_event_name": "AssistantMessage",
                "session_id": format!("s{}", index),
                "message": format!("m{}", index)
            });
            assert!(store.ingest(&event, index).is_some());
        }

        assert_eq!(store.items.len(), 3);
        assert_eq!(store.revision(), 5);

        let list = store.list(10);
        let items = list["items"].as_array().expect("items array");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["text"], json!("m4"));
        assert_eq!(items[1]["text"], json!("m3"));
        assert_eq!(items[2]["text"], json!("m2"));
    }

    #[test]
    fn long_text_is_truncated_by_characters() {
        let long_text = "é".repeat(9_000);
        let mut store = FeedStore::new(10);
        let event = json!({
            "hook_event_name": "AssistantMessage",
            "session_id": "s1",
            "message": long_text
        });
        assert!(store.ingest(&event, 1_000).is_some());

        let list = store.list(1);
        let row = &list["items"][0];
        let preview = row["full_text_preview"].as_str().expect("preview");
        assert_eq!(preview.chars().count(), 8_000);
        assert_eq!(preview.len(), 16_000);
        assert!(preview.chars().all(|character| character == 'é'));
        assert_eq!(row["full_text_truncated"], json!(true));
        assert_eq!(
            row["text"].as_str().expect("text").chars().count(),
            8_000
        );
    }

    #[test]
    fn text_is_paginated_and_checks_version() {
        let long_text = "a".repeat(20_000);
        let mut store = FeedStore::new(10);
        let event = json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "s1",
            "tool_input": { "prompt": long_text }
        });
        let id = store.ingest(&event, 1_000).expect("item");

        let page_one = store.text(&id, 0, None).expect("page one");
        assert_eq!(
            page_one["text"]
                .as_str()
                .expect("page one text")
                .chars()
                .count(),
            16_000
        );
        assert_eq!(page_one["next_offset"], json!(16_000));
        let version = page_one["version"].as_u64().expect("version");

        let page_two = store
            .text(&id, 16_000, Some(version))
            .expect("page two");
        assert_eq!(
            page_two["text"]
                .as_str()
                .expect("page two text")
                .chars()
                .count(),
            4_000
        );
        assert!(page_two.get("next_offset").is_none());

        assert_eq!(
            store.text(&id, 16_000, Some(version + 1)),
            Err("stale_item")
        );
        assert_eq!(
            store.text(&id, 20_001, Some(version)),
            Err("invalid_params")
        );
    }

    #[test]
    fn iso_dates_use_utc() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(
            iso8601_utc(1_793_102_464_000),
            "2026-10-27T12:01:04Z"
        );
    }

    fn permission_event(session: &str, tool: &str, input: Value) -> Value {
        json!({
            "hook_event_name": "PermissionRequest",
            "session_id": session,
            "_source": "claude",
            "_opencode_request_id": format!("req-{session}-{tool}"),
            "tool_name": tool,
            "tool_input": input,
        })
    }

    /// A permission request is a pending row; a phone reply resolves it and wakes the hook.
    #[test]
    fn reply_resolves_pending_request_and_wakes_hook() {
        let mut store = FeedStore::new(10);
        let event = permission_event("s1", "Write", json!({"file_path": "/tmp/x"}));
        let id = store.ingest(&event, 1_000).expect("item");
        let row = &store.list(10)["items"][0];
        assert_eq!(row["kind"], "permissionRequest");
        assert_eq!(row["status"], "pending");
        assert_eq!(row["request_id"], "req-s1-Write");

        let (request_id, mut waiter) = store.wait_for(&id).expect("pending waiter");
        let revision = store.revision();
        let decision = json!({"kind": "permission", "mode": "once"});
        store.reply(&request_id, decision.clone(), 2_000).unwrap();
        assert_eq!(waiter.try_recv().unwrap(), decision);
        assert!(store.revision() > revision, "phones must see the status change");
        let row = &store.list(10)["items"][0];
        assert_eq!(row["status"], "resolved");
        assert_eq!(row["decision"]["mode"], "once");
        assert_eq!(store.reply(&request_id, decision, 3_000), Err("not_pending"));
        assert!(store.wait_for(&id).is_none());
    }

    /// A decision of another kind cannot answer the request.
    #[test]
    fn reply_kind_must_match_request() {
        let mut store = FeedStore::new(10);
        store.ingest(&permission_event("s1", "Write", json!({})), 1_000);
        let wrong = json!({"kind": "question", "selections": ["Thé"]});
        assert_eq!(store.reply("req-s1-Write", wrong, 2_000), Err("invalid_params"));
        let unknown = json!({"kind": "permission", "mode": "once"});
        assert_eq!(store.reply("missing", unknown, 2_000), Err("not_found"));
    }

    /// Answered in the terminal: the next Stop of the same agent expires the request and
    /// releases its hook; another agent's Stop leaves it pending.
    #[test]
    fn later_stop_expires_only_its_own_pending_requests() {
        let mut store = FeedStore::new(10);
        let mine = store.ingest(&permission_event("s1", "Write", json!({})), 1_000).unwrap();
        let (_, mut waiter) = store.wait_for(&mine).unwrap();
        let other = json!({"hook_event_name": "Stop", "session_id": "s2", "_source": "claude"});
        store.ingest(&other, 2_000);
        assert!(matches!(
            waiter.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        let stop = json!({"hook_event_name": "Stop", "session_id": "s1", "_source": "claude"});
        store.ingest(&stop, 3_000);
        assert!(matches!(
            waiter.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Closed)
        ));
        let rows = store.list(10);
        let row = rows["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == mine)
            .unwrap();
        assert_eq!(row["status"], "expired");
    }

    /// AskUserQuestion and ExitPlanMode become question / plan rows in the macOS shape.
    #[test]
    fn question_and_plan_rows_carry_their_choices() {
        let mut store = FeedStore::new(10);
        let question = json!({"questions": [{"question": "Thé ou café ?", "header": "Boisson",
            "multiSelect": false, "options": [{"label": "Thé", "description": "Infusion"},
            {"label": "Café"}]}]});
        store.ingest(&permission_event("s1", "AskUserQuestion", question), 1_000);
        let plan = json!({"plan": "\n# Plan\n- étape"});
        store.ingest(&permission_event("s1", "ExitPlanMode", plan), 2_000);
        let rows = store.list(10);
        let plan_row = &rows["items"][0];
        assert_eq!(plan_row["kind"], "exitPlan");
        assert_eq!(plan_row["plan_summary"], "# Plan");
        let question_row = &rows["items"][1];
        assert_eq!(question_row["kind"], "question");
        assert_eq!(question_row["question_prompt"], "Thé ou café ?");
        assert_eq!(question_row["question_options"][0]["id"], "Thé");
        assert_eq!(question_row["question_options"][0]["description"], "Infusion");
        assert_eq!(question_row["questions"][0]["header"], "Boisson");
    }

    /// Reply verbs accept only the macOS modes and shapes.
    #[test]
    fn reply_verbs_validate_like_macos() {
        let ok = decision_from_reply(
            "feed.exit_plan.reply",
            &json!({"request_id": "r", "mode": "manual", "feedback": "  plus court "}),
        )
        .unwrap();
        assert_eq!(ok.1, json!({"kind": "exit_plan", "mode": "manual", "feedback": "  plus court "}));
        for (method, params) in [
            ("feed.permission.reply", json!({"request_id": "r", "mode": "maybe"})),
            ("feed.permission.reply", json!({"mode": "once"})),
            ("feed.question.reply", json!({"request_id": "r", "selections": [1]})),
            ("feed.exit_plan.reply", json!({"request_id": "r", "mode": "auto"})),
        ] {
            assert!(decision_from_reply(method, &params).is_err(), "{method} {params}");
        }
    }
}
