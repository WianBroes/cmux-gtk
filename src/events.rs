//! Reconnectable event stream, following upstream `docs/events.md`: every event gets a
//! process-local `seq` under a per-boot `boot_id`, is retained in a bounded replay buffer,
//! delivered to live socket subscribers and appended to a bounded JSONL audit log.
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Condvar, Mutex, OnceLock};

pub const PROTOCOL: &str = "cmux-events";
pub const VERSION: u64 = 1;
pub const HEARTBEAT_SECONDS: u64 = 15;
/// Pending events per live subscriber before it is closed as a slow consumer.
pub const SUBSCRIBER_QUEUE: usize = 1024;
const REPLAY_LIMIT: usize = 4096;
const FRAME_LIMIT: usize = 16 * 1024;
const LOG_FILE_LIMIT: u64 = 16 * 1024 * 1024;
const LOG_QUEUE_LIMIT: usize = 1024;

/// One encoded event frame, shared by the replay buffer and every subscriber.
pub struct Event {
    pub seq: u64,
    pub name: String,
    pub category: String,
    pub line: String,
}

/// Identity of what an event is about; every field is optional.
#[derive(Default)]
pub struct Scope {
    pub workspace: Option<uuid::Uuid>,
    pub surface: Option<String>,
    pub pane: Option<String>,
}

/// Subscription filters; empty lists match everything.
#[derive(Default, Clone)]
pub struct Filter {
    pub names: Vec<String>,
    pub categories: Vec<String>,
}

impl Filter {
    pub fn matches(&self, event: &Event) -> bool {
        (self.names.is_empty() || self.names.contains(&event.name))
            && (self.categories.is_empty() || self.categories.contains(&event.category))
    }
}

struct Bus {
    boot_id: uuid::Uuid,
    next_seq: u64,
    replay: VecDeque<Arc<Event>>,
    live: tokio::sync::broadcast::Sender<Arc<Event>>,
}

fn bus() -> &'static Mutex<Bus> {
    static BUS: OnceLock<Mutex<Bus>> = OnceLock::new();
    BUS.get_or_init(|| {
        Mutex::new(Bus {
            boot_id: uuid::Uuid::new_v4(),
            next_seq: 1,
            replay: VecDeque::new(),
            live: tokio::sync::broadcast::channel(SUBSCRIBER_QUEUE).0,
        })
    })
}

pub fn boot_id() -> uuid::Uuid {
    bus().lock().unwrap().boot_id
}

fn now() -> String {
    glib::DateTime::now_utc()
        .and_then(|date| date.format_iso8601())
        .map(|value| value.to_string())
        .unwrap_or_default()
}

/// The coarse subscription group: the name's first segment (`agent.hook.Stop` → `agent`).
fn category(name: &str) -> &str {
    name.split('.').next().unwrap_or(name)
}

/// Record one event; never blocks on subscribers or the disk.
pub fn publish(name: &str, source: &str, scope: Scope, payload: Value) {
    let mut bus = bus().lock().unwrap();
    let seq = bus.next_seq;
    bus.next_seq += 1;
    let mut frame = json!({
        "type": "event", "protocol": PROTOCOL, "version": VERSION,
        "boot_id": bus.boot_id, "seq": seq, "id": format!("{}-{seq}", bus.boot_id),
        "name": name, "category": category(name), "source": source, "occurred_at": now(),
        "workspace_id": scope.workspace, "surface_id": scope.surface,
        "pane_id": scope.pane, "window_id": null, "payload": payload,
    });
    let mut line = frame.to_string();
    if line.len() > FRAME_LIMIT {
        frame["payload"] = json!({"payload_truncated": true});
        line = frame.to_string();
    }
    let event = Arc::new(Event {
        seq,
        name: name.to_owned(),
        category: category(name).to_owned(),
        line,
    });
    if bus.replay.len() == REPLAY_LIMIT {
        bus.replay.pop_front();
    }
    bus.replay.push_back(event.clone());
    // No receiver is normal: nobody is streaming.
    let _ = bus.live.send(event.clone());
    drop(bus);
    // Tests share the process-wide bus but must never write the user's audit log.
    if cfg!(not(test)) {
        log_queue().push(event.line.clone());
    }
}

