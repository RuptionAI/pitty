//! One-shot local tools keep stdout machine-readable and approval on the TTY.
use crate::{Args, chat, local_tools::LocalTools, signals};
use pitty_core::{ChatReply, ChatRequest, Message};
use std::{
    io::{self, IsTerminal, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, TryRecvError},
    },
    time::Duration,
};

const POLL_INTERVAL: Duration = Duration::from_millis(40);
const APPROVAL_HINT: &str = "Action denied: approval requires interactive terminal input. Review the action in terminal chat, or explicitly choose --tools auto for unattended execution.";

/// A typed answer at the terminal: `yes` allows one action, `all` allows it and
/// every later one in this request, and anything else denies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answer {
    Deny,
    Once,
    All,
}

pub fn run(args: &Args, prompt: String) -> Result<(), String> {
    let policy = args
        .tools
        .policy()
        .ok_or_else(|| "Local tools are disabled.".to_string())?;
    let cwd = std::env::current_dir().map_err(|e| format!("Cannot read working folder: {e}"))?;
    let local = LocalTools::new(vec![cwd])?;
    // A closed terminal (SIGHUP) or SIGTERM must also stop and reap commands.
    let handlers = signals::install(&[libc::SIGINT, libc::SIGTERM, libc::SIGHUP])?;
    let cancel = Arc::new(AtomicBool::new(false));
    let (events_tx, events_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::sync_channel(1);
    let payload = ChatRequest {
        messages: vec![Message::user(prompt)],
    };
    let worker_args = args.clone();
    let worker_cancel = cancel.clone();
    let worker_policy = policy.clone();
    let worker = std::thread::Builder::new()
        .name("pitty-cli-tools".into())
        .spawn(move || {
            let result = chat::request_with_tools(
                &worker_args,
                &payload,
                worker_cancel,
                local,
                events_tx,
                worker_policy,
            );
            let _ = result_tx.send(result);
        })
        .map_err(|e| format!("Cannot start request: {e}"))?;

    let result = receive_result(
        &events_rx,
        &result_rx,
        &cancel,
        &policy,
        io::stdin().is_terminal(),
        io::stderr().is_terminal(),
        || signals::received().is_some(),
    );
    // Keep the handlers installed until command/process cleanup has finished.
    // Detaching here could leave a child running after Pitty exits.
    if worker.join().is_err() {
        return Err("Request worker stopped unexpectedly.".into());
    }
    // Every command has been reaped. Restore default handling first, so a
    // signal during a blocked final write still ends Pitty.
    drop(handlers);
    let reply = result?;
    if cancel.load(Ordering::SeqCst) || signals::received().is_some() {
        return Err("Request canceled. Completed local actions were not undone.".into());
    }
    if reply.truncated {
        diagnostic("Reply reached the output token limit; increase --max-tokens if needed.");
    }
    let output = format_reply(&reply, args.json, io::stdout().is_terminal())?;
    writeln!(io::stdout().lock(), "{output}").map_err(|e| format!("Cannot write response: {e}"))
}

fn receive_result(
    events: &Receiver<chat::ToolEvent>,
    result: &Receiver<Result<ChatReply, String>>,
    cancel: &AtomicBool,
    policy: &chat::Policy,
    interactive: bool,
    show_progress: bool,
    interrupted: impl Fn() -> bool,
) -> Result<ChatReply, String> {
    let mut announced_cancel = false;
    loop {
        if interrupted() {
            cancel.store(true, Ordering::SeqCst);
        }
        if cancel.load(Ordering::SeqCst) && !announced_cancel {
            if show_progress {
                diagnostic("Stopping request and local commands…");
            }
            announced_cancel = true;
        }
        match result.try_recv() {
            Ok(reply) => return reply,
            Err(TryRecvError::Disconnected) => {
                cancel.store(true, Ordering::SeqCst);
                return Err("Request worker stopped unexpectedly.".into());
            }
            Err(TryRecvError::Empty) => {}
        }
        match events.recv_timeout(POLL_INTERVAL) {
            Ok(chat::ToolEvent::Approval {
                summary,
                detail,
                review,
                response,
            }) => {
                let answer = if cancel.load(Ordering::SeqCst) {
                    Answer::Deny
                } else {
                    request_approval(&summary, &detail, review, cancel, interactive, &interrupted)
                };
                // Recheck after the answer. SIGINT must win over a queued yes.
                if interrupted() {
                    cancel.store(true, Ordering::SeqCst);
                }
                let allowed = answer != Answer::Deny && !cancel.load(Ordering::SeqCst);
                if allowed && answer == Answer::All {
                    policy.set_auto(true);
                }
                let _ = response.send(allowed);
            }
            Ok(chat::ToolEvent::Progress(summary)) => {
                if show_progress && !cancel.load(Ordering::SeqCst) {
                    diagnostic(&pitty_ui::sanitize(&summary));
                }
            }
            Ok(chat::ToolEvent::Completed { summary, output }) => {
                if show_progress {
                    diagnostic(&pitty_ui::sanitize(&summary));
                    if !output.trim().is_empty() {
                        diagnostic(&pitty_ui::sanitize(&output));
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                // The sender may close just before the result is posted.
                // Waiting on the result avoids a busy loop in that interval.
                match result.recv_timeout(POLL_INTERVAL) {
                    Ok(reply) => return reply,
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        cancel.store(true, Ordering::SeqCst);
                        return Err("Request worker stopped unexpectedly.".into());
                    }
                }
            }
        }
    }
}

fn request_approval(
    summary: &str,
    detail: &str,
    review: chat::Review,
    cancel: &AtomicBool,
    interactive: bool,
    interrupted: &impl Fn() -> bool,
) -> Answer {
    if !interactive {
        diagnostic(APPROVAL_HINT);
        return Answer::Deny;
    }
    match tty::approve(summary, detail, review, cancel, interrupted) {
        Ok(answer) => answer,
        Err(_) => {
            diagnostic(APPROVAL_HINT);
            Answer::Deny
        }
    }
}

fn answer_prompt(review: chat::Review) -> &'static [u8] {
    match review {
        chat::Review::Action => {
            b"Type yes to allow this action, or all to allow it and every later action in this request; Enter denies: "
        }
        chat::Review::Continue => {
            b"Type yes to continue, or all to continue and allow every later action in this request; Enter stops: "
        }
    }
}

fn diagnostic(text: &str) {
    let _ = writeln!(io::stderr().lock(), "{text}");
}

pub(crate) fn format_reply(
    reply: &ChatReply,
    json: bool,
    terminal: bool,
) -> Result<String, String> {
    if json {
        // Tool-enabled output has a stable local envelope; tool_trace contains
        // the completed protocol. Raw provider JSON remains --tools off only.
        serde_json::to_string_pretty(reply).map_err(|e| e.to_string())
    } else if terminal {
        Ok(pitty_ui::sanitize(&reply.message.content))
    } else {
        Ok(reply.message.content.clone())
    }
}

/// Show invisible controls literally rather than silently changing an action
/// during approval. Newlines remain readable; terminal escape sequences cannot
/// clear the screen or overwrite the command the user is about to approve.
fn approval_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        if (character.is_control() && character != '\n')
            || matches!(character, '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')
        {
            out.extend(character.escape_default());
        } else {
            out.push(character);
        }
    }
    out
}

