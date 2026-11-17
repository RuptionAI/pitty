use base64::{Engine, engine::general_purpose::STANDARD};
use pitty_core::Attachment;
use serde_json::{Value, json};
use std::{fs, io::Read, path::PathBuf};
pub const FILE_LIMIT: u64 = 16 * 1024 * 1024;
pub const CHAT_LIMIT: usize = 28 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub file_bytes: u64,
    pub text_bytes: u64,
    pub chat_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            file_bytes: FILE_LIMIT,
            text_bytes: 2 * MIB,
            chat_bytes: CHAT_LIMIT,
        }
    }
}
fn configured_mib(
    name: &str,
    value: Option<&str>,
    default: u64,
    maximum: u64,
) -> Result<u64, String> {
    let Some(value) = value else {
        return Ok(default * MIB);
    };
    let mib = value
        .parse::<u64>()
        .ok()
        .filter(|v| (1..=maximum).contains(v))
        .ok_or_else(|| format!("{name} must be a whole number from 1 to {maximum} MiB."))?;
    Ok(mib * MIB)
}
pub fn limits() -> Result<Limits, String> {
    let read = |name: &str, default, maximum| -> Result<u64, String> {
        let value = std::env::var(name).map(Some).or_else(|e| match e {
            std::env::VarError::NotPresent => Ok(None),
            _ => Err(format!("{name} contains invalid text.")),
        })?;
        configured_mib(name, value.as_deref(), default, maximum)
    };
    Ok(Limits {
        file_bytes: read("PITTY_MAX_FILE_MIB", 16, 64)?,
        text_bytes: read("PITTY_MAX_TEXT_MIB", 2, 16)?,
        chat_bytes: read("PITTY_MAX_CHAT_MIB", 28, 80)? as usize,
    })
}
pub fn chat_limit() -> Result<usize, String> {
    Ok(limits()?.chat_bytes)
}

pub fn path(text: &str) -> PathBuf {
    let text = text.trim();
    let literal = text.len() >= 2 && text.starts_with('\'') && text.ends_with('\'');
    let text = if text.len() >= 2
        && ((text.starts_with('\'') && text.ends_with('\''))
            || (text.starts_with('"') && text.ends_with('"')))
    {
        &text[1..text.len().saturating_sub(1)]
    } else {
        text
    };
    let mut unescaped = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' && !literal {
            if let Some(next) = chars.next() {
                unescaped.push(next);
            }
        } else {
            unescaped.push(c);
        }
    }
    if let Some(rest) = unescaped.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(unescaped)
}

