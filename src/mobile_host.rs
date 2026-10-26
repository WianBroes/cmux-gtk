//! Android mobile host: a bounded TCP JSON-RPC listener for the prototype contract.
//!
//! Frames are a big-endian 32-bit length followed by UTF-8 JSON. Requests run against the
//! existing GTK socket commands, so terminal reads, input and workspace state keep one owner.
//! The listener binds only loopback and Tailscale IPv4 addresses; authentication is the
//! `attach_token` in each request, except for the public `mobile.host.status` method.

use crate::mobile;
use crate::socket::commands::{self, MobileSizingOp, SocketCommand};
use serde_json::{json, Value};
use std::io;
use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::mpsc;

/// Start one accept loop per permitted local address. Binding failures are logged and do not
/// stop the application; a host with no Tailscale address still serves loopback.
pub fn start(runtime: &tokio::runtime::Handle, cmd_tx: mpsc::Sender<SocketCommand>) {
    let token = match mobile::load_or_create_token() {
        Ok(token) => Arc::new(token),
        Err(error) => {
            eprintln!("cmux: mobile host token unavailable: {error}");
            return;
        }
    };
    if mobile::tailscale_addresses()
        .map(|addresses| addresses.is_empty())
        .unwrap_or(true)
    {
        eprintln!("cmux: mobile host has no Tailscale IPv4 address; listening on loopback only");
    }
    let mut bound = 0usize;
    for address in mobile::listen_addresses() {
        let socket = SocketAddr::from((address, mobile::PORT));
        let listener = match StdTcpListener::bind(socket) {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("cmux: mobile host bind failed at {socket}: {error}");
                continue;
            }
        };
        if let Err(error) = listener.set_nonblocking(true) {
            eprintln!("cmux: mobile host nonblocking setup failed at {socket}: {error}");
            continue;
        }
        let token = token.clone();
        let cmd_tx = cmd_tx.clone();
        runtime.spawn(async move {
            let listener = match tokio::net::TcpListener::from_std(listener) {
                Ok(listener) => listener,
                Err(error) => {
                    eprintln!("cmux: mobile host listener setup failed at {socket}: {error}");
                    return;
                }
            };
            loop {
                match listener.accept().await {
                    Ok((stream, peer)) => {
                        let token = token.clone();
                        let cmd_tx = cmd_tx.clone();
                        tokio::spawn(async move {
                            handle_connection(stream, peer, token, cmd_tx).await;
                        });
                    }
                    Err(error) => {
                        eprintln!("cmux: mobile host accept failed: {error}");
                        break;
                    }
                }
            }
        });
        bound += 1;
    }
    if bound == 0 {
        eprintln!(
            "cmux: mobile host not listening on port {} (no bindable address)",
            mobile::PORT
        );
    }
}

