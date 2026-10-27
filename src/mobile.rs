//! Shared mobile-host contract: pairing token, ticket, host status and Android package files.
//!
//! The desktop listener and the `cmux mobile-ticket` CLI both use these helpers, so the
//! token path, Tailscale address filter and app metadata stay identical on both sides.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::net::Ipv4Addr;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Fixed prototype port shared with the Android client.
pub const PORT: u16 = 7443;
/// Largest accepted request or response JSON frame.
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
/// Largest single Android package chunk.
pub const MAX_CHUNK_BYTES: u64 = 1_048_576;
/// Largest pasted image, as the Mac's clipboard-image cap.
pub const MAX_PASTED_IMAGE_BYTES: usize = 10 * 1024 * 1024;
/// Token file name below the cmux configuration directory.
const TOKEN_FILE: &str = "mobile-token";

/// Pairing token path: `$XDG_CONFIG_HOME/cmux/mobile-token`.
pub fn token_path() -> PathBuf {
    cmux_platform::paths::config_dir().join(TOKEN_FILE)
}

/// Read the existing token, or create one with 32 random bytes encoded as 64 hex characters.
///
/// The file is created owner-only. An existing file must be a regular owner-owned file with
/// exactly 64 hex characters; otherwise this fails closed rather than accepting a weak token.
pub fn load_or_create_token() -> io::Result<String> {
    let path = token_path();
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("mobile token has no parent directory"))?;
    cmux_platform::filesystem::create_private_directory(parent)?;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
    {
        Ok(mut file) => {
            let mut secret = [0u8; 32];
            std::fs::File::open("/dev/urandom")?.read_exact(&mut secret)?;
            let token = hex(&secret);
            file.write_all(token.as_bytes())?;
            file.sync_all()?;
            Ok(token)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = std::fs::metadata(&path)?;
            if !metadata.is_file()
                || metadata.len() != 64
                || metadata.uid() != cmux_platform::filesystem::current_uid()
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "invalid mobile token file",
                ));
            }
            if metadata.permissions().mode() & 0o077 != 0 {
                cmux_platform::filesystem::restrict_file_to_owner(&path)?;
            }
            let token = std::fs::read_to_string(&path)?;
            if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid mobile token file",
                ));
            }
            Ok(token)
        }
        Err(error) => Err(error),
    }
}

/// The pairing ticket printed by `cmux mobile-ticket`.
///
/// Returns an error when no Tailscale IPv4 address exists, because a ticket without a
/// reachable host cannot be pasted into the Android app.
pub fn ticket() -> Result<Value, String> {
    let host = tailscale_addresses()
        .map_err(|error| format!("could not inspect network addresses: {error}"))?
        .into_iter()
        .next()
        .ok_or_else(|| "no Tailscale IPv4 address found".to_owned())?;
    let attach_token = load_or_create_token()
        .map_err(|error| format!("could not load mobile token: {error}"))?;
    Ok(json!({
        "v": 1,
        "host": host.to_string(),
        "port": PORT,
        "attach_token": attach_token,
    }))
}

