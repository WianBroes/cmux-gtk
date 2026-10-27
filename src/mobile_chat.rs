//! Agent conversations for the phone (upstream `mobile.chat.*`, iOS PR #22086): the Claude Code
//! session a terminal's hooks bound to it (`surface.resume.set`) is read from its JSONL
//! transcript, in the wire shapes of upstream `CmuxAgentChat` (`ChatMessage`,
//! `ChatSessionDescriptor`, `ChatHistoryPage`, `ChatSessionEvent`).
//! Parsing follows upstream `ClaudeTranscriptParser`: one seq per transcript line, prose,
//! thoughts, tools folded with their result, Bash as a terminal capture, edits as file edits.

use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Agents whose transcript the host can read, by resume `kind`.
pub const AGENTS: &[&str] = &["claude"];

const USER_NOISE_PREFIXES: &[&str] = &["<command-name>", "<local-command", "<system-reminder"];
const SUMMARY_ARGUMENT_KEYS: &[&str] = &[
    "file_path",
    "notebook_path",
    "path",
    "pattern",
    "command",
    "query",
    "url",
    "description",
    "prompt",
    "skill",
    "name",
];
// Upstream `TranscriptTextBudget` keeps phone payloads bounded the same way.
const MAX_BODY_CHARACTERS: usize = 64 * 1024;
const MAX_INPUT_DETAIL_CHARACTERS: usize = 4 * 1024;
const MAX_SUMMARY_ARGUMENT_CHARACTERS: usize = 120;
/// A transcript line longer than this (pasted images, huge tool output) is skipped unread.
const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// Messages appended and earlier messages changed (a tool call got its result) by one read.
#[derive(Debug, Default, PartialEq)]
pub struct Change {
    pub appended: Vec<Value>,
    pub updated: Vec<Value>,
}

/// One Claude transcript, read incrementally from the byte offset where the last read stopped.
#[derive(Default)]
pub struct ClaudeTranscript {
    messages: Vec<Value>,
    pending: HashMap<String, Vec<usize>>,
    next_seq: i64,
    offset: u64,
    partial: Vec<u8>,
    last_timestamp: Option<String>,
}