/// Parse path tokens without invoking a shell, expanding variables, or globbing.
fn tokens(text: &str) -> Result<Vec<String>, String> {
    let mut result = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' && quote != Some('\'') {
            token.push(
                chars
                    .next()
                    .ok_or("A path ends with an incomplete escape.")?,
            );
        } else if quote == Some(c) {
            quote = None;
        } else if quote.is_none() && matches!(c, '\'' | '"') {
            quote = Some(c);
        } else if quote.is_none() && c.is_whitespace() {
            if !token.is_empty() {
                result.push(std::mem::take(&mut token));
            }
        } else {
            token.push(c);
        }
    }
    if quote.is_some() {
        return Err("A quoted path is missing its closing quote.".into());
    }
    if !token.is_empty() {
        result.push(token);
    }
    Ok(result)
}
fn expand_home(text: &str) -> PathBuf {
    if let Some(rest) = text.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        PathBuf::from(home).join(rest)
    } else {
        PathBuf::from(text)
    }
}
/// /attach accepts several quoted or escaped paths. An existing path with
/// unquoted spaces is also accepted as a single file for drag-and-drop parity.
pub fn explicit_paths(text: &str) -> Result<Vec<PathBuf>, String> {
    if text.trim().is_empty() {
        return Err("Use /attach PATH [PATH ...]. Quote paths containing spaces.".into());
    }
    let whole = path(text);
    if whole.exists() {
        return Ok(vec![whole]);
    }
    path_tokens(text)
}
/// Parse quoted or shell-escaped paths without probing the filesystem.
pub fn path_tokens(text: &str) -> Result<Vec<PathBuf>, String> {
    let parsed = tokens(text)?;
    if parsed.is_empty() {
        return Err("No paths supplied. Quote paths containing spaces.".into());
    }
    Ok(parsed.iter().map(|s| expand_home(s)).collect())
}
fn existing_prefix(text: &str) -> Option<(PathBuf, usize)> {
    // A file path cannot exceed typical OS path limits. Bound probing independently
    // of the number of files or the length of a following prompt.
    let mut ends: Vec<usize> = text
        .char_indices()
        .take_while(|(i, _)| *i <= 4096)
        .filter(|(_, c)| c.is_whitespace() || *c == ',')
        .map(|(i, _)| i)
        .collect();
    if text.len() <= 4096 {
        ends.push(text.len());
    }
    for end in ends.into_iter().rev() {
        let candidate = path(&text[..end]);
        if candidate.is_file() {
            return Some((candidate, end));
        }
    }
    None
}
/// Cheap drag-and-drop hint. Never probe a drive from the terminal thread.
/// Quoted relative names are candidates too; a worker restores non-path text.
pub fn looks_like_drop(text: &str) -> bool {
    let text = text.trim_start();
    if text.starts_with("//") || text.is_empty() {
        return false;
    }
    if text.starts_with(['\'', '"']) {
        return true;
    }
    if text.starts_with("~/") || text.starts_with("./") || text.starts_with("../") {
        return true;
    }
    text.strip_prefix('/').is_some_and(|rest| {
        rest.split_whitespace()
            .next()
            .is_some_and(|first| first.contains('/'))
    })
}
/// Recognize consecutive existing local files, retaining the following prompt.
/// This stages files only; it never uploads them or interprets shell syntax.
pub fn dropped_paths(text: &str) -> Option<(Vec<PathBuf>, String)> {
    let mut rest = text.trim();
    if rest.starts_with("//")
        || rest.len() > 1_048_576
        || !rest.starts_with(['/', '~', '\'', '"', '.'])
    {
        return None;
    }
    let mut paths = Vec::new();
    while let Some((candidate, end)) = existing_prefix(rest) {
        paths.push(candidate);
        rest = rest[end..].trim_start_matches([',', ' ', '\n', '\r', '\t']);
        if rest.is_empty() || !rest.starts_with(['/', '~', '\'', '"', '.']) {
            break;
        }
    }
    (!paths.is_empty()).then(|| (paths, rest.to_string()))
}

