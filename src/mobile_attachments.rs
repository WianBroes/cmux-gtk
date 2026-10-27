//! Files sent from the phone (`mobile.task.attachment.upload`), port of upstream's
//! `MobileTaskAttachmentStore`: chunks are appended to a staging file under
//! `~/.cache/cmux/task-attachments/<operation>/` and, on the last one, moved to a safe,
//! unique name whose absolute path is returned. Re-sending a completed upload returns the
//! same path. Not ported: the per-operation caps (10 files, 64 MiB) and the cleanup sweep.

use base64::Engine as _;
use serde_json::{json, Value};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Upstream `maximumFileBytes` and `maximumChunkBytes`.
pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_CHUNK_BYTES: usize = 3 * 1024 * 1024;

type Failure = (String, String);

fn invalid(message: &str) -> Failure {
    ("invalid_params".to_owned(), message.to_owned())
}

fn io(error: std::io::Error) -> Failure {
    ("internal_error".to_owned(), format!("Could not store the attachment: {error}"))
}

/// Upstream's cache root (`~/.cache/cmux/task-attachments`).
pub fn default_root() -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
    cache.join("cmux").join("task-attachments")
}

/// Upstream `sanitizedFileName`: separators and control characters become one `_`,
/// leading dots go (no hidden files), empty → `attachment`, at most 180 characters.
pub fn sanitized_file_name(raw: &str) -> String {
    let mut out = String::new();
    let mut replaced = false;
    for c in raw.chars() {
        if c == '/' || c == '\\' || c.is_control() {
            if !replaced {
                out.push('_');
                replaced = true;
            }
        } else {
            out.push(c);
            replaced = false;
        }
    }
    let name = out.trim().trim_start_matches('.').trim();
    let name = if name.is_empty() { "attachment" } else { name };
    name.chars().take(180).collect()
}

fn private_dir(path: &Path) -> Result<(), Failure> {
    std::fs::create_dir_all(path).map_err(io)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(io)
}

/// `name.ext`, then `name-2.ext`, `name-3.ext`… until the name is free.
fn unique_path(directory: &Path, name: &str) -> PathBuf {
    let candidate = directory.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    };
    (2..)
        .map(|n| directory.join(format!("{stem}-{n}{ext}")))
        .find(|path| !path.exists())
        .expect("an unused name exists")
}