impl ClaudeTranscript {
    /// Read what was written since the previous call; a file that shrank is read again whole.
    pub fn read_from(&mut self, path: &Path) -> std::io::Result<Change> {
        let mut file = std::fs::File::open(path)?;
        let length = file.metadata()?.len();
        if length < self.offset {
            *self = Self::default();
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        file.take(length - self.offset).read_to_end(&mut bytes)?;
        self.offset += bytes.len() as u64;
        Ok(self.feed(&bytes))
    }

    /// Parse complete lines of `bytes`, keeping a trailing partial line for the next call.
    pub fn feed(&mut self, bytes: &[u8]) -> Change {
        self.partial.extend_from_slice(bytes);
        let mut appended = Vec::new();
        let mut resolved = Vec::new();
        let mut start = 0;
        while let Some(end) = self.partial[start..].iter().position(|byte| *byte == b'\n') {
            let line = &self.partial[start..start + end];
            let seq = self.next_seq;
            self.next_seq += 1;
            if line.len() <= MAX_LINE_BYTES {
                if let Ok(root) = serde_json::from_slice::<Value>(line) {
                    let first = self.messages.len();
                    self.line(&root, seq, &mut resolved);
                    appended.extend(first..self.messages.len());
                }
            }
            start += end + 1;
        }
        self.partial.drain(..start);
        if self.partial.len() > MAX_LINE_BYTES {
            self.partial.clear();
        }
        let fresh: HashSet<usize> = appended.iter().copied().collect();
        let mut seen = HashSet::new();
        Change {
            appended: appended.iter().map(|&i| self.messages[i].clone()).collect(),
            updated: resolved
                .into_iter()
                .filter(|i| !fresh.contains(i) && seen.insert(*i))
                .map(|i| self.messages[i].clone())
                .collect(),
        }
    }

    /// `ChatHistoryPage`: the newest `limit` messages older than `before_seq`, oldest first.
    pub fn page(&self, before_seq: Option<i64>, limit: usize) -> Value {
        let older: Vec<&Value> = self
            .messages
            .iter()
            .filter(|message| before_seq.is_none_or(|before| seq_of(message) < before))
            .collect();
        let start = older.len().saturating_sub(limit);
        json!({"messages": older[start..], "has_more": start > 0})
    }

    fn line(&mut self, root: &Value, seq: i64, resolved: &mut Vec<usize>) {
        // Task-subagent traffic shares the file; its user lines are prompts nobody typed.
        if root["isSidechain"].as_bool() == Some(true) {
            return;
        }
        if let Some(stamp) = root["timestamp"].as_str() {
            self.last_timestamp = Some(wire_timestamp(stamp));
        }
        let timestamp = self
            .last_timestamp
            .clone()
            .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_owned());
        let line_id = root["uuid"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("line-{seq}"));
        let content = &root["message"]["content"];
        let mut emitted = 0;
        match root["type"].as_str() {
            Some("user") if root["isMeta"].as_bool() != Some(true) => {
                if let Some(text) = content.as_str() {
                    self.user_prose(text, &line_id, &mut emitted, seq, &timestamp);
                }
                for block in content.as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => self.user_prose(
                            block["text"].as_str().unwrap_or_default(),
                            &line_id,
                            &mut emitted,
                            seq,
                            &timestamp,
                        ),
                        Some("tool_result") => self.tool_result(block, resolved),
                        _ => {}
                    }
                }
            }
            Some("assistant") => {
                if let Some(text) = content.as_str() {
                    self.agent_text("prose", text, &line_id, &mut emitted, seq, &timestamp);
                }
                for block in content.as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => self.agent_text(
                            "prose",
                            block["text"].as_str().unwrap_or_default(),
                            &line_id,
                            &mut emitted,
                            seq,
                            &timestamp,
                        ),
                        Some("thinking") => self.agent_text(
                            "thought",
                            block["thinking"].as_str().unwrap_or_default(),
                            &line_id,
                            &mut emitted,
                            seq,
                            &timestamp,
                        ),
                        Some("tool_use") => {
                            self.tool_use(block, &line_id, &mut emitted, seq, &timestamp)
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn push(
        &mut self,
        line_id: &str,
        emitted: &mut usize,
        seq: i64,
        role: &str,
        timestamp: &str,
        kind: Value,
    ) -> usize {
        let id = if *emitted == 0 {
            line_id.to_owned()
        } else {
            format!("{line_id}#{emitted}")
        };
        *emitted += 1;
        self.messages.push(json!({
            "id": id, "seq": seq, "role": role, "timestamp": timestamp, "kind": kind,
        }));
        self.messages.len() - 1
    }

    fn user_prose(
        &mut self,
        text: &str,
        line_id: &str,
        emitted: &mut usize,
        seq: i64,
        timestamp: &str,
    ) {
        let trimmed = text.trim();
        if trimmed.is_empty()
            || USER_NOISE_PREFIXES
                .iter()
                .any(|prefix| trimmed.starts_with(prefix))
        {
            return;
        }
        let kind = json!({"type": "prose", "text": truncate(text, MAX_BODY_CHARACTERS)});
        self.push(line_id, emitted, seq, "user", timestamp, kind);
    }

    fn agent_text(
        &mut self,
        kind: &str,
        text: &str,
        line_id: &str,
        emitted: &mut usize,
        seq: i64,
        timestamp: &str,
    ) {
        if text.trim().is_empty() {
            return;
        }
        let kind = json!({"type": kind, "text": truncate(text, MAX_BODY_CHARACTERS)});
        self.push(line_id, emitted, seq, "agent", timestamp, kind);
    }

    fn tool_use(
        &mut self,
        block: &Value,
        line_id: &str,
        emitted: &mut usize,
        seq: i64,
        timestamp: &str,
    ) {
        let Some(name) = block["name"].as_str() else {
            return;
        };
        let input = &block["input"];
        let mut positions = Vec::new();
        for kind in tool_kinds(name, input) {
            positions.push(self.push(line_id, emitted, seq, "agent", timestamp, kind));
        }
        if let Some(call) = block["id"].as_str() {
            self.pending.insert(call.to_owned(), positions);
        }
    }

    fn tool_result(&mut self, block: &Value, resolved: &mut Vec<usize>) {
        let Some(positions) = block["tool_use_id"]
            .as_str()
            .and_then(|call| self.pending.remove(call))
        else {
            return;
        };
        let output = result_text(&block["content"]);
        let failed = block["is_error"].as_bool() == Some(true);
        for position in positions {
            let kind = &mut self.messages[position]["kind"];
            match kind["type"].as_str() {
                Some("terminal") => {
                    kind["is_running"] = json!(false);
                    if let Some(output) = &output {
                        kind["output"] = json!(truncate(output, MAX_BODY_CHARACTERS));
                        if let Some(code) = exit_code(output) {
                            kind["exit_code"] = json!(code);
                        }
                    }
                    if failed && kind.get("exit_code").is_none() {
                        kind["exit_code"] = json!(1);
                    }
                }
                Some("tool_use") => {
                    kind["status"] = json!(if failed { "failed" } else { "succeeded" });
                    if let Some(output) = &output {
                        kind["output"] = json!(truncate(output, MAX_BODY_CHARACTERS));
                    }
                }
                _ => continue,
            }
            resolved.push(position);
        }
    }
}

fn tool_kinds(name: &str, input: &Value) -> Vec<Value> {
    if name == "Bash" {
        if let Some(command) = input["command"].as_str() {
            return vec![json!({"type": "terminal", "command": command, "is_running": true})];
        }
    }
    if matches!(name, "Write" | "Edit" | "MultiEdit" | "NotebookEdit") {
        if let Some(edit) = file_edit(name, input) {
            return vec![edit];
        }
    }
    if name == "AskUserQuestion" {
        let questions: Vec<Value> = input["questions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|question| {
                let prompt = question["question"].as_str()?;
                let options: Vec<Value> = question["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|option| {
                        let label = option["label"].as_str()?;
                        Some(json!({"label": label, "detail": option["description"]}))
                    })
                    .collect();
                Some(json!({"type": "question", "prompt": prompt, "options": options}))
            })
            .collect();
        if !questions.is_empty() {
            return questions;
        }
    }
    let summary = SUMMARY_ARGUMENT_KEYS
        .iter()
        .find_map(|key| {
            input[*key]
                .as_str()
                .filter(|value| !value.trim().is_empty())
        })
        .map(|value| {
            let line = value.split_whitespace().collect::<Vec<_>>().join(" ");
            format!(
                "{name} {}",
                truncate(&line, MAX_SUMMARY_ARGUMENT_CHARACTERS)
            )
        })
        .unwrap_or_else(|| name.to_owned());
    let mut kind =
        json!({"type": "tool_use", "tool_name": name, "summary": summary, "status": "running"});
    if !input.is_null() {
        kind["input_detail"] = json!(truncate(&input.to_string(), MAX_INPUT_DETAIL_CHARACTERS));
    }
    vec![kind]
}

