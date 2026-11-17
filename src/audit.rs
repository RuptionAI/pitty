//! Local decision log.
//!
//! Each approval, decline, refusal, and gated run appends one line. The line
//! records the decision, who allowed it, the tool, and the path or command.
//! It does not record file contents or command output. `prev` is the SHA-256
//! of the previous line, including its newline, so a changed or deleted middle
//! line fails a later check. The file lives on this computer. Someone who can
//! rewrite the whole file can build a new chain, and this check cannot see that.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_LOG: u64 = 32 * 1024 * 1024;
const MAX_SUBJECT: usize = 240;
const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Line {
    v: u8,
    n: u64,
    t: String,
    prev: String,
    decision: String,
    actor: String,
    tool: String,
    subject: String,
}

pub(crate) fn note(actor: &str, decision: &str, tool: &str, subject: &str, secret: &str) {
    let Some(path) = destination() else {
        remember_failure(
            &default_failure_path(),
            "Cannot place the audit log. Set HOME or XDG_DATA_HOME to an absolute path.",
        );
        return;
    };
    match append_at(&path, actor, decision, tool, subject, secret) {
        Ok(()) => clear_failure(&path),
        Err(error) => remember_failure(&failure_path(&path), &error),
    }
}

pub(crate) fn explain() -> Vec<String> {
    match destination() {
        Some(path) => explain_at(&path),
        None => vec![
            "Pitty cannot place an audit log.".into(),
            "Set HOME or XDG_DATA_HOME to an absolute path.".into(),
        ],
    }
}

fn destination() -> Option<PathBuf> {
    // Tests must not append to the account that is running them.
    if cfg!(test) {
        return Some(std::env::temp_dir().join(format!("pitty-audit-{}.log", std::process::id())));
    }
    crate::paths::data_dir().map(|dir| dir.join("audit.log"))
}

fn default_failure_path() -> PathBuf {
    std::env::temp_dir().join(format!("pitty-audit-{}.failure", std::process::id()))
}

fn failure_path(log: &Path) -> PathBuf {
    log.with_file_name("audit.failure")
}

fn append_at(
    path: &Path,
    actor: &str,
    decision: &str,
    tool: &str,
    subject: &str,
    secret: &str,
) -> Result<(), String> {
    if !crate::audit_cert::known(actor, decision) {
        return Err("Internal audit record was refused.".into());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Cannot create the audit directory: {error}"))?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("Cannot open the audit log: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("Cannot restrict the audit log: {error}"))?;
    }
    let _lock = Lock::exclusive(&file)?;
    let len = file
        .metadata()
        .map_err(|error| format!("Cannot read the audit log: {error}"))?
        .len();
    if len >= MAX_LOG {
        return Err("The audit log has reached 32 MiB, so Pitty stopped appending.".into());
    }
    let (n, prev) = tip(&mut file, len)?;
    let line = serde_json::json!({
        "v": 1,
        "n": n,
        "t": utc_now(),
        "prev": prev,
        "decision": decision,
        "actor": actor,
        "tool": bound(tool, 64),
        "subject": sanitize(subject, secret),
    });
    let body = serde_json::to_string(&line).map_err(|error| error.to_string())?;
    file.seek(SeekFrom::End(0))
        .map_err(|error| format!("Cannot write the audit log: {error}"))?;
    file.write_all(body.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("Cannot write the audit log: {error}"))?;
    Ok(())
}

fn tip(file: &mut File, len: u64) -> Result<(u64, String), String> {
    if len == 0 {
        return Ok((1, GENESIS.into()));
    }
    let start = len.saturating_sub(4096);
    file.seek(SeekFrom::Start(start))
        .map_err(|error| format!("Cannot read the audit log: {error}"))?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .map_err(|error| format!("Cannot read the audit log: {error}"))?;
    if !buffer.ends_with(b"\n") {
        return Err("The audit log ends with a partial line.".into());
    }
    let text = String::from_utf8(buffer).map_err(|_| "The audit log is not UTF-8.".to_string())?;
    let last = text
        .lines()
        .next_back()
        .filter(|line| !line.is_empty())
        .ok_or_else(|| "The audit log's last line is empty.".to_string())?;
    let parsed: Line = serde_json::from_str(last)
        .map_err(|_| "The audit log's last line is not a record.".to_string())?;
    if parsed.v != 1 {
        return Err("The audit log's last line is not a record.".into());
    }
    let mut bytes = last.as_bytes().to_vec();
    bytes.push(b'\n');
    Ok((parsed.n.saturating_add(1), digest(&bytes)))
}