pub fn load(path: &std::path::Path) -> Result<Attachment, String> {
    let display = path.display().to_string();
    let limits = limits().map_err(|reason| format!("Cannot attach {display}: {reason}"))?;
    load_with_limits(path, limits)
}
fn file_kind(path: &std::path::Path) -> Result<(&'static str, &'static str), String> {
    let ext = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    let (kind, mime) = match ext.as_str() {
        "png" => ("image", "image/png"),
        "jpg" | "jpeg" => ("image", "image/jpeg"),
        "webp" => ("image", "image/webp"),
        "gif" => ("image", "image/gif"),
        "pdf" => ("file", "application/pdf"),
        "mp4" => ("video", "video/mp4"),
        "mov" => ("video", "video/mov"),
        "mpeg" | "mpg" => ("video", "video/mpeg"),
        "webm" => ("video", "video/webm"),
        "mp3" => ("audio", "mp3"),
        "wav" => ("audio", "wav"),
        "aiff" | "aif" => ("audio", "aiff"),
        "aac" => ("audio", "aac"),
        "ogg" => ("audio", "ogg"),
        "flac" => ("audio", "flac"),
        "m4a" => ("audio", "m4a"),
        // Raw PCM carries no header; 16-bit is the common default.
        "pcm" | "pcm16" => ("audio", "pcm16"),
        "pcm24" => ("audio", "pcm24"),
        "zip" | "exe" | "dmg" | "docx" | "xlsx" | "pptx" | "heic" => {
            return Err(format!(
                "unsupported .{ext} format; convert documents to PDF/text or images to PNG/JPEG"
            ));
        }
        _ => ("text", "text/plain"),
    };
    Ok((kind, mime))
}
fn load_with_limits(path: &std::path::Path, limits: Limits) -> Result<Attachment, String> {
    let display = path.display().to_string();
    let result = (|| -> Result<Attachment, String> {
        let (kind, mime) = file_kind(path)?;
        let resolved = fs::canonicalize(path).map_err(|e| e.to_string())?;
        let before = fs::metadata(&resolved).map_err(|e| e.to_string())?;
        if !before.is_file() {
            return Err("not a regular file".into());
        }
        let read_limit = if kind == "text" {
            limits.file_bytes.min(limits.text_bytes)
        } else {
            limits.file_bytes
        };
        if before.len() > read_limit {
            return Err(format!(
                "larger than the {} MiB {} limit; adjust {} if the provider permits it",
                read_limit / MIB,
                if kind == "text" && limits.text_bytes < limits.file_bytes {
                    "text"
                } else {
                    "per-file"
                },
                if kind == "text" && limits.text_bytes < limits.file_bytes {
                    "PITTY_MAX_TEXT_MIB"
                } else {
                    "PITTY_MAX_FILE_MIB"
                }
            ));
        }
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            use std::os::unix::fs::OpenOptionsExt;
            #[cfg(target_os = "macos")]
            const FLAGS: i32 = 0x4 | 0x100; // O_NONBLOCK | O_NOFOLLOW
            #[cfg(target_os = "linux")]
            const FLAGS: i32 = 0x800 | 0x20000;
            options.custom_flags(FLAGS);
        }
        let mut file = options.open(&resolved).map_err(|e| e.to_string())?;
        let opened = file.metadata().map_err(|e| e.to_string())?;
        if !opened.is_file() || !same_snapshot(&before, &opened) {
            return Err("file changed while opening; select it again".into());
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(read_limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > read_limit {
            return Err(format!(
                "file grew beyond the {} MiB limit",
                read_limit / MIB
            ));
        }
        let after = file.metadata().map_err(|e| e.to_string())?;
        let target = fs::metadata(&resolved).map_err(|e| e.to_string())?;
        if fs::canonicalize(path).map_err(|e| e.to_string())? != resolved
            || !same_snapshot(&opened, &after)
            || !same_snapshot(&opened, &target)
        {
            return Err("file changed while reading; select it again".into());
        }
        if bytes.is_empty() {
            return Err("file is empty".into());
        }
        validate_signature(mime, &bytes)?;
        let name = path
            .file_name()
            .ok_or("missing filename")?
            .to_string_lossy()
            .to_string();
        let data = if kind == "text" {
            let text = String::from_utf8(bytes).map_err(|_| "unsupported binary format; use a supported image, PDF, audio/video, or UTF-8 text file")?;
            // Other control bytes signal binary data. Page breaks (form feed,
            // vertical tab) appear in RFCs and some sources, and ESC starts the
            // color codes in saved logs.
            if text.chars().any(|c| {
                c.is_control() && !matches!(c, '\n' | '\r' | '\t' | '\x0b' | '\x0c' | '\x1b')
            }) {
                return Err("file contains binary control bytes".into());
            }
            // Colored logs lose their escape codes; other text is sent unchanged.
            if text.contains('\x1b') {
                pitty_ui::sanitize_copy(&text)
            } else {
                text
            }
        } else {
            STANDARD.encode(bytes)
        };
        Ok(Attachment {
            name,
            kind: kind.into(),
            mime: mime.into(),
            data,
        })
    })();
    result.map_err(|reason| format!("Cannot attach {display}: {reason}."))
}
fn same_snapshot(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        a.dev() == b.dev()
            && a.ino() == b.ino()
            && a.len() == b.len()
            && a.mtime() == b.mtime()
            && a.mtime_nsec() == b.mtime_nsec()
            && a.ctime() == b.ctime()
            && a.ctime_nsec() == b.ctime_nsec()
    }
    #[cfg(not(unix))]
    {
        a.len() == b.len() && a.modified().ok() == b.modified().ok()
    }
}
fn validate_signature(mime: &str, bytes: &[u8]) -> Result<(), String> {
    // Reject obvious renamed/non-media files. This is a signature check, not a
    // codec parser; provider-specific decoding remains the provider's decision.
    let valid = match mime {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "image/webp" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
        "application/pdf" => bytes
            .iter()
            .take(1024)
            .copied()
            .collect::<Vec<_>>()
            .windows(5)
            .any(|w| w == b"%PDF-"),
        "wav" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE"),
        "flac" => bytes.starts_with(b"fLaC"),
        "ogg" => bytes.starts_with(b"OggS"),
        "aiff" => bytes.starts_with(b"FORM") && matches!(bytes.get(8..12), Some(b"AIFF" | b"AIFC")),
        _ => true,
    };
    if valid {
        Ok(())
    } else {
        Err(format!("contents do not match the declared {mime} format"))
    }
}