/// Upstream `TranscriptDiffBuilder` in its simplest form: old lines removed, new lines added.
fn file_edit(name: &str, input: &Value) -> Option<Value> {
    let path = input["file_path"]
        .as_str()
        .or_else(|| input["notebook_path"].as_str())?;
    let pairs: Vec<(&str, &str)> = match name {
        "Write" => vec![("", input["content"].as_str().unwrap_or_default())],
        "MultiEdit" => input["edits"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|edit| {
                (
                    edit["old_string"].as_str().unwrap_or_default(),
                    edit["new_string"].as_str().unwrap_or_default(),
                )
            })
            .collect(),
        _ => vec![(
            input["old_string"].as_str().unwrap_or_default(),
            input["new_string"]
                .as_str()
                .or_else(|| input["new_source"].as_str())
                .unwrap_or_default(),
        )],
    };
    let (mut additions, mut deletions, mut diff) = (0, 0, String::new());
    for (old, new) in pairs {
        for line in old.lines() {
            deletions += 1;
            diff.push_str(&format!("-{line}\n"));
        }
        for line in new.lines() {
            additions += 1;
            diff.push_str(&format!("+{line}\n"));
        }
    }
    let mut kind = json!({
        "type": "file_edit",
        "file_path": path,
        "operation": if name == "Write" { "write" } else { "edit" },
        "additions": additions,
        "deletions": deletions,
    });
    if !diff.is_empty() {
        kind["unified_diff"] = json!(truncate(&diff, MAX_BODY_CHARACTERS));
    }
    Some(kind)
}