fn explain_at(path: &Path) -> Vec<String> {
    let mut lines = vec![
        "Decisions only. File contents and command output stay out of this log.".into(),
        "Each line stores the SHA-256 of the line before it.".into(),
        "A changed or deleted middle line breaks this check.".into(),
        "The log stays on this computer. Rewriting the whole file is not visible here.".into(),
        "LeanRust certifies the numeric admission rule, redaction from 8 bytes, and the subject cap.".into(),
        "The log's words, this program, the hash, and the disk are outside that certificate.".into(),
        String::new(),
    ];
    if let Some(failure) = read_failure(&failure_path(path)) {
        lines.push(format!("Last write failed: {failure}"));
        lines.push(String::new());
    }
    match verify(path) {
        Ok(report) => {
            lines.push(format!(
                "{} · {} records · {}",
                if report.intact {
                    "Chain intact"
                } else {
                    "Chain broken"
                },
                report.records,
                path.display()
            ));
            if let Some(problem) = report.problem {
                lines.push(problem);
            }
            if !report.recent.is_empty() {
                lines.push(String::new());
                lines.extend(report.recent);
            }
        }
        Err(error) => lines.push(error),
    }
    lines
}

struct Report {
    records: usize,
    intact: bool,
    problem: Option<String>,
    recent: Vec<String>,
}

fn verify(path: &Path) -> Result<Report, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Report {
                records: 0,
                intact: true,
                problem: None,
                recent: vec!["No decisions recorded yet.".into()],
            });
        }
        Err(error) => return Err(format!("Cannot read the audit log: {error}")),
    };
    if bytes.len() as u64 > MAX_LOG {
        return Err("The audit log exceeds 32 MiB, so this check stopped.".into());
    }
    if bytes.is_empty() {
        return Ok(Report {
            records: 0,
            intact: true,
            problem: None,
            recent: vec!["No decisions recorded yet.".into()],
        });
    }
    if !bytes.ends_with(b"\n") {
        return Ok(broken(0, "The audit log ends with a partial line."));
    }
    let text = String::from_utf8(bytes).map_err(|_| "The audit log is not UTF-8.".to_string())?;
    let mut previous = GENESIS.to_string();
    let mut recent = Vec::new();
    let mut records = 0usize;
    for (index, line) in text.lines().enumerate() {
        if line.is_empty() {
            return Ok(broken(index, "The audit log contains an empty line."));
        }
        let parsed: Line = match serde_json::from_str(line) {
            Ok(parsed) => parsed,
            Err(_) => {
                return Ok(broken(
                    index,
                    &format!("Record {} is not a decision line.", index + 1),
                ));
            }
        };
        records += 1;
        if parsed.v != 1 || parsed.n != records as u64 || parsed.prev != previous {
            return Ok(broken(
                index,
                &format!("Record {} does not follow the line before it.", index + 1),
            ));
        }
        if !crate::audit_cert::known(&parsed.actor, &parsed.decision) {
            return Ok(broken(
                index,
                &format!("Record {} has an unknown decision.", index + 1),
            ));
        }
        let mut hashed = line.as_bytes().to_vec();
        hashed.push(b'\n');
        previous = digest(&hashed);
        recent.push(format!(
            "{} · {} · {} · {}",
            parsed.t, parsed.decision, parsed.tool, parsed.subject
        ));
    }
    let start = recent.len().saturating_sub(12);
    Ok(Report {
        records,
        intact: true,
        problem: None,
        recent: recent.split_off(start),
    })
}

fn broken(index: usize, problem: &str) -> Report {
    Report {
        records: index,
        intact: false,
        problem: Some(problem.into()),
        recent: Vec::new(),
    }
}

fn sanitize(subject: &str, secret: &str) -> String {
    let mut text = if crate::audit_cert::hides_secret(secret.len()) {
        subject.replace(secret, "[REDACTED]")
    } else {
        subject.to_string()
    };
    text = text
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    bound(&text, MAX_SUBJECT)
}