/// Pairing link for the Android app's `cmuxandroid://pair` intent filter, built from a ticket.
pub fn pairing_link(ticket: &Value) -> Result<String, String> {
    let host = ticket
        .get("host")
        .and_then(Value::as_str)
        .filter(|host| !host.is_empty())
        .ok_or_else(|| "ticket has no host".to_owned())?;
    let port = ticket
        .get("port")
        .and_then(Value::as_u64)
        .ok_or_else(|| "ticket has no port".to_owned())?;
    let token = ticket
        .get("attach_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| "ticket has no attach_token".to_owned())?;
    Ok(format!("cmuxandroid://pair?h={host}&p={port}&t={token}"))
}

/// QR code of a pairing link as Unicode half-block characters (no image rendering).
pub fn pairing_qr(link: &str) -> Result<String, String> {
    qrcode::QrCode::new(link.as_bytes())
        .map_err(|error| format!("could not encode pairing QR code: {error}"))
        .map(|code| {
            code.render::<qrcode::render::unicode::Dense1x2>()
                .quiet_zone(true)
                .build()
        })
}

/// Public host status, shared by `mobile.host.status` and workspace-list entries.
pub fn host_status(workspace_count: usize) -> Value {
    json!({
        "mac_device_id": device_id(),
        "mac_display_name": display_name(),
        "workspace_count": workspace_count,
        "terminal_fidelity": "text",
        "capabilities": ["terminal.text", "app.update", "terminal.input.keys", "terminal.paste", "terminal.paste_image", "feed.list", "feed.text", "feed.permission.reply", "feed.question.reply", "feed.exit_plan.reply", "workspace.actions.v1", "workspace.read_state.v1", "workspace.close.v1", "terminal.create", "workspace.create"],
    })
}

/// Split typed text into the individual key presses `surface.send_key` accepts.
///
/// Control bytes and escape sequences become key names (`0x03` → `ctrl-c`, `ESC[A` → `up`);
/// every other character stays a one-character string. `mobile.terminal.input` types text
/// this way so control bytes arrive as real key presses instead of bracketed paste.
pub fn input_keys(text: &str) -> Vec<String> {
    let characters: Vec<char> = text.chars().collect();
    let mut keys = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        index += 1;
        match character {
            '\u{1b}' => keys.extend(escape_keys(&characters, &mut index)),
            '\r' => {
                // A CRLF pair is one Enter, not two.
                if characters.get(index) == Some(&'\n') {
                    index += 1;
                }
                keys.push("enter".to_owned());
            }
            '\n' => keys.push("enter".to_owned()),
            '\t' => keys.push("tab".to_owned()),
            '\u{8}' | '\u{7f}' => keys.push("backspace".to_owned()),
            control if ('\u{1}'..='\u{1a}').contains(&control) => {
                keys.push(format!("ctrl-{}", (control as u8 - 1 + b'a') as char));
            }
            '\0' | '\u{1c}'..='\u{1f}' => {}
            other => keys.push(other.to_string()),
        }
    }
    keys
}

/// Keys produced by one `ESC`, advancing `index` over the characters it consumed.
///
/// An unknown or incomplete sequence yields `escape` alone, leaving the following
/// characters to be read again as ordinary input.
fn escape_keys(characters: &[char], index: &mut usize) -> Vec<String> {
    let Some(&next) = characters.get(*index) else {
        return vec!["escape".to_owned()];
    };
    let sequence = match next {
        '[' => csi_sequence(characters, *index + 1),
        'O' => ss3_sequence(characters, *index + 1),
        _ => None,
    };
    if let Some((key, consumed)) = sequence {
        *index += consumed;
        return vec![key];
    }
    if next == '[' {
        // A complete CSI sequence with no key name (mouse report `ESC[<64;10;5M`, …)
        // is passed through whole, like the Mac host writing the phone's bytes to the
        // PTY: `send_key` writes it as one typed input instead of literal text.
        if let Some(length) = csi_length(characters, *index + 1) {
            let raw: String = std::iter::once('\u{1b}')
                .chain(characters[*index..*index + length].iter().copied())
                .collect();
            *index += length;
            return vec![raw];
        }
        return vec!["escape".to_owned()];
    }
    if next == 'O' {
        return vec!["escape".to_owned()];
    }
    if next.is_ascii_alphabetic() {
        *index += 1;
        return vec![format!("alt-{}", next.to_ascii_lowercase())];
    }
    if !next.is_control() {
        *index += 1;
        return vec!["escape".to_owned(), next.to_string()];
    }
    vec!["escape".to_owned()]
}

/// Length from the `[` of a complete CSI sequence (parameters 0x30–0x3F,
/// intermediates 0x20–0x2F, final 0x40–0x7E), or `None` when it is cut short.
fn csi_length(characters: &[char], start: usize) -> Option<usize> {
    let mut position = start;
    while let Some(&character) = characters.get(position) {
        position += 1;
        match character {
            '\u{30}'..='\u{3f}' | '\u{20}'..='\u{2f}' => {}
            '\u{40}'..='\u{7e}' => return Some(position - start + 1),
            _ => return None,
        }
    }
    None
}