fn same_attachment(a: &Attachment, b: &Attachment) -> bool {
    a.name == b.name && a.kind == b.kind && a.mime == b.mime && a.data == b.data
}
fn stored_size(file: &Attachment) -> Result<usize, String> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(128);
    serde_json::to_writer(&mut counter, file).map_err(|e| e.to_string())?;
    Ok(counter.0)
}
fn fingerprint(file: &Attachment) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    (&file.name, &file.kind, &file.mime, &file.data).hash(&mut hash);
    hash.finish()
}
/// Atomically stage unique payloads without allocating serialized copies.
/// Hashes index candidates only; full equality resolves collisions.
pub fn stage_batch(
    existing: &mut Vec<Attachment>,
    files: Vec<Attachment>,
) -> Result<usize, String> {
    use std::collections::HashMap;
    let limit = chat_limit()?;
    let mut total = 0usize;
    let mut seen: HashMap<u64, Vec<usize>> = HashMap::new();
    for (index, file) in existing.iter().enumerate() {
        total = total.saturating_add(stored_size(file)?);
        seen.entry(fingerprint(file)).or_default().push(index);
    }
    let original = existing.len();
    let mut pending: Vec<Attachment> = Vec::new();
    for file in files {
        let hash = fingerprint(&file);
        if seen.get(&hash).is_some_and(|indices| {
            indices.iter().any(|&i| {
                same_attachment(
                    if i < original {
                        &existing[i]
                    } else {
                        &pending[i - original]
                    },
                    &file,
                )
            })
        }) {
            continue;
        }
        total = total.saturating_add(stored_size(&file)?);
        if total > limit {
            return Err(format!(
                "Cannot attach {}: combined files exceed {} MiB. Use /detach to make room.",
                file.name,
                limit / MIB as usize
            ));
        }
        seen.entry(hash).or_default().push(original + pending.len());
        pending.push(file);
    }
    let count = pending.len();
    existing.extend(pending);
    Ok(count)
}
/// Stage once without duplicating the payload or making an upload. Return whether added.
pub fn stage(existing: &mut Vec<Attachment>, file: Attachment) -> Result<bool, String> {
    stage_batch(existing, vec![file]).map(|added| added > 0)
}
pub fn load_batch(paths: &[PathBuf]) -> Result<Vec<Attachment>, String> {
    use std::collections::HashMap;
    let mut result: Vec<Attachment> = Vec::new();
    let mut seen: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut total = 0usize;
    let limit = chat_limit()?;
    for path in paths {
        let file = load(path)?;
        let hash = fingerprint(&file);
        if seen
            .get(&hash)
            .is_some_and(|indices| indices.iter().any(|&i| same_attachment(&result[i], &file)))
        {
            continue;
        }
        total = total.saturating_add(stored_size(&file)?);
        if total > limit {
            return Err(format!(
                "Cannot attach {}: this batch exceeds {} MiB; no files staged.",
                path.display(),
                limit / MIB as usize
            ));
        }
        seen.entry(hash).or_default().push(result.len());
        result.push(file);
    }
    Ok(result)
}