/// Atomically compute the ack, the matching replay after `after_seq` and a live receiver,
/// so no event is lost or duplicated between replay and live delivery.
pub fn subscribe(
    after_seq: Option<u64>,
    filter: &Filter,
    heartbeats: bool,
) -> (Value, Vec<Arc<Event>>, tokio::sync::broadcast::Receiver<Arc<Event>>) {
    let bus = bus().lock().unwrap();
    let latest = bus.next_seq - 1;
    let oldest = bus.replay.front().map_or(bus.next_seq, |event| event.seq);
    let after = after_seq.unwrap_or(latest);
    // Older than what is retained, or from a previous process with a higher sequence.
    let gap = after_seq.is_some_and(|after| after + 1 < oldest || after > latest);
    let effective = if after > latest { latest } else { after };
    let replay: Vec<_> = bus
        .replay
        .iter()
        .filter(|event| event.seq > effective && filter.matches(event))
        .cloned()
        .collect();
    let ack = json!({
        "type": "ack", "protocol": PROTOCOL, "version": VERSION, "boot_id": bus.boot_id,
        "subscription_id": uuid::Uuid::new_v4(),
        "heartbeat_interval_seconds": if heartbeats { HEARTBEAT_SECONDS } else { 0 },
        "replay_count": replay.len(),
        "resume": {
            "after_seq": effective, "requested_after_seq": after_seq,
            "oldest_seq": oldest, "latest_seq": latest, "next_seq": bus.next_seq, "gap": gap,
        },
        "filters": {"names": filter.names, "categories": filter.categories},
    });
    (ack, replay, bus.live.subscribe())
}

pub fn heartbeat(subscription: &Value) -> String {
    let latest = bus().lock().unwrap().next_seq - 1;
    json!({
        "type": "heartbeat", "protocol": PROTOCOL, "version": VERSION, "boot_id": boot_id(),
        "subscription_id": subscription, "latest_seq": latest, "occurred_at": now(),
    })
    .to_string()
}

/// Accept a string or an array of strings under either of two parameter names.
fn strings(params: &Value, plural: &str, singular: &str) -> Result<Vec<String>, &'static str> {
    let value = params.get(plural).or_else(|| params.get(singular));
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(one)) => Ok(vec![one.clone()]),
        Some(Value::Array(many)) => many
            .iter()
            .map(|item| item.as_str().map(ToOwned::to_owned))
            .collect::<Option<_>>()
            .ok_or("filters must be strings"),
        Some(_) => Err("filters must be a string or an array of strings"),
    }
}

/// Parse `events.stream` params: `(after_seq, filter, include_heartbeats)`.
pub fn parse_stream(params: &Value) -> Result<(Option<u64>, Filter, bool), &'static str> {
    let after = match params.get("after_seq").or_else(|| params.get("after")) {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_u64().ok_or("after_seq must be a non-negative integer")?),
    };
    let filter = Filter {
        names: strings(params, "names", "name")?,
        categories: strings(params, "categories", "category")?,
    };
    let heartbeats = match params.get("include_heartbeats") {
        None | Some(Value::Null) => true,
        Some(value) => value.as_bool().ok_or("include_heartbeats must be a boolean")?,
    };
    Ok((after, filter, heartbeats))
}

/// Disk lines waiting for the writer thread; the oldest are dropped under backpressure.
struct LogQueue {
    lines: Mutex<VecDeque<String>>,
    ready: Condvar,
}