/// Key name of a CSI sequence (`ESC [ …`) and the characters it spans from the `[`.
fn csi_sequence(characters: &[char], start: usize) -> Option<(String, usize)> {
    match *characters.get(start)? {
        'A' => Some(("up".to_owned(), 2)),
        'B' => Some(("down".to_owned(), 2)),
        'C' => Some(("right".to_owned(), 2)),
        'D' => Some(("left".to_owned(), 2)),
        'H' => Some(("home".to_owned(), 2)),
        'F' => Some(("end".to_owned(), 2)),
        'Z' => Some(("shift-tab".to_owned(), 2)),
        '2'..='6' if characters.get(start + 1) == Some(&'~') => {
            let key = match characters[start] {
                '2' => "insert",
                '3' => "delete",
                '5' => "pageup",
                '6' => "pagedown",
                _ => return None,
            };
            Some((key.to_owned(), 3))
        }
        _ => None,
    }
}

/// Key name of an SS3 sequence (`ESC O …`) and the characters it spans from the `O`.
fn ss3_sequence(characters: &[char], start: usize) -> Option<(String, usize)> {
    let key = match *characters.get(start)? {
        'A' => "up",
        'B' => "down",
        'C' => "right",
        'D' => "left",
        'H' => "home",
        'F' => "end",
        _ => return None,
    };
    Some((key.to_owned(), 2))
}

/// Whether a local IPv4 address may be bound: loopback or Tailscale's CGNAT range.
pub fn is_allowed_address(address: Ipv4Addr) -> bool {
    let [first, second, ..] = address.octets();
    address == Ipv4Addr::LOCALHOST || (first == 100 && (64..=127).contains(&second))
}

/// Addresses the mobile listener may bind, loopback first.
pub fn listen_addresses() -> Vec<Ipv4Addr> {
    let mut addresses = vec![Ipv4Addr::LOCALHOST];
    if let Ok(local) = cmux_platform::network::ipv4_addresses() {
        addresses.extend(
            local
                .into_iter()
                .filter(|address| *address != Ipv4Addr::LOCALHOST && is_allowed_address(*address)),
        );
    }
    addresses.sort();
    addresses.dedup();
    if let Some(index) = addresses.iter().position(Ipv4Addr::is_loopback) {
        addresses.swap(0, index);
    }
    addresses
}

/// Every local Tailscale IPv4 address, sorted.
pub fn tailscale_addresses() -> io::Result<Vec<Ipv4Addr>> {
    let mut addresses: Vec<_> = cmux_platform::network::ipv4_addresses()?
        .into_iter()
        .filter(|address| !address.is_loopback() && is_allowed_address(*address))
        .collect();
    addresses.sort();
    addresses.dedup();
    Ok(addresses)
}

/// Check the request's attach token, except for the unauthenticated status method.
///
/// The comparison is byte-wise and does not exit early on the first mismatch.
pub fn authorized(request: &Value, expected: &str) -> bool {
    if request.get("method").and_then(Value::as_str) == Some("mobile.host.status") {
        return true;
    }
    request
        .get("auth")
        .and_then(|auth| auth.get("attach_token"))
        .and_then(Value::as_str)
        .is_some_and(|token| constant_time_eq(token.as_bytes(), expected.as_bytes()))
}

/// Compare two byte strings without short-circuiting on the first differing byte.
pub fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for (index, byte) in left.iter().enumerate() {
        difference |= usize::from(*byte ^ right.get(index).copied().unwrap_or(0));
    }
    difference == 0
}

/// Android package directory: `$XDG_DATA_HOME/cmux/mobile`.
pub fn app_directory() -> PathBuf {
    cmux_platform::paths::data_dir().join("mobile")
}

/// Read the latest Android package descriptor, or `None` when either file is absent/invalid.
pub fn app_latest() -> Option<Value> {
    app_latest_in(&app_directory())
}