#[derive(Default)]
struct ApprovalAnswer {
    bytes: Vec<u8>,
    overflow: bool,
}
impl ApprovalAnswer {
    fn push(&mut self, byte: u8) -> Option<Answer> {
        if byte == b'\n' || byte == b'\r' {
            let typed = std::str::from_utf8(&self.bytes)
                .ok()
                .filter(|_| !self.overflow)
                .map(str::trim);
            return Some(match typed {
                Some(answer) if answer.eq_ignore_ascii_case("yes") => Answer::Once,
                Some(answer) if answer.eq_ignore_ascii_case("all") => Answer::All,
                _ => Answer::Deny,
            });
        }
        if self.bytes.len() < 128 {
            self.bytes.push(byte);
        } else {
            self.overflow = true;
        }
        None
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod tty {
    use super::*;
    use libc::{poll, pollfd, tcflush};
    use std::{
        fs::OpenOptions,
        io::Read,
        os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    };

    fn write_prompt(
        tty: &mut std::fs::File,
        mut bytes: &[u8],
        cancel: &AtomicBool,
        interrupted: &impl Fn() -> bool,
    ) -> io::Result<bool> {
        while !bytes.is_empty() {
            if interrupted() {
                cancel.store(true, Ordering::SeqCst);
            }
            if cancel.load(Ordering::SeqCst) {
                return Ok(false);
            }
            match tty.write(bytes) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "Terminal closed.")),
                Ok(count) => bytes = &bytes[count..],
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let mut fd = pollfd {
                        fd: tty.as_raw_fd(),
                        events: libc::POLLOUT,
                        revents: 0,
                    };
                    // SAFETY: one initialized pollfd, bounded wait, live tty.
                    let ready = unsafe { poll(&mut fd, 1, POLL_INTERVAL.as_millis() as i32) };
                    if ready < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted
                    {
                        return Err(io::Error::last_os_error());
                    }
                    if fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                        return Ok(false);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Ok(true)
    }

    pub(super) fn approve(
        summary: &str,
        detail: &str,
        review: chat::Review,
        cancel: &AtomicBool,
        interrupted: &impl Fn() -> bool,
    ) -> io::Result<Answer> {
        let mut tty = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open("/dev/tty")?;
        if !tty.is_terminal() {
            return Ok(Answer::Deny);
        }
        // Flush input queued before this specific action was shown. In
        // canonical mode this also discards an unfinished stale input line.
        // SAFETY: tty is a live terminal fd, libc::TCIFLUSH is the platform ABI.
        if unsafe { tcflush(tty.as_raw_fd(), libc::TCIFLUSH) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let prompt = format!("\n{}\n{}\n", approval_text(summary), approval_text(detail));
        if !write_prompt(&mut tty, prompt.as_bytes(), cancel, interrupted)? {
            return Ok(Answer::Deny);
        }
        // Also discard answers typed while a long preview was still being
        // printed. Only input after the complete action can authorize it.
        // SAFETY: same live tty descriptor and platform input-queue selector.
        if unsafe { tcflush(tty.as_raw_fd(), libc::TCIFLUSH) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if !write_prompt(&mut tty, answer_prompt(review), cancel, interrupted)? {
            return Ok(Answer::Deny);
        }
        let mut answer = ApprovalAnswer::default();
        loop {
            if interrupted() {
                cancel.store(true, Ordering::SeqCst);
            }
            if cancel.load(Ordering::SeqCst) {
                let _ = writeln!(tty, "\nCanceled.");
                return Ok(Answer::Deny);
            }
            let mut fd = pollfd {
                fd: tty.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: fd points to one initialized pollfd and remains alive
            // for the bounded call. No terminal mode is modified.
            let ready = unsafe { poll(&mut fd, 1, POLL_INTERVAL.as_millis() as i32) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if ready == 0 {
                continue;
            }
            if fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                return Ok(Answer::Deny);
            }
            let mut bytes = [0u8; 256];
            match tty.read(&mut bytes) {
                Ok(0) => return Ok(Answer::Deny),
                Ok(count) => {
                    for &byte in &bytes[..count] {
                        if let Some(typed) = answer.push(byte) {
                            if interrupted() || cancel.load(Ordering::SeqCst) {
                                return Ok(Answer::Deny);
                            }
                            return Ok(typed);
                        }
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(error),
            }
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod tty {
    use super::*;
    pub(super) fn approve(
        _: &str,
        _: &str,
        _: chat::Review,
        _: &AtomicBool,
        _: &impl Fn() -> bool,
    ) -> io::Result<Answer> {
        Ok(Answer::Deny)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply() -> ChatReply {
        ChatReply {
            message: Message {
                role: "assistant".into(),
                content: "Done.\n".into(),
                attachments: vec![],
                reasoning_details: None,
                tool_trace: Some(serde_json::json!([
                    {"role":"assistant","content":null,"tool_calls":[{"id":"local-1","type":"function","function":{"name":"read_file","arguments":"{}"}}]},
                    {"role":"tool","tool_call_id":"local-1","content":"fixture"}
                ])),
            },
            truncated: false,
        }
    }

    #[test]
    fn tool_json_is_one_parseable_reply_with_trace() {
        let output = format_reply(&reply(), true, true).unwrap();
        let parsed: ChatReply = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed.message.content, "Done.\n");
        assert_eq!(
            parsed.message.tool_trace.unwrap()[1]["tool_call_id"],
            "local-1"
        );
        assert!(!parsed.truncated);
    }

    #[test]
    fn approvals_require_exact_words_and_bound_input() {
        for (text, expected) in [
            ("yes\n", Answer::Once),
            (" YES \n", Answer::Once),
            ("all\n", Answer::All),
            (" All \n", Answer::All),
            ("y\n", Answer::Deny),
            ("a\n", Answer::Deny),
            ("\n", Answer::Deny),
            ("yes please\n", Answer::Deny),
            ("all of it\n", Answer::Deny),
            ("yes\u{1b}[2J\n", Answer::Deny),
        ] {
            let mut answer = ApprovalAnswer::default();
            assert_eq!(
                text.bytes().find_map(|byte| answer.push(byte)),
                Some(expected),
                "{text:?}"
            );
        }
        let mut answer = ApprovalAnswer::default();
        for byte in b"yes"
            .iter()
            .copied()
            .chain(std::iter::repeat_n(b' ', 10_000))
        {
            assert_eq!(answer.push(byte), None);
        }
        assert_eq!(answer.push(b'\n'), Some(Answer::Deny));
        assert_eq!(answer.bytes.len(), 128);
        assert!(
            std::str::from_utf8(answer_prompt(chat::Review::Action))
                .unwrap()
                .contains("all to allow it and every later action")
        );
        assert!(
            std::str::from_utf8(answer_prompt(chat::Review::Continue))
                .unwrap()
                .contains("Enter stops")
        );
    }

    #[test]
    fn approval_controls_are_visible_and_terminal_output_is_sanitized() {
        assert_eq!(
            approval_text("run \u{1b}[2J\u{202e}abc\r\n"),
            "run \\u{1b}[2J\\u{202e}abc\\r\n"
        );
        let mut response = reply();
        response.message.content = "okay\u{1b}]52;c;ZXhhbXBsZQ==\u{7}".into();
        assert_eq!(format_reply(&response, false, true).unwrap(), "okay");
        assert_eq!(
            format_reply(&response, false, false).unwrap(),
            response.message.content
        );
    }

    #[test]
    fn unattended_approval_is_denied_and_worker_finishes() {
        let (events_tx, events_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let (approval_tx, approval_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            events_tx
                .send(chat::ToolEvent::Approval {
                    summary: "Write fixture".into(),
                    detail: "fixture only; no file is written".into(),
                    review: chat::Review::Action,
                    response: approval_tx,
                })
                .unwrap();
            assert!(!approval_rx.recv_timeout(Duration::from_secs(2)).unwrap());
            result_tx.send(Ok(reply())).unwrap();
        });
        let cancel = AtomicBool::new(false);
        let output = receive_result(
            &events_rx,
            &result_rx,
            &cancel,
            &chat::Policy::new(false),
            false,
            false,
            || false,
        )
        .unwrap();
        assert_eq!(output.message.content, "Done.\n");
        assert!(!cancel.load(Ordering::SeqCst));
        worker.join().unwrap();
    }

    #[test]
    fn interruption_denies_queued_approval_before_worker_cleanup() {
        let (events_tx, events_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let (approval_tx, approval_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            events_tx
                .send(chat::ToolEvent::Approval {
                    summary: "Must not execute".into(),
                    detail: String::new(),
                    review: chat::Review::Action,
                    response: approval_tx,
                })
                .unwrap();
            assert!(!approval_rx.recv_timeout(Duration::from_secs(2)).unwrap());
            result_tx.send(Err("Request canceled.".into())).unwrap();
        });
        let cancel = AtomicBool::new(false);
        let error = receive_result(
            &events_rx,
            &result_rx,
            &cancel,
            &chat::Policy::new(false),
            true,
            false,
            || true,
        )
        .unwrap_err();
        assert_eq!(error, "Request canceled.");
        assert!(cancel.load(Ordering::SeqCst));
        worker.join().unwrap();
    }
}