/// Serve framed requests from one authenticated TCP peer until it disconnects.
async fn handle_connection(
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
    token: Arc<String>,
    cmd_tx: mpsc::Sender<SocketCommand>,
) {
    let (reader, writer) = stream.into_split();
    // Requests keep flowing after a subscription: events go out from their own task.
    let writer = Arc::new(tokio::sync::Mutex::new(writer));
    let mut events: Option<tokio::task::JoinHandle<()>> = None;
    // Wakes the event task right after a scroll or input instead of at the next tick.
    let wake = Arc::new(tokio::sync::Notify::new());
    // Shared sizing: this phone's participant id and the terminal it is sized on
    // (one at a time, like upstream); it leaves on disconnect.
    static NEXT_CLIENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let mut sizing_id = format!(
        "mobile:conn-{}",
        NEXT_CLIENT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let mut sized: Option<String> = None;
    let mut reader = BufReader::new(reader);
    loop {
        let frame = match read_frame(&mut reader).await {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(error) => {
                let code = if error.kind() == io::ErrorKind::InvalidData {
                    "frame_too_large"
                } else {
                    "invalid_request"
                };
                let _ = send_json(&writer, &error_response(Value::Null, code, &error.to_string())).await;
                break;
            }
        };
        let request: Value = match serde_json::from_slice(&frame) {
            Ok(request) => request,
            Err(_) => {
                let _ = send_json(
                    &writer,
                    &error_response(Value::Null, "parse_error", "invalid JSON"),
                )
                .await;
                continue;
            }
        };
        let Some(object) = request.as_object() else {
            let _ = send_json(
                &writer,
                &error_response(
                    Value::Null,
                    "invalid_request",
                    "request must be an object",
                ),
            )
            .await;
            continue;
        };
        let id = object.get("id").cloned().unwrap_or(Value::Null);
        let method = match object.get("method").and_then(Value::as_str) {
            Some(method) if !method.trim().is_empty() => method,
            _ => {
                let _ = send_json(
                    &writer,
                    &error_response(id, "invalid_request", "method must be a string"),
                )
                .await;
                continue;
            }
        };
        let params = match object.get("params") {
            None | Some(Value::Null) => json!({}),
            Some(Value::Object(params)) => Value::Object(params.clone()),
            Some(_) => {
                let _ = send_json(
                    &writer,
                    &error_response(id, "invalid_params", "params must be an object"),
                )
                .await;
                continue;
            }
        };
        if !mobile::authorized(&request, &token) {
            crate::diagnostics::record(
                "mobile.auth.rejected",
                json!({"method": method, "peer": peer.ip().to_string()}),
            );
            let _ = send_json(
                &writer,
                &error_response(id, "unauthorized", "invalid attach_token"),
            )
            .await;
            continue;
        }

        match method {
            "mobile.host.status" => {
                let workspace_count = workspace_count(&cmd_tx).await;
                let response = success_response(id, mobile::host_status(workspace_count));
                if send_json(&writer, &response).await.is_err() {
                    break;
                }
            }
            "mobile.workspace.list" => {
                match gtk_call(&cmd_tx, |resp_tx| SocketCommand::MobileWorkspaceList {
                    req_id: Value::Null,
                    resp_tx,
                })
                .await
                {
                    Ok(result) => {
                        if send_json(&writer, &success_response(id, result))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err((code, message)) => {
                        let _ = send_json(&writer, &error_response(id, &code, &message)).await;
                    }
                }
            }
            "mobile.terminal.replay" => {
                let Some(surface_id) = params.get("surface_id").and_then(Value::as_str) else {
                    let _ = send_json(
                        &writer,
                        &error_response(id, "invalid_params", "Missing surface_id"),
                    )
                    .await;
                    continue;
                };
                match read_terminal_text(&cmd_tx, surface_id).await {
                    Ok(TerminalScreen { text, rows_above, columns, render_grid }) => {
                        let result = json!({
                            "surface_id": surface_id,
                            "text": text,
                            "rows_above": rows_above,
                            "columns": columns,
                            "render_grid": render_grid,
                        });
                        if send_json(&writer, &success_response(id, result))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err((code, message)) => {
                        let _ = send_json(&writer, &error_response(id, &code, &message)).await;
                    }
                }
            }
            "mobile.terminal.input" => {
                let text = match params.get("text").and_then(Value::as_str) {
                    Some(text) if !text.is_empty() => text.to_owned(),
                    _ => {
                        let _ = send_json(
                            &writer,
                            &error_response(id, "invalid_params", "Missing text"),
                        )
                        .await;
                        continue;
                    }
                };
                let Some(surface_id) = params.get("surface_id").and_then(Value::as_str) else {
                    let _ = send_json(
                        &writer,
                        &error_response(id, "invalid_params", "Missing surface_id"),
                    )
                    .await;
                    continue;
                };
                // Typed input is delivered as key presses so control bytes stay real
                // keys instead of landing in the terminal as bracketed paste.
                let keys = mobile::input_keys(&text);
                let key_count = keys.len();
                let mut first_error: Option<(String, String)> = None;
                for key in keys {
                    let delivered = gtk_call(&cmd_tx, |resp_tx| SocketCommand::SurfaceSendKey {
                        req_id: Value::Null,
                        id: Some(surface_id.to_owned()),
                        workspace: None,
                        key,
                        resp_tx,
                    })
                    .await;
                    if let Err(error) = delivered {
                        first_error = Some(error);
                        break;
                    }
                }
                if let Some((code, message)) = first_error {
                    let _ = send_json(&writer, &error_response(id, &code, &message)).await;
                    continue;
                }
                if sized.as_deref() == Some(surface_id) {
                    // Follow latest: typing from the phone makes it the owner (upstream 0.65.1).
                    let _ = sizing_call(&cmd_tx, MobileSizingOp::Activity {
                        surface_id: surface_id.to_owned(),
                        participant_id: sizing_id.clone(),
                    })
                    .await;
                }
                wake.notify_one();
                let result = json!({"surface_id": surface_id, "keys": key_count});
                if send_json(&writer, &success_response(id, result))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            "mobile.terminal.activity" => {
                // Not upstream: the phone took focus (tapped the input field or the
                // terminal) without sending anything yet. Same effect as a keystroke
                // for Follow latest, like the Mac's focus-click.
                let Some(surface_id) = params.get("surface_id").and_then(Value::as_str) else {
                    let _ = send_json(
                        &writer,
                        &error_response(id, "invalid_params", "Missing surface_id"),
                    )
                    .await;
                    continue;
                };
                let noted = sized.as_deref() == Some(surface_id);
                if noted {
                    let _ = sizing_call(&cmd_tx, MobileSizingOp::Activity {
                        surface_id: surface_id.to_owned(),
                        participant_id: sizing_id.clone(),
                    })
                    .await;
                }
                let result = json!({"surface_id": surface_id, "noted": noted});
                if send_json(&writer, &success_response(id, result))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            "mobile.terminal.scroll" => {
                let Some(surface_id) = params.get("surface_id").and_then(Value::as_str) else {
                    let _ = send_json(
                        &writer,
                        &error_response(id, "invalid_params", "Missing surface_id"),
                    )
                    .await;
                    continue;
                };
                let delta_lines = params
                    .get("delta_lines")
                    .and_then(Value::as_f64)
                    .filter(|delta| delta.is_finite())
                    .unwrap_or(0.0);
                if delta_lines != 0.0 {
                    let scrolled = gtk_call(&cmd_tx, |resp_tx| SocketCommand::SurfaceMouseScroll {
                        req_id: Value::Null,
                        id: Some(surface_id.to_owned()),
                        delta_lines,
                        resp_tx,
                    })
                    .await;
                    if let Err((code, message)) = scrolled {
                        let _ = send_json(&writer, &error_response(id, &code, &message)).await;
                        continue;
                    }
                }
                wake.notify_one();
                let result = json!({"surface_id": surface_id, "delta_lines": delta_lines});
                if send_json(&writer, &success_response(id, result))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            "mobile.terminal.history" => {
                // The phone scrolls its own copy of the history (upstream iOS owns
                // primary-screen scrolling): rows just above the screen, end_row
                // matching the rows_above of terminal.text.
                let Some(surface_id) = params.get("surface_id").and_then(Value::as_str) else {
                    let _ = send_json(
                        &writer,
                        &error_response(id, "invalid_params", "Missing surface_id"),
                    )
                    .await;
                    continue;
                };
                let max_lines = params
                    .get("max_lines")
                    .and_then(Value::as_u64)
                    .unwrap_or(1000)
                    .min(2000);
                let read = gtk_call(&cmd_tx, |resp_tx| SocketCommand::SurfaceReadHistory {
                    req_id: Value::Null,
                    id: Some(surface_id.to_owned()),
                    max_lines,
                    resp_tx,
                })
                .await;
                let response = match read {
                    Ok(result) => success_response(
                        id,
                        json!({
                            "surface_id": surface_id,
                            "text": result.get("text").cloned().unwrap_or_else(|| json!("")),
                            "end_row": result.get("end_row").cloned().unwrap_or_else(|| json!(0)),
                        }),
                    ),
                    Err((code, message)) => error_response(id, &code, &message),
                };
                if send_json(&writer, &response).await.is_err() {
                    break;
                }
            }
            "mobile.terminal.paste" => {
                let text = match params.get("text").and_then(Value::as_str) {
                    Some(text) if !text.is_empty() => text.to_owned(),
                    _ => {
                        let _ = send_json(
                            &writer,
                            &error_response(id, "invalid_params", "Missing text"),
                        )
                        .await;
                        continue;
                    }
                };
                let Some(surface_id) = params.get("surface_id").and_then(Value::as_str) else {
                    let _ = send_json(
                        &writer,
                        &error_response(id, "invalid_params", "Missing surface_id"),
                    )
                    .await;
                    continue;
                };
                let pasted = gtk_call(&cmd_tx, |resp_tx| SocketCommand::SurfaceSendText {
                    req_id: Value::Null,
                    id: Some(surface_id.to_owned()),
                    workspace: None,
                    text,
                    resp_tx,
                })
                .await;
                if pasted.is_ok() && sized.as_deref() == Some(surface_id) {
                    // Paste is explicit input for Follow latest, as on the Mac.
                    let _ = sizing_call(&cmd_tx, MobileSizingOp::Activity {
                        surface_id: surface_id.to_owned(),
                        participant_id: sizing_id.clone(),
                    })
                    .await;
                }
                match pasted {
                    Ok(result) => {
                        if send_json(&writer, &success_response(id, result))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err((code, message)) => {
                        let _ = send_json(&writer, &error_response(id, &code, &message)).await;
                    }
                }
            }
            "mobile.terminal.paste_image" | "terminal.paste_image" => {
                // Upstream `v2MobileTerminalPasteImage`: the phone sends the image as
                // base64; it is saved to a temporary file and its shell-escaped path
                // typed into the terminal, like a local image paste (Claude Code
                // attaches it from the path).
                use base64::Engine as _;
                let Some(surface_id) = params.get("surface_id").and_then(Value::as_str) else {
                    let _ = send_json(
                        &writer,
                        &error_response(id, "invalid_params", "Missing surface_id"),
                    )
                    .await;
                    continue;
                };
                let image = params
                    .get("image_base64")
                    .and_then(Value::as_str)
                    .and_then(|text| base64::engine::general_purpose::STANDARD.decode(text).ok())
                    .filter(|image| !image.is_empty());
                let Some(image) = image else {
                    let _ = send_json(
                        &writer,
                        &error_response(id, "invalid_params", "Missing or invalid image_base64"),
                    )
                    .await;
                    continue;
                };
                let format = params.get("image_format").and_then(Value::as_str).unwrap_or("png");
                let path = match mobile::save_pasted_image(&std::env::temp_dir(), &image, format) {
                    Ok(path) => path,
                    Err(_) => {
                        let _ = send_json(
                            &writer,
                            &error_response(
                                id,
                                "invalid_params",
                                "Image payload was empty or exceeded the size limit",
                            ),
                        )
                        .await;
                        continue;
                    }
                };
                let text = crate::ghostty::text::shell_escape(&path.to_string_lossy());
                let pasted = gtk_call(&cmd_tx, |resp_tx| SocketCommand::SurfaceSendText {
                    req_id: Value::Null,
                    id: Some(surface_id.to_owned()),
                    workspace: None,
                    text,
                    resp_tx,
                })
                .await;
                if pasted.is_ok() && sized.as_deref() == Some(surface_id) {
                    let _ = sizing_call(&cmd_tx, MobileSizingOp::Activity {
                        surface_id: surface_id.to_owned(),
                        participant_id: sizing_id.clone(),
                    })
                    .await;
                }
                let response = match pasted {
                    Ok(_) => success_response(
                        id,
                        json!({"surface_id": surface_id, "path": path.to_string_lossy()}),
                    ),
                    Err((code, message)) => {
                        let _ = std::fs::remove_file(&path);
                        error_response(id, &code, &message)
                    }
                };
                if send_json(&writer, &response).await.is_err() {
                    break;
                }
            }
            "mobile.terminal.viewport" | "mobile.terminal.size_policy.set" | "mobile.terminal.size_state" => {
                let Some(surface_id) = params.get("surface_id").and_then(Value::as_str) else {
                    let _ = send_json(
                        &writer,
                        &error_response(id, "invalid_params", "Missing surface_id"),
                    )
                    .await;
                    continue;
                };
                let op = match method {
                    "mobile.terminal.viewport" => {
                        if let Some(client) = params.get("client_id").and_then(Value::as_str) {
                            if !client.is_empty() {
                                sizing_id = format!("mobile:{client}");
                            }
                        }
                        // A phone sizes one terminal at a time: leave the previous one.
                        if let Some(previous) = sized.take().filter(|previous| previous != surface_id) {
                            let _ = sizing_call(&cmd_tx, MobileSizingOp::Detach {
                                surface_id: previous,
                                participant_id: sizing_id.clone(),
                            })
                            .await;
                        }
                        sized = Some(surface_id.to_owned());
                        MobileSizingOp::Viewport {
                            surface_id: surface_id.to_owned(),
                            participant: viewport_participant(&sizing_id, &params),
                        }
                    }
                    "mobile.terminal.size_policy.set" => {
                        let Some(policy) = params
                            .get("policy")
                            .cloned()
                            .and_then(|policy| serde_json::from_value(policy).ok())
                        else {
                            let _ = send_json(
                                &writer,
                                &error_response(id, "invalid_params", "Missing or invalid policy"),
                            )
                            .await;
                            continue;
                        };
                        MobileSizingOp::Policy { surface_id: surface_id.to_owned(), policy }
                    }
                    _ => MobileSizingOp::Refresh { surface_id: surface_id.to_owned() },
                };
                let response = match sizing_call(&cmd_tx, op).await {
                    Ok(size_state) => {
                        wake.notify_one();
                        success_response(
                            id,
                            json!({
                                "surface_id": surface_id,
                                "size_state": size_state,
                                "self_participant_id": sizing_id,
                            }),
                        )
                    }
                    Err((code, message)) => error_response(id, &code, &message),
                };
                if send_json(&writer, &response).await.is_err() {
                    break;
                }
            }
            "mobile.events.subscribe" => {
                let topics = params.get("topics").and_then(Value::as_array);
                if !topics.is_some_and(|topics| {
                    topics
                        .iter()
                        .any(|topic| topic.as_str() == Some("terminal.text"))
                }) {
                    let _ = send_json(
                        &writer,
                        &error_response(
                            id,
                            "invalid_params",
                            "topics must include terminal.text",
                        ),
                    )
                    .await;
                    continue;
                }
                let Some(surface_id) = params.get("surface_id").and_then(Value::as_str) else {
                    let _ = send_json(
                        &writer,
                        &error_response(id, "invalid_params", "Missing surface_id"),
                    )
                    .await;
                    continue;
                };
                let ack = success_response(
                    id,
                    json!({
                        "subscribed": true,
                        "topics": ["terminal.text"],
                        "surface_id": surface_id,
                    }),
                );
                if send_json(&writer, &ack).await.is_err() {
                    break;
                }
                if let Some(previous) = events.take() {
                    previous.abort();
                }
                events = Some(tokio::spawn(stream_terminal_events(
                    writer.clone(),
                    surface_id.to_owned(),
                    cmd_tx.clone(),
                    wake.clone(),
                )));
            }
            "mobile.app.latest" => {
                let response = success_response(id, mobile::app_latest().unwrap_or(Value::Null));
                if send_json(&writer, &response).await.is_err() {
                    break;
                }
            }
            "mobile.app.chunk" => {
                let offset = match params.get("offset").and_then(Value::as_u64) {
                    Some(offset) => offset,
                    None => {
                        let _ = send_json(
                            &writer,
                            &error_response(id, "invalid_params", "offset must be a non-negative integer"),
                        )
                        .await;
                        continue;
                    }
                };
                let length = match params.get("length").and_then(Value::as_u64) {
                    Some(length) => length,
                    None => {
                        let _ = send_json(
                            &writer,
                            &error_response(id, "invalid_params", "length must be a non-negative integer"),
                        )
                        .await;
                        continue;
                    }
                };
                match mobile::app_chunk(offset, length) {
                    Ok(result) => {
                        if send_json(&writer, &success_response(id, result))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err((code, message)) => {
                        let _ = send_json(&writer, &error_response(id, &code, &message)).await;
                    }
                }
            }
            _ => {
                let _ = send_json(
                    &writer,
                    &error_response(id, "method_not_found", "unknown mobile method"),
                )
                .await;
            }
        }
    }
    if let Some(task) = events {
        task.abort();
    }
    if let Some(surface_id) = sized {
        let _ = sizing_call(&cmd_tx, MobileSizingOp::Detach {
            surface_id,
            participant_id: sizing_id,
        })
        .await;
    }
}

/// This phone as a sizing participant, from `mobile.terminal.viewport` params
/// (upstream MobileTerminalViewportParameters.report).
fn viewport_participant(id: &str, params: &Value) -> crate::mobile_sizing::TerminalSizingParticipant {
    use crate::mobile_sizing::{TerminalDeviceKind, TerminalGridSize, TerminalSizingParticipant};
    let text = |key: &str| params.get(key).and_then(Value::as_str).map(str::to_owned);
    let cells = |key: &str| {
        params
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .filter(|value| *value > 0)
    };
    let kind = text("device_kind").map(|kind| TerminalDeviceKind::parse(&kind)).unwrap_or_default();
    let mut participant = TerminalSizingParticipant::new(id, kind);
    participant.device_name = text("device_name");
    participant.device_id = text("device_id");
    participant.viewport = match (cells("viewport_columns"), cells("viewport_rows")) {
        (Some(cols), Some(rows)) => Some(TerminalGridSize { cols, rows }),
        _ => None,
    };
    participant
}

/// Run one shared-sizing event on the GTK thread; the terminal's `size_state` or null.
async fn sizing_call(cmd_tx: &mpsc::Sender<SocketCommand>, op: MobileSizingOp) -> Result<Value, (String, String)> {
    let result = gtk_call(cmd_tx, |resp_tx| SocketCommand::MobileSizing { op, resp_tx }).await?;
    Ok(result.get("size_state").cloned().unwrap_or(Value::Null))
}

/// Poll one terminal at four frames per second, or at once when `wake` fires
/// (scroll, input), and emit its full text whenever it changes.
async fn stream_terminal_events(
    writer: Arc<tokio::sync::Mutex<OwnedWriteHalf>>,
    surface_id: String,
    cmd_tx: mpsc::Sender<SocketCommand>,
    wake: Arc<tokio::sync::Notify>,
) {
    let mut sequence = 0u64;
    let mut last: Option<TerminalScreen> = None;
    let mut last_size: Value = Value::Null;
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(250));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = wake.notified() => {}
        }
        // Shared sizing: re-measure the PC pane and push the state when it changes.
        if let Ok(size_state) = sizing_call(&cmd_tx, MobileSizingOp::Refresh { surface_id: surface_id.clone() }).await {
            if !size_state.is_null() && size_state != last_size {
                let event = json!({
                    "kind": "event",
                    "topic": "mobile.terminal.size_state",
                    "payload": {"surface_id": surface_id, "size_state": size_state},
                });
                last_size = size_state;
                if send_json(&writer, &event).await.is_err() {
                    return;
                }
            }
        }
        let Ok(screen) = read_terminal_text(&cmd_tx, &surface_id).await else {
            continue;
        };
        if last.as_ref() == Some(&screen) {
            continue;
        }
        sequence += 1;
        let TerminalScreen { text, rows_above, columns, render_grid } = &screen;
        let event = json!({
            "kind": "event",
            "topic": "terminal.text",
            "payload": {
                "surface_id": surface_id,
                "text": text,
                "rows_above": rows_above,
                "columns": columns,
                "render_grid": render_grid,
                "seq": sequence,
            },
        });
        last = Some(screen);
        if send_json(&writer, &event).await.is_err() {
            return;
        }
    }
}

/// One read of a terminal for the phone.
#[derive(PartialEq)]
struct TerminalScreen {
    text: String,
    rows_above: u64,
    columns: u64,
    /// Styled grid (`terminal.render_grid` format of the macOS host), `null` if unavailable.
    render_grid: Value,
}

/// Ask the GTK main thread for the current visible terminal text, the number of
/// scrollback rows above it (0 on the alternate screen) and the grid width in columns,
/// which the phone uses to fit the whole width on its screen.
async fn read_terminal_text(
    cmd_tx: &mpsc::Sender<SocketCommand>,
    surface_id: &str,
) -> Result<TerminalScreen, (String, String)> {
    let result = gtk_call(cmd_tx, |resp_tx| SocketCommand::SurfaceReadText {
        req_id: Value::Null,
        scrollback: false,
        render_grid: true,
        id: Some(surface_id.to_owned()),
        workspace: None,
        resp_tx,
    })
    .await?;
    let text = result
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let rows_above = result.get("rows_above").and_then(Value::as_u64).unwrap_or(0);
    let columns = result.get("columns").and_then(Value::as_u64).unwrap_or(0);
    let render_grid = result.get("render_grid").cloned().unwrap_or(Value::Null);
    Ok(TerminalScreen { text, rows_above, columns, render_grid })
}

/// Count workspaces through the existing GTK command; unavailable state reports zero.
async fn workspace_count(cmd_tx: &mpsc::Sender<SocketCommand>) -> usize {
    gtk_call(cmd_tx, |resp_tx| SocketCommand::WorkspaceList {
        req_id: Value::Null,
        resp_tx,
    })
    .await
    .ok()
    .and_then(|result| result.get("workspaces").and_then(Value::as_array).map(Vec::len))
    .unwrap_or(0)
}

/// Send one command to the GTK owner and unwrap the existing socket response envelope.
async fn gtk_call<F>(
    cmd_tx: &mpsc::Sender<SocketCommand>,
    make: F,
) -> Result<Value, (String, String)>
where
    F: FnOnce(commands::RespTx) -> SocketCommand,
{
    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
    if cmd_tx.send(make(resp_tx)).await.is_err() {
        return Err((
            "unavailable".to_owned(),
            "GTK command channel is closed".to_owned(),
        ));
    }
    let response = resp_rx.await.map_err(|_| {
        (
            "internal_error".to_owned(),
            "GTK handler dropped its response".to_owned(),
        )
    })?;
    if response.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(response.get("result").cloned().unwrap_or(Value::Null));
    }
    let code = response
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .unwrap_or("internal_error")
        .to_owned();
    let message = response
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("mobile command failed")
        .to_owned();
    Err((code, message))
}

/// Read one length-prefixed frame, rejecting an oversized header before allocating its payload.
async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut header = [0u8; 4];
    if reader.read(&mut header[..1]).await? == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut header[1..]).await?;
    let size = u32::from_be_bytes(header) as usize;
    if size > mobile::MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame exceeds byte limit",
        ));
    }
    let mut frame = vec![0u8; size];
    reader.read_exact(&mut frame).await?;
    Ok(Some(frame))
}