/// Read a package descriptor from an explicit directory (used by tests and the listener).
pub fn app_latest_in(directory: &Path) -> Option<Value> {
    let metadata = std::fs::read_to_string(directory.join("cmux-android.json")).ok()?;
    let descriptor: Value = serde_json::from_str(&metadata).ok()?;
    let version_code = descriptor.get("version_code")?.as_i64()?;
    let version_name = descriptor.get("version_name")?.as_str()?;
    let (size, sha256) = package_fingerprint(&directory.join("cmux-android.apk")).ok()?;
    Some(json!({
        "version_code": version_code,
        "version_name": version_name,
        "size": size,
        "sha256": sha256,
    }))
}

/// Read one bounded package chunk; a short read at EOF is returned as-is.
pub fn app_chunk(offset: u64, length: u64) -> Result<Value, (String, String)> {
    app_chunk_in(&app_directory(), offset, length)
}

/// Read one chunk from an explicit directory (used by tests and the listener).
pub fn app_chunk_in(directory: &Path, offset: u64, length: u64) -> Result<Value, (String, String)> {
    if length > MAX_CHUNK_BYTES {
        return Err((
            "invalid_params".to_owned(),
            format!("length exceeds {MAX_CHUNK_BYTES} bytes"),
        ));
    }
    let path = directory.join("cmux-android.apk");
    let mut file = std::fs::File::open(&path)
        .map_err(|error| ("not_found".to_owned(), format!("package unavailable: {error}")))?;
    let size = file
        .metadata()
        .map_err(|error| ("internal_error".to_owned(), error.to_string()))?
        .len();
    if offset > size {
        return Err((
            "invalid_params".to_owned(),
            "offset is beyond the package size".to_owned(),
        ));
    }
    let available = (size - offset).min(length);
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| ("internal_error".to_owned(), error.to_string()))?;
    let mut data = Vec::with_capacity(usize::try_from(available).unwrap_or(0));
    file.take(available)
        .read_to_end(&mut data)
        .map_err(|error| ("internal_error".to_owned(), error.to_string()))?;
    use base64::Engine as _;
    Ok(json!({
        "offset": offset,
        "data_b64": base64::engine::general_purpose::STANDARD.encode(data),
    }))
}

/// Stable host identity: the machine id when available, else the host name.
fn device_id() -> String {
    std::fs::read_to_string("/etc/machine-id")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(display_name)
}

/// Local host name used as the display name; never empty.
fn display_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|value| !value.is_empty()))
        .unwrap_or_else(|| "cmux".to_owned())
}

/// Streaming size and SHA-256 of a package file.
fn package_fingerprint(path: &Path) -> io::Result<(u64, String)> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        size += u64::try_from(read).unwrap_or(0);
        hasher.update(&buffer[..read]);
    }
    Ok((size, hex(&hasher.finalize())))
}

/// Write image bytes pasted on the phone to a new temporary file and return its path.
///
/// Mirrors the Mac's `saveImageData`: `clipboard-<date>-<id>.<ext>` in the temporary
/// directory, extension limited to known image types (default `png`), 10 MB cap. The
/// caller types the shell-escaped path into the terminal, as for a local image paste.
pub fn save_pasted_image(directory: &Path, data: &[u8], format: &str) -> io::Result<PathBuf> {
    if data.is_empty() || data.len() > MAX_PASTED_IMAGE_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "image empty or too large"));
    }
    let token = format.trim().to_ascii_lowercase();
    let extension = match token.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "heif" | "tiff" | "bmp" => token.as_str(),
        _ => "png",
    };
    let timestamp = glib::DateTime::now_local()
        .and_then(|now| now.format("%Y-%m-%d-%H%M%S"))
        .map(|text| text.to_string())
        .unwrap_or_default();
    let id = uuid::Uuid::new_v4().simple().to_string();
    let path = directory.join(format!("clipboard-{timestamp}-{}.{extension}", &id[..8]));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    file.write_all(data)?;
    Ok(path)
}

/// Lowercase hexadecimal encoding.
fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

/// `workspace.action` from the phone, as upstream iOS sends it
/// (`MobileShellComposite+WorkspaceActions.swift`): the row swipes and menu only, never
/// `close_others` and the like, and always an explicit workspace, never the active one.
pub struct WorkspaceActionRequest {
    pub action: String,
    pub workspace: String,
    pub title: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
}

