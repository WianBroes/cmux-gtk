//! `cmux events`: print the app's `events.stream` as newline-delimited JSON, optionally
//! persisting a cursor and reconnecting from the last received event (upstream `docs/events.md`).
use super::CliError;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};

pub struct Options {
    pub after: Option<u64>,
    pub cursor_file: Option<std::path::PathBuf>,
    pub names: Vec<String>,
    pub categories: Vec<String>,
    pub reconnect: bool,
    pub limit: Option<u64>,
    pub ack: bool,
    pub heartbeats: bool,
}

/// How one connection ended.
enum End {
    /// The requested number of events was printed.
    Done,
    /// The connection closed or the server dropped a slow subscription.
    Closed,
}

pub fn run(socket: &str, options: Options) -> Result<(), CliError> {
    let mut cursor = options.after.or_else(|| read_cursor(options.cursor_file.as_deref()));
    let mut printed = 0;
    loop {
        match stream_once(socket, &options, &mut cursor, &mut printed) {
            Ok(End::Done) => return Ok(()),
            Ok(End::Closed) if !options.reconnect => return Ok(()),
            Err(error) if !options.reconnect => return Err(error),
            Ok(End::Closed) | Err(_) => std::thread::sleep(std::time::Duration::from_secs(1)),
        }
    }
}

fn read_cursor(path: Option<&std::path::Path>) -> Option<u64> {
    std::fs::read_to_string(path?).ok()?.trim().parse().ok()
}

/// Replace the cursor file atomically so a crash never leaves a torn number.
fn write_cursor(path: &std::path::Path, seq: u64) -> Result<(), CliError> {
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, format!("{seq}\n"))
        .and_then(|()| std::fs::rename(&temporary, path))
        .map_err(|error| CliError::Output(format!("cursor file: {error}")))
}

fn stream_once(
    socket: &str,
    options: &Options,
    cursor: &mut Option<u64>,
    printed: &mut u64,
) -> Result<End, CliError> {
    let stream = cmux_platform::local_socket::connect(
        std::path::Path::new(socket),
        std::time::Duration::from_secs(5),
    )
    .map_err(|error| CliError::Connection(format!("cannot connect to {socket}: {error}")))?;
    // Heartbeats keep a live stream readable; only the connect is bounded.
    stream
        .set_read_timeout(None)
        .map_err(|error| CliError::Connection(error.to_string()))?;
    let request = json!({
        "id": "cmux-events", "method": "events.stream",
        "params": {
            "after_seq": cursor, "names": options.names, "categories": options.categories,
            "include_heartbeats": options.heartbeats,
        },
    });
    (&stream)
        .write_all(format!("{request}\n").as_bytes())
        .map_err(|error| CliError::Connection(error.to_string()))?;
    let mut stdout = std::io::stdout().lock();
    for line in BufReader::new(&stream).lines() {
        let line = line.map_err(|error| CliError::Connection(error.to_string()))?;
        let frame: Value = serde_json::from_str(&line)
            .map_err(|_| CliError::Protocol("invalid event frame".into()))?;
        match frame["type"].as_str() {
            Some("ack") if !options.ack => continue,
            // Reconnecting clients see the ack once, like the first connection.
            Some("ack") if *printed > 0 => continue,
            Some("heartbeat") if !options.heartbeats => continue,
            Some("ack" | "heartbeat") => {}
            Some("event") => {
                *cursor = frame["seq"].as_u64().or(*cursor);
                *printed += 1;
            }
            _ => {
                let message = frame["error"]["message"].as_str().unwrap_or("stream error");
                eprintln!("cmux events: {message}");
                return if frame["error"]["code"] == "slow_consumer" {
                    Ok(End::Closed)
                } else {
                    Err(CliError::Command(message.to_owned()))
                };
            }
        }
        writeln!(stdout, "{line}")
            .and_then(|()| stdout.flush())
            .map_err(|error| CliError::Output(error.to_string()))?;
        if frame["type"] == "event" {
            if let (Some(path), Some(seq)) = (&options.cursor_file, *cursor) {
                write_cursor(path, seq)?;
            }
            if options.limit.is_some_and(|limit| *printed >= limit) {
                return Ok(End::Done);
            }
        }
    }
    Ok(End::Closed)
}