/// Write one length-prefixed frame.
async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &[u8]) -> io::Result<()> {
    if frame.len() > mobile::MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame exceeds byte limit",
        ));
    }
    let size = u32::try_from(frame.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame is too large"))?;
    writer.write_all(&size.to_be_bytes()).await?;
    writer.write_all(frame).await
}

/// Encode and write one JSON frame on a writer shared with the event task.
async fn send_json(writer: &tokio::sync::Mutex<OwnedWriteHalf>, value: &Value) -> io::Result<()> {
    write_json(&mut *writer.lock().await, value).await
}

/// Encode and write one JSON frame.
async fn write_json<W: AsyncWrite + Unpin>(writer: &mut W, value: &Value) -> io::Result<()> {
    write_frame(writer, value.to_string().as_bytes()).await
}

fn success_response(id: Value, result: Value) -> Value {
    json!({"id": id, "ok": true, "result": result})
}

fn error_response(id: Value, code: &str, message: &str) -> Value {
    json!({"id": id, "ok": false, "error": {"code": code, "message": message}})
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every emitted token is one literal character or a name `named_key::parse` accepts.
    #[test]
    fn input_keys_emits_named_key_payloads() {
        let samples = [
            "ls\r",
            "\x03",
            "\x1b[A\x1b[D",
            "\x1bOB",
            "\x1b[5~\x1b[6~",
            "\x1b",
            "\x1bb",
            "a\r\nb",
            "\x7f",
            "a b",
            "\x1b[Z",
            "\x1b[9x",
            "\x1b[H\x1bOF",
            "\x01\x1a",
            "\x1b1",
        ];
        for sample in samples {
            for key in crate::mobile::input_keys(sample) {
                assert!(
                    key.chars().count() == 1 || crate::ghostty::named_key::parse(&key).is_some(),
                    "{key:?} from {sample:?} is neither a character nor a key name",
                );
            }
        }
    }

    /// Two complete frames coalesced into one read are returned separately and in order.
    #[tokio::test]
    async fn frames_split_at_their_lengths() {
        let (mut client, server) = tokio::io::duplex(1024);
        let first = b"{\"id\":1}";
        let second = b"{\"id\":2}";
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&u32::try_from(first.len()).unwrap().to_be_bytes());
        bytes.extend_from_slice(first);
        bytes.extend_from_slice(&u32::try_from(second.len()).unwrap().to_be_bytes());
        bytes.extend_from_slice(second);
        client.write_all(&bytes).await.unwrap();
        let mut reader = BufReader::new(server);
        assert_eq!(read_frame(&mut reader).await.unwrap().unwrap(), first);
        assert_eq!(read_frame(&mut reader).await.unwrap().unwrap(), second);
    }

    /// An oversized declared length is refused before its payload is read or allocated.
    #[tokio::test]
    async fn oversized_frame_is_rejected() {
        let (mut client, server) = tokio::io::duplex(16);
        let size = u32::try_from(mobile::MAX_FRAME_BYTES + 1).unwrap();
        client.write_all(&size.to_be_bytes()).await.unwrap();
        let mut reader = BufReader::new(server);
        let error = read_frame(&mut reader).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    /// A clean close before any header is a normal end of connection.
    #[tokio::test]
    async fn clean_eof_is_not_an_error() {
        let (client, server) = tokio::io::duplex(16);
        drop(client);
        let mut reader = BufReader::new(server);
        assert!(read_frame(&mut reader).await.unwrap().is_none());
    }

    /// Frames round-trip through the writer with the advertised big-endian prefix.
    #[tokio::test]
    async fn written_frame_has_big_endian_length() {
        let (mut writer, mut reader) = tokio::io::duplex(64);
        write_frame(&mut writer, b"ok").await.unwrap();
        let mut header = [0u8; 4];
        reader.read_exact(&mut header).await.unwrap();
        assert_eq!(u32::from_be_bytes(header), 2);
        let mut payload = [0u8; 2];
        reader.read_exact(&mut payload).await.unwrap();
        assert_eq!(&payload, b"ok");
    }

    /// The allowed bind set is loopback plus Tailscale only.
    #[test]
    fn addresses_are_filtered() {
        assert_eq!(
            mobile::listen_addresses()[0],
            std::net::Ipv4Addr::LOCALHOST
        );
    }
}