const PHONE_WORKSPACE_ACTIONS: &[&str] = &[
    "pin",
    "unpin",
    "rename",
    "mark_read",
    "mark_unread",
    "set_color",
    "clear_color",
    "set_description",
    "clear_description",
];

pub fn workspace_action_request(params: &Value) -> Result<WorkspaceActionRequest, (String, String)> {
    let text = |key: &str| params.get(key).and_then(Value::as_str).map(str::to_owned);
    let invalid = |message: &str| ("invalid_params".to_owned(), message.to_owned());
    let action = text("action").ok_or_else(|| invalid("action must be a string"))?;
    if !PHONE_WORKSPACE_ACTIONS.contains(&action.as_str()) {
        return Err(invalid(&format!("unsupported workspace action from the phone: {action}")));
    }
    let workspace = text("workspace_id")
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| invalid("workspace_id is required"))?;
    Ok(WorkspaceActionRequest {
        action,
        workspace,
        title: text("title"),
        color: text("color"),
        description: text("description"),
    })
}

/// The workspace a phone `terminal.create` targets: always explicit, never the active one.
pub fn terminal_create_workspace(params: &Value) -> Result<String, (String, String)> {
    params
        .get("workspace_id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ("invalid_params".to_owned(), "workspace_id is required".to_owned()))
}

/// Optional title of a phone `workspace.create` (upstream iOS sends none, or a spec title).
pub fn workspace_create_title(params: &Value) -> Option<String> {
    params
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_owned)
}