fn result_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_owned());
    }
    let texts: Vec<&str> = content
        .as_array()?
        .iter()
        .filter(|block| block["type"].as_str() == Some("text"))
        .filter_map(|block| block["text"].as_str())
        .collect();
    (!texts.is_empty()).then(|| texts.join("\n"))
}

fn exit_code(output: &str) -> Option<i64> {
    let head: String = output.chars().take(200).collect();
    let rest = &head[head.find("Exit code")? + "Exit code".len()..];
    let rest = rest.strip_prefix(':').unwrap_or(rest).trim_start();
    let end = rest
        .char_indices()
        .find(|(index, c)| !(c.is_ascii_digit() || (*index == 0 && *c == '-')))
        .map_or(rest.len(), |(index, _)| index);
    rest[..end].parse().ok()
}

fn seq_of(message: &Value) -> i64 {
    message["seq"].as_i64().unwrap_or_default()
}

fn truncate(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

/// Claude stamps milliseconds; upstream decodes plain ISO 8601 (`.iso8601`), so drop them.
fn wire_timestamp(stamp: &str) -> String {
    match (stamp.find('.'), stamp.rfind(['Z', '+'])) {
        (Some(dot), Some(zone)) if dot < zone => format!("{}{}", &stamp[..dot], &stamp[zone..]),
        _ => stamp.to_owned(),
    }
}

/// A session id is a single path component: Claude's UUIDs, never `..` or a separator.
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Where Claude keeps a session: `<config>/projects/<cwd, every non-alphanumeric as '-'>/<id>.jsonl`,
/// else any project folder holding that id (the agent moved directory mid-session).
pub fn claude_transcript(
    config_dir: Option<&str>,
    cwd: Option<&str>,
    session_id: &str,
) -> Option<PathBuf> {
    if !valid_session_id(session_id) {
        return None;
    }
    let config = match config_dir {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".claude"),
    };
    let projects = config.join("projects");
    let file = format!("{session_id}.jsonl");
    if let Some(cwd) = cwd {
        let folder: String = cwd
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let path = projects.join(folder).join(&file);
        if path.is_file() {
            return Some(path);
        }
    }
    std::fs::read_dir(&projects)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(&file))
        .find(|path| path.is_file())
}

/// One terminal running a readable agent session, as the GTK side reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatTerminal {
    pub session_id: String,
    pub agent: String,
    pub workspace_id: String,
    pub terminal_id: String,
    pub title: String,
    pub cwd: Option<String>,
    pub config_dir: Option<String>,
    /// `idle`, `working` or `needs_input` (upstream `ChatAgentState`).
    pub state: &'static str,
}

impl ChatTerminal {
    pub fn from_value(value: &Value) -> Option<Self> {
        Some(Self {
            session_id: value["session_id"].as_str()?.to_owned(),
            agent: value["agent"].as_str()?.to_owned(),
            workspace_id: value["workspace_id"].as_str()?.to_owned(),
            terminal_id: value["terminal_id"].as_str()?.to_owned(),
            title: value["title"].as_str().unwrap_or_default().to_owned(),
            cwd: value["cwd"].as_str().map(str::to_owned),
            config_dir: value["config_dir"].as_str().map(str::to_owned),
            state: match value["state"].as_str() {
                Some("working") => "working",
                Some("needs_input") => "needs_input",
                _ => "idle",
            },
        })
    }

    pub fn transcript(&self) -> Option<PathBuf> {
        match self.agent.as_str() {
            "claude" => claude_transcript(
                self.config_dir.as_deref(),
                self.cwd.as_deref(),
                &self.session_id,
            ),
            _ => None,
        }
    }

    /// `ChatSessionDescriptor`; `since` and `last_activity_at` come from the transcript's mtime.
    pub fn descriptor(&self, last_activity: Option<&str>) -> Value {
        let mut state = Map::new();
        state.insert("state".into(), json!(self.state));
        if self.state != "idle" {
            state.insert(
                "since".into(),
                json!(last_activity.unwrap_or("1970-01-01T00:00:00Z")),
            );
        }
        let mut descriptor = json!({
            "session_id": self.session_id,
            "agent_kind": self.agent,
            "kind": "agent",
            "title": self.title,
            "workspace_id": self.workspace_id,
            "terminal_id": self.terminal_id,
            "state": state,
            "version": 0,
        });
        if let Some(cwd) = &self.cwd {
            descriptor["cwd"] = json!(cwd);
        }
        if let Some(at) = last_activity {
            descriptor["last_activity_at"] = json!(at);
        }
        descriptor
    }
}