impl LogQueue {
    fn push(&self, line: String) {
        let mut lines = self.lines.lock().unwrap();
        if lines.len() == LOG_QUEUE_LIMIT {
            lines.pop_front();
        }
        lines.push_back(line);
        self.ready.notify_one();
    }
}

fn log_queue() -> &'static LogQueue {
    static QUEUE: OnceLock<Arc<LogQueue>> = OnceLock::new();
    QUEUE.get_or_init(|| {
        let queue = Arc::new(LogQueue {
            lines: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
        });
        let writer = queue.clone();
        let path = cmux_platform::paths::state_dir().join("events.jsonl");
        let _ = std::thread::Builder::new()
            .name("cmux-events-log".into())
            .spawn(move || write_log(&writer, &path));
        queue
    })
}

/// Append batches to `events.jsonl`, rotating to `events.jsonl.1` at the size cap.
fn write_log(queue: &LogQueue, path: &std::path::Path) {
    loop {
        let batch: Vec<String> = {
            let mut lines = queue.lines.lock().unwrap();
            while lines.is_empty() {
                lines = queue.ready.wait(lines).unwrap();
            }
            lines.drain(..).collect()
        };
        let _ = append(path, &batch);
    }
}

fn append(path: &std::path::Path, batch: &[String]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        cmux_platform::filesystem::create_private_directory(parent)?;
    }
    let incoming: u64 = batch.iter().map(|line| line.len() as u64 + 1).sum();
    if std::fs::metadata(path).is_ok_and(|meta| meta.len() + incoming > LOG_FILE_LIMIT) {
        std::fs::rename(path, path.with_extension("jsonl.1"))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let mut buffer = String::with_capacity(incoming as usize);
    for line in batch {
        buffer.push_str(line);
        buffer.push('\n');
    }
    file.write_all(buffer.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Replay honours the cursor and filters; the ack reports gaps for cursors the process cannot serve.
    #[test]
    fn replay_filters_and_gap() {
        publish("test.replay.one", "test", Scope::default(), json!({}));
        let (ack, _, _) = subscribe(None, &Filter::default(), true);
        let start = ack["resume"]["latest_seq"].as_u64().unwrap();
        publish("test.replay.two", "test", Scope::default(), json!({"n": 2}));
        publish("notification.created", "test", Scope::default(), json!({}));
        let only_test = Filter {
            names: vec!["test.replay.two".into()],
            categories: Vec::new(),
        };
        let (ack, replay, _) = subscribe(Some(start), &only_test, true);
        assert_eq!(ack["resume"]["gap"], false);
        assert!(replay.iter().all(|event| event.name == "test.replay.two"));
        assert!(!replay.is_empty());
        let (ack, _, _) = subscribe(Some(u64::MAX / 2), &Filter::default(), true);
        assert_eq!(ack["resume"]["gap"], true);
        let categories = Filter {
            names: Vec::new(),
            categories: vec!["notification".into()],
        };
        let (_, replay, _) = subscribe(Some(start), &categories, true);
        assert!(replay.iter().all(|event| event.category == "notification"));
    }

    #[test]
    fn oversized_payload_is_truncated() {
        publish(
            "test.big",
            "test",
            Scope::default(),
            json!({"blob": "x".repeat(FRAME_LIMIT)}),
        );
        let (_, replay, _) = subscribe(Some(0), &Filter { names: vec!["test.big".into()], categories: vec![] }, true);
        let frame: Value = serde_json::from_str(&replay.last().unwrap().line).unwrap();
        assert_eq!(frame["payload"]["payload_truncated"], true);
    }

    #[test]
    fn stream_params_accept_aliases() {
        let (after, filter, heartbeats) =
            parse_stream(&json!({"after": 5, "category": "feed", "include_heartbeats": false}))
                .unwrap();
        assert_eq!(after, Some(5));
        assert_eq!(filter.categories, vec!["feed".to_string()]);
        assert!(!heartbeats);
        assert!(parse_stream(&json!({"after_seq": -1})).is_err());
    }
}