/// Upstream's create replies: the workspace list plus the new id under `key`
/// (`created_terminal_id`, `created_workspace_id`).
pub fn with_created(mut list: Value, key: &str, created: Option<&Value>) -> Value {
    if let (Some(object), Some(created)) = (list.as_object_mut(), created) {
        object.insert(key.to_owned(), created.clone());
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pairing link carries every ticket field and its QR code is Unicode half blocks.
    #[test]
    fn pairing_link_and_qr() {
        let token = "ab".repeat(32);
        let ticket = json!({"v": 1, "host": "100.64.0.7", "port": 7443, "attach_token": token});
        let link = pairing_link(&ticket).unwrap();
        assert_eq!(link, format!("cmuxandroid://pair?h=100.64.0.7&p=7443&t={token}"));
        let qr = pairing_qr(&link).unwrap();
        assert!(qr.lines().count() > 20, "QR code is too short");
        assert!(qr.contains('█'), "QR code has no dark modules");
        assert!(pairing_link(&json!({"host": "100.64.0.7"})).is_err());
        assert!(pairing_link(&json!({"host": "", "port": 1, "attach_token": "t"})).is_err());
    }

    /// The status payload is available without authentication and has the fixed contract keys.
    #[test]
    fn status_is_public() {
        let request = json!({"method": "mobile.host.status"});
        assert!(authorized(&request, "secret"));
        let status = host_status(3);
        assert_eq!(status["workspace_count"], 3);
        assert_eq!(status["terminal_fidelity"], "text");
        assert_eq!(
            status["capabilities"],
            json!(["terminal.text", "app.update", "terminal.input.keys", "terminal.paste", "terminal.paste_image", "feed.list", "feed.text", "feed.permission.reply", "feed.question.reply", "feed.exit_plan.reply", "workspace.actions.v1", "workspace.read_state.v1", "workspace.close.v1", "terminal.create", "workspace.create"])
        );
        assert!(status["mac_device_id"].is_string());
        assert!(status["mac_display_name"].is_string());
    }

    /// A phone terminal.create needs an explicit workspace and answers with the new terminal id.
    #[test]
    fn terminal_create_targets_an_explicit_workspace() {
        assert_eq!(terminal_create_workspace(&json!({"workspace_id": "w-1"})).unwrap(), "w-1");
        assert!(terminal_create_workspace(&json!({})).is_err());
        assert!(terminal_create_workspace(&json!({"workspace_id": " "})).is_err());
        let reply = with_created(json!({"workspaces": []}), "created_terminal_id", Some(&json!("s-9")));
        assert_eq!(reply, json!({"workspaces": [], "created_terminal_id": "s-9"}));
        assert_eq!(with_created(json!({}), "created_workspace_id", None), json!({}));
        assert_eq!(workspace_create_title(&json!({"title": "  Essai "})).as_deref(), Some("Essai"));
        assert_eq!(workspace_create_title(&json!({"title": " "})), None);
    }

    /// A pasted image lands in a new private file with a safe extension.
    #[test]
    fn pasted_image_is_saved_with_a_safe_extension() {
        let directory = std::env::temp_dir().join(format!("cmux-paste-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = save_pasted_image(&directory, b"\x89PNG", "JPG").unwrap();
        assert_eq!(path.parent(), Some(directory.as_path()));
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("clipboard-") && name.ends_with(".jpg"), "{name}");
        assert_eq!(std::fs::read(&path).unwrap(), b"\x89PNG");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let odd = save_pasted_image(&directory, b"x", "../sh").unwrap();
        assert_eq!(odd.extension().unwrap(), "png");
        assert!(save_pasted_image(&directory, b"", "png").is_err());
        assert!(save_pasted_image(&directory, &vec![0; MAX_PASTED_IMAGE_BYTES + 1], "png").is_err());
        std::fs::remove_dir_all(&directory).unwrap();
    }

    /// Typed text is split into key presses: controls, escape sequences and literals.
    #[test]
    fn input_keys_splits_typed_text() {
        assert_eq!(input_keys("ls\r"), ["l", "s", "enter"]);
        assert_eq!(input_keys("\x03"), ["ctrl-c"]);
        assert_eq!(input_keys("\x1b[A\x1b[D"), ["up", "left"]);
        assert_eq!(input_keys("\x1bOB"), ["down"]);
        assert_eq!(input_keys("\x1b[5~\x1b[6~"), ["pageup", "pagedown"]);
        assert_eq!(input_keys("\x1b"), ["escape"]);
        assert_eq!(input_keys("\x1bb"), ["alt-b"]);
        assert_eq!(input_keys("a\r\nb"), ["a", "enter", "b"]);
        // Mouse reports and other unnamed CSI sequences stay whole.
        assert_eq!(input_keys("\x1b[<64;98;25M"), ["\x1b[<64;98;25M"]);
        assert_eq!(input_keys("\x1b[<0;3;4m\x1b[A"), ["\x1b[<0;3;4m", "up"]);
        assert_eq!(input_keys("\x1b[<64;9"), ["escape", "[", "<", "6", "4", ";", "9"]);
        assert_eq!(input_keys("\x7f"), ["backspace"]);
        assert_eq!(input_keys("a b"), ["a", " ", "b"]);
        assert_eq!(input_keys("\x1b[Z"), ["shift-tab"]);
        assert_eq!(input_keys("\x1b[9x"), ["\x1b[9x"]);
    }

    /// The remaining control bytes and escape sequences, including what is dropped.
    #[test]
    fn input_keys_covers_controls_and_sequences() {
        assert_eq!(input_keys("\x01\x1a"), ["ctrl-a", "ctrl-z"]);
        assert_eq!(input_keys("\x0b\x0c"), ["ctrl-k", "ctrl-l"]);
        assert_eq!(input_keys("\t\x08"), ["tab", "backspace"]);
        assert_eq!(input_keys("\x1b[H\x1b[F"), ["home", "end"]);
        assert_eq!(input_keys("\x1bOH\x1bOF"), ["home", "end"]);
        assert_eq!(input_keys("\x1b[3~\x1b[2~"), ["delete", "insert"]);
        assert_eq!(input_keys("\x1bB"), ["alt-b"]);
        assert_eq!(input_keys("\x1b1"), ["escape", "1"]);
        assert!(input_keys("\0\x1c\x1d\x1e\x1f").is_empty());
        assert_eq!(input_keys("É+-"), ["É", "+", "-"]);
    }

    /// Every other method needs the exact token; missing, short and wrong tokens are refused.
    #[test]
    fn token_is_required_and_compared() {
        let method = json!({"method": "mobile.workspace.list"});
        assert!(!authorized(&method, "secret"));
        let wrong = json!({"method": "mobile.workspace.list", "auth": {"attach_token": "secreu"}});
        assert!(!authorized(&wrong, "secret"));
        let short = json!({"method": "mobile.workspace.list", "auth": {"attach_token": "secretx"}});
        assert!(!authorized(&short, "secret"));
        let right = json!({"method": "mobile.workspace.list", "auth": {"attach_token": "secret"}});
        assert!(authorized(&right, "secret"));
        assert!(!constant_time_eq(b"a", b"b"));
        assert!(constant_time_eq(b"same", b"same"));
    }

    /// Only loopback and 100.64.0.0/10 are bindable.
    #[test]
    fn address_filter_is_narrow() {
        assert!(is_allowed_address(Ipv4Addr::new(127, 0, 0, 1)));
        assert!(is_allowed_address(Ipv4Addr::new(100, 64, 0, 1)));
        assert!(is_allowed_address(Ipv4Addr::new(100, 127, 255, 255)));
        assert!(!is_allowed_address(Ipv4Addr::new(100, 63, 255, 255)));
        assert!(!is_allowed_address(Ipv4Addr::new(100, 128, 0, 0)));
        assert!(!is_allowed_address(Ipv4Addr::new(0, 0, 0, 0)));
        assert!(!is_allowed_address(Ipv4Addr::new(192, 168, 1, 2)));
    }

    /// A missing descriptor is null, and a complete one reports the computed size and digest.
    #[test]
    fn latest_package_is_optional_and_fingerprinted() {
        let directory = temporary_directory("latest");
        assert!(app_latest_in(&directory).is_none());
        std::fs::write(directory.join("cmux-android.json"), b"{\"version_code\":7,\"version_name\":\"1.2\"}")
            .unwrap();
        assert!(app_latest_in(&directory).is_none());
        std::fs::write(directory.join("cmux-android.apk"), b"package").unwrap();
        let latest = app_latest_in(&directory).unwrap();
        assert_eq!(latest["version_code"], 7);
        assert_eq!(latest["version_name"], "1.2");
        assert_eq!(latest["size"], 7);
        let digest = latest["sha256"].as_str().unwrap();
        assert_eq!(digest.len(), 64);
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// Chunk requests enforce the byte cap and reject offsets past the package.
    #[test]
    fn chunk_bounds() {
        let directory = temporary_directory("chunk");
        std::fs::write(directory.join("cmux-android.apk"), b"0123456789").unwrap();
        use base64::Engine as _;
        let chunk = app_chunk_in(&directory, 4, 3).unwrap();
        assert_eq!(chunk["offset"], 4);
        assert_eq!(
            chunk["data_b64"],
            base64::engine::general_purpose::STANDARD.encode(b"456")
        );
        let end = app_chunk_in(&directory, 9, 100).unwrap();
        assert_eq!(
            end["data_b64"],
            base64::engine::general_purpose::STANDARD.encode(b"9")
        );
        assert_eq!(
            app_chunk_in(&directory, 0, MAX_CHUNK_BYTES + 1)
                .unwrap_err()
                .0,
            "invalid_params"
        );
        assert_eq!(
            app_chunk_in(&directory, 11, 1).unwrap_err().0,
            "invalid_params"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// Create a per-test directory without touching the user's XDG locations.
    fn temporary_directory(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "cmux-mobile-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn phone_workspace_action_keeps_row_actions_and_needs_a_workspace() {
        let request = workspace_action_request(&json!({
            "action": "rename", "workspace_id": "w1", "title": "Nouveau", "client_id": "c"
        }))
        .unwrap();
        assert_eq!(request.action, "rename");
        assert_eq!(request.workspace, "w1");
        assert_eq!(request.title.as_deref(), Some("Nouveau"));
        assert!(workspace_action_request(&json!({"action": "mark_unread", "workspace_id": "w1"})).is_ok());
        // Never the active workspace by omission, never bulk closes.
        assert!(workspace_action_request(&json!({"action": "pin"})).is_err());
        assert!(workspace_action_request(&json!({"action": "close_others", "workspace_id": "w1"})).is_err());
    }
}