/// A file's modification time as ISO 8601 UTC, for `last_activity_at`.
pub fn modified_at(path: &Path) -> Option<String> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let seconds = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    Some(iso8601(seconds))
}

fn iso8601(seconds: i64) -> String {
    // Civil-from-days (Howard Hinnant), enough for a UTC stamp without a date crate.
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(values: &[Value]) -> Vec<u8> {
        values
            .iter()
            .map(|value| format!("{value}\n"))
            .collect::<String>()
            .into_bytes()
    }

    #[test]
    /// Prose, thinking and a Bash call become messages with one seq per line; the result folds in.
    fn claude_lines_become_messages_and_results_fold_in() {
        let mut transcript = ClaudeTranscript::default();
        let change = transcript.feed(&lines(&[
            json!({"type": "user", "uuid": "u1", "timestamp": "2026-10-27T17:00:00.123Z",
                "message": {"content": "liste les fichiers"}}),
            json!({"type": "assistant", "uuid": "a1", "timestamp": "2026-10-27T17:00:01.000Z",
            "message": {"content": [
                {"type": "thinking", "thinking": "je regarde"},
                {"type": "text", "text": "Je lance ls."},
                {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}},
            ]}}),
        ]));
        let kinds: Vec<_> = change
            .appended
            .iter()
            .map(|m| m["kind"]["type"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["prose", "thought", "prose", "terminal"]);
        assert_eq!(change.appended[0]["role"], "user");
        assert_eq!(change.appended[0]["timestamp"], "2026-10-27T17:00:00Z");
        assert_eq!(change.appended[3]["id"], "a1#2");
        assert_eq!(change.appended[3]["seq"], 1);
        assert_eq!(change.appended[3]["kind"]["is_running"], true);

        let change = transcript.feed(&lines(&[json!({"type": "user", "uuid": "u2",
            "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "content": "a\nb"}]}})]));
        assert!(change.appended.is_empty());
        assert_eq!(change.updated.len(), 1);
        assert_eq!(change.updated[0]["id"], "a1#2");
        assert_eq!(change.updated[0]["kind"]["is_running"], false);
        assert_eq!(change.updated[0]["kind"]["output"], "a\nb");
    }

    #[test]
    /// A line split across two reads is parsed once complete; noise, meta and sidechain lines are skipped.
    fn partial_lines_wait_and_noise_is_skipped() {
        let mut transcript = ClaudeTranscript::default();
        let bytes = lines(&[
            json!({"type": "user", "message": {"content": "<command-name>/clear</command-name>"}}),
            json!({"type": "user", "isMeta": true, "message": {"content": "caveat"}}),
            json!({"type": "assistant", "isSidechain": true, "message": {"content": [{"type": "text", "text": "sub"}]}}),
            json!({"type": "assistant", "uuid": "a1", "message": {"content": [{"type": "text", "text": "fini"}]}}),
        ]);
        let cut = bytes.len() - 10;
        assert_eq!(transcript.feed(&bytes[..cut]), Change::default());
        let change = transcript.feed(&bytes[cut..]);
        assert_eq!(change.appended.len(), 1);
        assert_eq!(change.appended[0]["seq"], 3);
        assert_eq!(change.appended[0]["kind"]["text"], "fini");
    }

    #[test]
    /// Edits carry line counts; generic tools a summary; a failed result marks the tool failed.
    fn edits_and_generic_tools() {
        let mut transcript = ClaudeTranscript::default();
        transcript.feed(&lines(&[
            json!({"type": "assistant", "uuid": "a1", "message": {"content": [
                {"type": "tool_use", "id": "e", "name": "Edit",
                    "input": {"file_path": "/p/a.rs", "old_string": "x\ny", "new_string": "z"}},
                {"type": "tool_use", "id": "g", "name": "Grep", "input": {"pattern": "fn  main"}},
            ]}}),
            json!({"type": "user", "message": {"content": [
                {"type": "tool_result", "tool_use_id": "g", "is_error": true, "content": [{"type": "text", "text": "boom"}]}]}}),
        ]));
        let page = transcript.page(None, 10);
        let edit = &page["messages"][0]["kind"];
        assert_eq!(edit["type"], "file_edit");
        assert_eq!(
            (edit["additions"].as_i64(), edit["deletions"].as_i64()),
            (Some(1), Some(2))
        );
        assert_eq!(edit["unified_diff"], "-x\n-y\n+z\n");
        let tool = &page["messages"][1]["kind"];
        assert_eq!(tool["summary"], "Grep fn main");
        assert_eq!(tool["status"], "failed");
        assert_eq!(tool["output"], "boom");
    }

    #[test]
    /// Pages walk back from the newest message; has_more says whether older ones remain.
    fn history_pages_walk_back() {
        let mut transcript = ClaudeTranscript::default();
        let values: Vec<Value> = (0..5)
            .map(|n| json!({"type": "assistant", "uuid": format!("a{n}"), "message": {"content": format!("m{n}")}}))
            .collect();
        transcript.feed(&lines(&values));
        let page = transcript.page(None, 2);
        assert_eq!(page["messages"][0]["kind"]["text"], "m3");
        assert_eq!(page["has_more"], true);
        let page = transcript.page(Some(3), 10);
        assert_eq!(page["messages"].as_array().unwrap().len(), 3);
        assert_eq!(page["has_more"], false);
    }

    #[test]
    /// The file is read from where the last read stopped, and again whole after it shrank.
    fn reads_resume_at_the_offset() {
        let dir = std::env::temp_dir().join(format!("cmux-chat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        let line = |text: &str| {
            format!(
                "{}\n",
                json!({"type": "assistant", "message": {"content": text}})
            )
        };
        std::fs::write(&path, line("un")).unwrap();
        let mut transcript = ClaudeTranscript::default();
        assert_eq!(transcript.read_from(&path).unwrap().appended.len(), 1);
        std::fs::write(&path, line("un") + &line("deux")).unwrap();
        let change = transcript.read_from(&path).unwrap();
        assert_eq!(change.appended.len(), 1);
        assert_eq!(change.appended[0]["kind"]["text"], "deux");
        std::fs::write(&path, line("neuf")).unwrap();
        let change = transcript.read_from(&path).unwrap();
        assert_eq!(change.appended[0]["kind"]["text"], "neuf");
        assert_eq!(change.appended[0]["seq"], 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    /// The transcript sits in the cwd's project folder, found by id elsewhere; ids never escape it.
    fn transcript_path_follows_claude_layout() {
        let config = std::env::temp_dir().join(format!("cmux-claude-{}", std::process::id()));
        let folder = config.join("projects/-home-u-projets-mon-app");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("abc-1.jsonl"), "").unwrap();
        let config_dir = config.to_str();
        assert_eq!(
            claude_transcript(
                config_dir,
                Some("/home/u/projets/mon_app"),
                "abc-1"
            ),
            Some(folder.join("abc-1.jsonl"))
        );
        assert_eq!(
            claude_transcript(config_dir, Some("/ailleurs"), "abc-1"),
            Some(folder.join("abc-1.jsonl"))
        );
        assert_eq!(claude_transcript(config_dir, None, "../abc-1"), None);
        std::fs::remove_dir_all(&config).unwrap();
    }

    #[test]
    fn descriptor_and_stamps() {
        let terminal = ChatTerminal::from_value(&json!({"session_id": "s", "agent": "claude",
            "workspace_id": "w", "terminal_id": "t", "title": "Claude", "state": "working"}))
        .unwrap();
        let descriptor = terminal.descriptor(Some("2026-10-27T17:00:00Z"));
        assert_eq!(
            descriptor["state"],
            json!({"state": "working", "since": "2026-10-27T17:00:00Z"})
        );
        assert_eq!(descriptor["agent_kind"], "claude");
        assert_eq!(iso8601(1_793_120_400), "2026-10-27T17:00:00Z");
        assert_eq!(exit_code("Exit code 127\nnot found"), Some(127));
        assert_eq!(exit_code("ok"), None);
        assert_eq!(
            wire_timestamp("2026-10-27T17:00:00.5+02:00"),
            "2026-10-27T17:00:00+02:00"
        );
    }
}