pub fn content(file: &Attachment) -> Value {
    let data_url = || format!("data:{};base64,{}", file.mime, file.data);
    match file.kind.as_str() {
        "image" => json!({"type":"image_url","image_url":{"url":data_url()}}),
        "video" => json!({"type":"video_url","video_url":{"url":data_url()}}),
        "audio" => {
            json!({"type":"input_audio","input_audio":{"data":file.data,"format":file.mime}})
        }
        "file" => json!({"type":"file","file":{"filename":file.name,"file_data":data_url()}}),
        _ => json!({"type":"text","text":format!("Attached file: {}\n\n{}", file.name, file.data)}),
    }
}
pub(crate) const TOOL_BATCH_LIMIT: usize = 8;
pub(crate) const TOOL_ARGUMENT_LIMIT: usize = 1_048_576;
pub(crate) const TOOL_TRACE_LIMIT: usize = 16 * (TOOL_BATCH_LIMIT + 1);

/// Only replay an ordered assistant/function-result exchange. History files are
/// data, not permission to inject arbitrary system/user messages into a request.
pub(crate) fn validate_tool_trace(trace: &Value) -> Result<(), String> {
    use std::collections::HashSet;
    let rows = trace
        .as_array()
        .ok_or("Invalid saved tool trace: expected an array.")?;
    if rows.is_empty() || rows.len() > TOOL_TRACE_LIMIT {
        return Err("Invalid saved tool trace: too many or no messages.".into());
    }
    let mut pending = HashSet::new();
    for row in rows {
        match row["role"].as_str() {
            Some("assistant") if pending.is_empty() => {
                let calls = function_calls(row)?;
                if calls.is_empty() {
                    return Err("Invalid saved tool trace: assistant has no calls.".into());
                }
                if !row["content"].is_null() && !row["content"].is_string() {
                    return Err("Invalid saved tool trace: assistant content is not text.".into());
                }
                for (id, _, _) in calls {
                    pending.insert(id);
                }
            }
            Some("tool") => {
                let id = row["tool_call_id"]
                    .as_str()
                    .ok_or("Invalid saved tool trace: missing result ID.")?;
                if !pending.remove(id) || !row["content"].is_string() {
                    return Err("Invalid saved tool trace: unmatched or duplicate result.".into());
                }
            }
            _ => {
                return Err(
                    "Invalid saved tool trace: unexpected message role or missing results.".into(),
                );
            }
        }
    }
    if !pending.is_empty() {
        return Err("Invalid saved tool trace: incomplete results.".into());
    }
    Ok(())
}

/// Parse only the API's structured function-call field, never assistant prose.
pub(crate) fn function_calls(message: &Value) -> Result<Vec<(&str, &str, &str)>, String> {
    let Some(value) = message.get("tool_calls").filter(|v| !v.is_null()) else {
        return Ok(vec![]);
    };
    let calls = value
        .as_array()
        .ok_or("Invalid function calls: expected an array.")?;
    if !calls.is_empty() && !message["content"].is_null() && !message["content"].is_string() {
        return Err("Invalid function-call message: assistant content is not text.".into());
    }
    if calls.len() > TOOL_BATCH_LIMIT {
        return Err("Model requested more than eight tools in one turn; no tools from that turn were executed.".into());
    }
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::with_capacity(calls.len());
    for call in calls {
        let id = call["id"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 256 && s.bytes().all(|c| c.is_ascii_graphic()))
            .ok_or("Invalid function call ID.")?;
        if !seen.insert(id) {
            return Err(
                "Duplicate function call ID; no tools from that turn were executed.".into(),
            );
        }
        if call["type"] != "function" {
            return Err("Unsupported tool-call type.".into());
        }
        let name = call["function"]["name"]
            .as_str()
            .filter(|s| {
                !s.is_empty()
                    && s.len() <= 128
                    && s.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
            })
            .ok_or("Invalid function name.")?;
        let arguments = call["function"]["arguments"]
            .as_str()
            .filter(|s| s.len() <= TOOL_ARGUMENT_LIMIT)
            .ok_or("Invalid function arguments or arguments exceed 1 MiB.")?;
        result.push((id, name, arguments));
    }
    Ok(result)
}