fn bound(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    let keep = crate::audit_cert::kept(count, limit);
    if keep == count {
        return text.to_string();
    }
    let mut end = text.chars().take(keep).map(char::len_utf8).sum();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut shortened = text[..end].to_string();
    shortened.push('…');
    shortened
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn remember_failure(path: &Path, error: &str) {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut text = bound(error, 300);
    text.push('\n');
    if let Ok(mut file) = options.open(path) {
        let _ = file.write_all(text.as_bytes());
    }
}

fn clear_failure(log: &Path) {
    let _ = fs::remove_file(failure_path(log));
}

fn read_failure(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let line = text.lines().next()?.trim();
    if line.is_empty() {
        None
    } else {
        Some(line.to_string())
    }
}

fn utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let (year, month, day) = civil_from_days((secs / 86_400) as i64);
    let tod = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

/// Howard Hinnant's civil_from_days, days since 1970-01-01.
fn civil_from_days(mut z: i64) -> (i32, u32, u32) {
    z += 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

struct Lock(#[cfg(unix)] i32);

impl Lock {
    fn exclusive(file: &File) -> Result<Self, String> {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = file.as_raw_fd();
            let result = unsafe { libc::flock(fd, libc::LOCK_EX) };
            if result != 0 {
                return Err(format!(
                    "Cannot lock the audit log: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(Self(fd))
        }
        #[cfg(not(unix))]
        {
            let _ = file;
            Ok(Self())
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::flock(self.0, libc::LOCK_UN);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn temp_log(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pitty-audit-{}-{}-{name}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_file(&path);
        path
    }

    #[test]
    fn epoch_is_the_unix_civil_date() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(1), (1970, 1, 2));
    }

    #[test]
    fn chain_links_each_line_and_hides_the_key() {
        let path = temp_log("chain");
        let secret = "sk-or-secret-value-123456";
        append_at(
            &path,
            "user",
            "approved",
            "edit_file",
            &format!("Edit src/main.rs {secret}"),
            secret,
        )
        .unwrap();
        append_at(&path, "user", "ran", "edit_file", "Edit src/main.rs", "").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains(secret));
        assert!(text.contains("[REDACTED]"));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let first = format!("{}\n", lines[0]);
        let second: Line = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second.prev, digest(first.as_bytes()));
        assert_eq!(second.n, 2);
        let report = verify(&path).unwrap();
        assert!(report.intact);
        assert_eq!(report.records, 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_changed_middle_line_breaks_the_check() {
        let path = temp_log("tamper");
        append_at(
            &path,
            "user",
            "declined",
            "run_command",
            "Run in /tmp: true",
            "",
        )
        .unwrap();
        append_at(
            &path,
            "session",
            "auto",
            "run_command",
            "Run in /tmp: true",
            "",
        )
        .unwrap();
        let text = fs::read_to_string(&path)
            .unwrap()
            .replace("declined", "approved");
        fs::write(&path, text).unwrap();
        let report = verify(&path).unwrap();
        assert!(!report.intact);
        assert!(report.problem.unwrap().contains("Record 2"));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn command_text_is_truncated_and_output_is_not_a_field() {
        let path = temp_log("bound");
        let subject = format!("echo {}", "x".repeat(2000));
        append_at(&path, "user", "ran", "run_command", &subject, "").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.len() < 2000);
        assert!(!text.contains("stdout"));
        assert!(text.contains('…'));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn concurrent_appends_keep_one_chain() {
        let path = temp_log("concurrent");
        let barrier = Arc::new(Barrier::new(4));
        let mut handles = Vec::new();
        for id in 0..4 {
            let path = path.clone();
            let barrier = barrier.clone();
            handles.push(thread::spawn(move || {
                barrier.wait();
                for _ in 0..10 {
                    append_at(
                        &path,
                        "mcp",
                        "refused",
                        "read_file",
                        &format!("worker {id}"),
                        "",
                    )
                    .unwrap();
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        let report = verify(&path).unwrap();
        assert!(report.intact, "{:?}", report.problem);
        assert_eq!(report.records, 40);
        let _ = fs::remove_file(&path);
    }
}