/// One chunk; answers `{received_bytes}` and, after the last chunk, `{path}`.
pub fn upload(root: &Path, params: &Value) -> Result<Value, Failure> {
    let uuid = |key: &str| {
        params
            .get(key)
            .and_then(Value::as_str)
            .and_then(|text| uuid::Uuid::parse_str(text).ok())
            .map(|id| id.to_string())
            .ok_or_else(|| invalid(&format!("{key} must be a UUID")))
    };
    let operation = uuid("operation_id")?;
    let upload = uuid("upload_id")?;
    let file_name = params
        .get("file_name")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("file_name must be a string"))?;
    let total = params
        .get("total_bytes")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid("total_bytes must be a number"))?;
    let offset = params
        .get("offset")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid("offset must be a number"))?;
    let last = params
        .get("last")
        .and_then(Value::as_bool)
        .ok_or_else(|| invalid("last must be a boolean"))?;
    let chunk = params
        .get("data_b64")
        .and_then(Value::as_str)
        .and_then(|text| base64::engine::general_purpose::STANDARD.decode(text).ok())
        .ok_or_else(|| invalid("data_b64 must be base64"))?;
    if total > MAX_FILE_BYTES {
        return Err(invalid("Attachment is larger than 32 MB"));
    }
    if chunk.len() > MAX_CHUNK_BYTES {
        return Err(invalid("Attachment chunk is larger than 3 MB"));
    }

    let directory = root.join(&operation);
    let record = directory.join(".completed").join(&upload);
    if let Ok(path) = std::fs::read_to_string(&record) {
        if Path::new(&path).exists() {
            return Ok(json!({"received_bytes": total, "path": path}));
        }
    }
    let staging = directory.join(".staging");
    private_dir(&staging)?;
    private_dir(&directory.join(".completed"))?;
    let staged = staging.join(&upload);
    let current = match std::fs::metadata(&staged) {
        Ok(meta) => meta.len(),
        Err(_) if offset == 0 => 0,
        Err(_) => return Err(invalid("Attachment upload must start at offset 0")),
    };
    if offset == 0 && current > 0 {
        // A restarted upload replaces the partial file.
        std::fs::remove_file(&staged).map_err(io)?;
    } else if current != offset {
        return Err(invalid("Attachment chunk offset does not match the received bytes"));
    }
    if offset + chunk.len() as u64 > total {
        return Err(invalid("Attachment chunk exceeds total_bytes"));
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&staged)
        .map_err(io)?;
    file.write_all(&chunk).map_err(io)?;
    let received = offset + chunk.len() as u64;
    if !last {
        return Ok(json!({"received_bytes": received}));
    }
    if received != total {
        return Err(invalid("Final attachment byte count does not match total_bytes"));
    }
    let path = unique_path(&directory, &sanitized_file_name(file_name));
    std::fs::rename(&staged, &path).map_err(io)?;
    let path = path.to_string_lossy().into_owned();
    std::fs::write(&record, &path).map_err(io)?;
    Ok(json!({"received_bytes": received, "path": path}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(operation: &str, upload: &str, name: &str, total: u64, offset: u64, data: &[u8], last: bool) -> Value {
        json!({
            "operation_id": operation, "upload_id": upload, "file_name": name,
            "total_bytes": total, "offset": offset, "last": last,
            "data_b64": base64::engine::general_purpose::STANDARD.encode(data),
        })
    }

    /// Two chunks land in one private file under a safe name; a re-send gives the same path.
    #[test]
    fn chunks_are_assembled_once() {
        let root = std::env::temp_dir().join(format!("cmux-att-{}", uuid::Uuid::new_v4()));
        let op = uuid::Uuid::new_v4().to_string();
        let up = uuid::Uuid::new_v4().to_string();
        let first = upload(&root, &chunk(&op, &up, "../.rapport.pdf", 6, 0, b"abc", false)).unwrap();
        assert_eq!(first, json!({"received_bytes": 3}));
        assert!(upload(&root, &chunk(&op, &up, "x", 6, 2, b"def", true)).is_err(), "wrong offset accepted");
        let done = upload(&root, &chunk(&op, &up, "../.rapport.pdf", 6, 3, b"def", true)).unwrap();
        let path = done["path"].as_str().unwrap().to_owned();
        assert_eq!(Path::new(&path), root.join(&op).join("_.rapport.pdf"));
        assert_eq!(std::fs::read(&path).unwrap(), b"abcdef");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let again = upload(&root, &chunk(&op, &up, "../.rapport.pdf", 6, 0, b"abc", false)).unwrap();
        assert_eq!(again["path"], json!(path));
        // Same name, other upload: a second file, not an overwrite.
        let other = upload(&root, &chunk(&op, &uuid::Uuid::new_v4().to_string(), "../.rapport.pdf", 1, 0, b"z", true)).unwrap();
        assert!(other["path"].as_str().unwrap().ends_with("/_.rapport-2.pdf"));
        assert_eq!(std::fs::read(&path).unwrap(), b"abcdef");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Sizes, ids and the final count are checked.
    #[test]
    fn bad_uploads_are_refused() {
        let root = std::env::temp_dir().join(format!("cmux-att-{}", uuid::Uuid::new_v4()));
        let op = uuid::Uuid::new_v4().to_string();
        let up = uuid::Uuid::new_v4().to_string();
        assert!(upload(&root, &chunk("pas-un-uuid", &up, "a", 1, 0, b"a", true)).is_err());
        assert!(upload(&root, &chunk(&op, &up, "a", MAX_FILE_BYTES + 1, 0, b"a", false)).is_err());
        assert!(upload(&root, &chunk(&op, &up, "a", 1, 0, b"ab", true)).is_err());
        assert!(upload(&root, &chunk(&op, &up, "a", 5, 0, b"ab", true)).is_err());
        assert!(upload(&root, &chunk(&op, &uuid::Uuid::new_v4().to_string(), "a", 5, 3, b"ab", true)).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn file_names_are_sanitized() {
        assert_eq!(sanitized_file_name("a/b\\\nc"), "a_b_c");
        assert_eq!(sanitized_file_name("..."), "attachment");
        assert_eq!(sanitized_file_name("  .env "), "env");
        assert_eq!(sanitized_file_name(&"x".repeat(300)).len(), 180);
    }
}