pub fn messages(messages: &[pitty_core::Message]) -> Result<Value, String> {
    let mut result = Vec::new();
    for message in messages {
        if let Some(trace) = &message.tool_trace {
            if message.role != "assistant" {
                return Err("Only assistant messages may contain a saved tool trace.".into());
            }
            validate_tool_trace(trace)?;
            // Normalize allowed protocol fields, excluding unexpected fields in
            // a modified history file while keeping reasoning signatures intact.
            for row in trace.as_array().unwrap() {
                let mut copy = serde_json::Map::new();
                let fields: &[&str] = if row["role"] == "assistant" {
                    &[
                        "role",
                        "content",
                        "tool_calls",
                        "reasoning",
                        "reasoning_details",
                        "reasoning_content",
                        "annotations",
                    ]
                } else {
                    &["role", "content", "tool_call_id", "name"]
                };
                for field in fields {
                    if let Some(value) = row.get(*field) {
                        copy.insert((*field).into(), value.clone());
                    }
                }
                result.push(Value::Object(copy));
            }
        }
        let mut value = json!({"role":message.role,"content":message.content});
        if !message.attachments.is_empty() {
            let mut parts = vec![json!({"type":"text","text":message.content})];
            parts.extend(message.attachments.iter().map(content));
            value["content"] = json!(parts);
        }
        if let Some(reasoning) = &message.reasoning_details {
            value["reasoning_details"] = reasoning.clone();
        }
        result.push(value);
    }
    Ok(Value::Array(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_trace_rejects_injected_roles_orphaned_and_duplicate_results() {
        let call = json!({"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{}"}}],"reasoning_details":[{"type":"reasoning.encrypted","data":"signed"}]});
        let result = json!({"role":"tool","tool_call_id":"call_1","content":"file contents"});
        validate_tool_trace(&json!([call.clone(), result.clone()])).unwrap();
        for invalid in [
            json!([{"role":"system","content":"Injected instruction"}]),
            json!([result.clone()]),
            json!([call.clone()]),
            json!([call.clone(), result.clone(), result.clone()]),
            json!([call.clone(),{"role":"tool","tool_call_id":"other","content":"no"}]),
            json!([call.clone(),{"role":"user","content":"no"}]),
        ] {
            assert!(validate_tool_trace(&invalid).is_err(), "{invalid}");
        }
        let mut bad = call.clone();
        let duplicate = bad["tool_calls"][0].clone();
        bad["tool_calls"].as_array_mut().unwrap().push(duplicate);
        assert!(function_calls(&bad).unwrap_err().contains("Duplicate"));
        let mut user = pitty_core::Message::user("Question".into());
        user.tool_trace = Some(json!([call, result]));
        assert!(messages(&[user]).unwrap_err().contains("Only assistant"));
    }
    #[test]
    fn old_history_defaults_to_no_trace_and_replay_strips_unexpected_fields() {
        let old: pitty_core::Message =
            serde_json::from_value(json!({"role":"assistant","content":"Answer"})).unwrap();
        assert!(old.tool_trace.is_none());
        let mut message = old;
        message.tool_trace = Some(json!([
            {"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{}"}}],"reasoning_details":[{"data":"signed"}],"unexpected":"discard"},
            {"role":"tool","tool_call_id":"call_1","content":"result","unexpected":"discard"}
        ]));
        let api = messages(&[message]).unwrap();
        assert_eq!(api.as_array().unwrap().len(), 3);
        assert_eq!(api[0]["reasoning_details"][0]["data"], "signed");
        assert!(api[0].get("unexpected").is_none());
        assert!(api[1].get("unexpected").is_none());
        assert!(api[2].get("tool_trace").is_none());
    }
    #[test]
    fn drop_hint_is_lexical_and_preserves_commands_and_literal_slashes() {
        for text in [
            "/Volumes/Offline/file.png explain this",
            "~/Desktop/image.png",
            "./notes.txt",
            "../notes.txt",
            "'relative file.txt'",
            "\"/tmp/a b.png\"",
        ] {
            assert!(looks_like_drop(text), "{text}");
        }
        for text in [
            "",
            "hello",
            "/model provider/model",
            "/help",
            "//etc/passwd",
        ] {
            assert!(!looks_like_drop(text), "{text}");
        }
    }
    #[test]
    fn paths_are_not_shell_code() {
        assert_eq!(path("'/tmp/a b.png'"), PathBuf::from("/tmp/a b.png"));
        assert_eq!(path("/tmp/a\\ b.png"), PathBuf::from("/tmp/a b.png"));
        assert_eq!(path(r"'/tmp/a\b.png'"), PathBuf::from(r"/tmp/a\b.png"));
        assert_eq!(
            path("/tmp/$(touch bad).png"),
            PathBuf::from("/tmp/$(touch bad).png")
        );
    }
    #[test]
    fn correct_parts_and_no_local_paths_in_payload() {
        let file = Attachment {
            name: "photo.png".into(),
            kind: "image".into(),
            mime: "image/png".into(),
            data: "YWJj".into(),
        };
        assert_eq!(
            content(&file)["image_url"]["url"],
            "data:image/png;base64,YWJj"
        );
        let mut message = pitty_core::Message::user("Describe this".into());
        message.attachments.push(file);
        assert_eq!(
            messages(&[message]).unwrap()[0]["content"][1]["type"],
            "image_url"
        );
    }
    #[test]
    fn errors_name_the_exact_file() {
        assert!(
            load(std::path::Path::new("/nonexistent/rejected-file.png"))
                .unwrap_err()
                .contains("/nonexistent/rejected-file.png")
        );
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            // Parallel tests can read the same clock value; a counter keeps
            // every fixture directory unique.
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "pitty-attachments-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn file(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, b"test content").unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn multiple_quoted_and_escaped_paths_preserve_following_prompt() {
        let fixture = Fixture::new();
        let first = fixture.file("first image.png");
        let second = fixture.file("second image.jpg");
        let text = format!(
            "'{}' {} , describe both",
            first.display(),
            second.display().to_string().replace(' ', "\\ ")
        );
        let (paths, prompt) = dropped_paths(&text).unwrap();
        assert_eq!(paths, vec![first.clone(), second.clone()]);
        assert_eq!(prompt, "describe both");
        let args = format!("'{}' \"{}\"", first.display(), second.display());
        assert_eq!(explicit_paths(&args).unwrap(), vec![first, second]);
        assert!(dropped_paths("//etc/passwd").is_none());
    }
    #[test]
    fn many_files_are_not_limited_by_count() {
        let fixture = Fixture::new();
        let paths: Vec<_> = (0..32)
            .map(|i| fixture.file(&format!("file-{i}.txt")))
            .collect();
        let input = paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(explicit_paths(&input).unwrap(), paths);
        assert_eq!(dropped_paths(&input).unwrap().0.len(), 32);
    }
    #[test]
    fn limits_are_configurable_but_bounded_and_errors_name_file() {
        assert_eq!(
            configured_mib("PITTY_MAX_CHAT_MIB", Some("80"), 28, 80).unwrap(),
            80 * MIB
        );
        for value in ["0", "81", "18446744073709551615", "no"] {
            assert!(
                configured_mib("PITTY_MAX_CHAT_MIB", Some(value), 28, 80)
                    .unwrap_err()
                    .contains("PITTY_MAX_CHAT_MIB")
            );
        }
        let fixture = Fixture::new();
        let file = fixture.file("oversized.txt");
        let limits = Limits {
            file_bytes: 4,
            text_bytes: 4,
            chat_bytes: 4,
        };
        let error = load_with_limits(&file, limits).unwrap_err();
        assert!(error.contains(&file.display().to_string()));
        assert!(error.contains("per-file"));
        let error = load_with_limits(&fixture.0, Limits::default()).unwrap_err();
        assert!(error.contains("not a regular file"));
    }
    #[test]
    fn unsupported_files_report_full_name_and_audio_parts_preserve_format() {
        let fixture = Fixture::new();
        let file = fixture.file("rejected presentation.pptx");
        assert!(
            load_with_limits(&file, Limits::default())
                .unwrap_err()
                .contains(&file.display().to_string())
        );
        let audio_path = fixture.file("audio.flac");
        fs::write(&audio_path, b"fLaCtest content").unwrap();
        let audio = load_with_limits(&audio_path, Limits::default()).unwrap();
        assert_eq!(content(&audio)["input_audio"]["format"], "flac");
        assert_eq!(
            content(&audio)["input_audio"]["data"],
            STANDARD.encode(b"fLaCtest content")
        );
    }
    #[test]
    fn media_signatures_and_binary_text_fail_with_exact_filename() {
        let fixture = Fixture::new();
        let image = fixture.file("renamed.png");
        let error = load(&image).unwrap_err();
        assert!(error.contains(&image.display().to_string()));
        assert!(error.contains("image/png"));
        let binary = fixture.file("notes.txt");
        fs::write(&binary, b"abc\0def").unwrap();
        assert!(load(&binary).unwrap_err().contains("binary control bytes"));
        fs::write(&binary, [0xff, 0xfe]).unwrap();
        assert!(load(&binary).unwrap_err().contains("unsupported binary"));
        fs::write(&image, b"\x89PNG\r\n\x1a\nheader").unwrap();
        assert!(load(&image).is_ok());
    }
    #[test]
    fn smaller_text_limit_is_checked_before_reading() {
        let fixture = Fixture::new();
        let text = fixture.file("text.txt");
        let error = load_with_limits(
            &text,
            Limits {
                file_bytes: 64,
                text_bytes: 4,
                chat_bytes: 64,
            },
        )
        .unwrap_err();
        assert!(error.contains("text limit"));
        assert!(error.contains("PITTY_MAX_TEXT_MIB"));
    }
    #[test]
    fn repeated_files_do_not_duplicate_staged_payloads() {
        let fixture = Fixture::new();
        let path = fixture.file("same.rs");
        let files = load_batch(&[path.clone(), path.clone(), path]).unwrap();
        assert_eq!(files.len(), 1);
        let file = files[0].clone();
        let mut staged = Vec::new();
        assert_eq!(stage_batch(&mut staged, files).unwrap(), 1);
        assert!(!stage(&mut staged, file.clone()).unwrap());
        assert_eq!(staged.len(), 1);
        let mut changed = file;
        changed.data.push_str(" changed");
        assert!(stage(&mut staged, changed).unwrap());
        assert_eq!(staged.len(), 2);
    }
    #[cfg(unix)]
    #[test]
    fn snapshot_identity_detects_same_length_replacements() {
        let fixture = Fixture::new();
        let file = fixture.file("original.txt");
        let before = fs::metadata(&file).unwrap();
        let replacement = fixture.file("replacement.txt");
        fs::rename(replacement, &file).unwrap();
        assert!(!same_snapshot(&before, &fs::metadata(&file).unwrap()));
    }
}
