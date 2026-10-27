use serde_json::{json, Map, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
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
}

impl Item {
    fn is_visible(&self) -> bool {
        match self.kind.as_str() {
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
            _ => {}
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
}

impl FeedStore {
    pub fn new(cap: usize) -> Self {
        Self {
            items: VecDeque::new(),
            revision: 0,
            cap,
            contexts: HashMap::new(),
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
            _ => String::new(),
        };

        let id = Uuid::new_v4().to_string();
        self.revision += 1;

        self.items.push_back(Item {
            id: id.clone(),
            workstream_id,
            source,
            kind: kind.to_string(),
            status: "telemetry".to_string(),
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
        });

        while self.items.len() > self.cap {
            self.items.pop_front();
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
}
