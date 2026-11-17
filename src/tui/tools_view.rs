//! Local actions, review, and the visible execution ledger.
use super::*;
use crate::chat::ToolEvent;
use std::collections::VecDeque;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct ActionRecord {
    pub source: String,
    pub summary: String,
    pub output: String,
}
pub(super) struct ToolStream {
    pub source: String,
    pub chat_id: String,
    pub cancel: Arc<AtomicBool>,
    pub receiver: Receiver<ToolEvent>,
}
pub(super) struct Approval {
    source: String,
    summary: String,
    detail: String,
    response: mpsc::Sender<bool>,
    cancel: Arc<AtomicBool>,
}

fn review_text(text: &str) -> String {
    let mut output = String::new();
    for ch in text.chars() {
        if (ch.is_control() && ch != '\n')
            || matches!(ch, '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')
        {
            output.extend(ch.escape_default());
        } else {
            output.push(ch);
        }
    }
    output
}

pub(super) fn assistant(content: String) -> Message {
    let mut message = Message::user(content);
    message.role = "assistant".into();
    message
}

/// Whether a message is the output of a command the user ran with /run or /exec
/// (recorded as a "/run COMMAND" prompt), rather than a model reply.
pub(super) fn is_command_result(messages: &[Message], index: usize) -> bool {
    let Some(previous) = index.checked_sub(1).and_then(|i| messages.get(i)) else {
        return false;
    };
    let message = &messages[index];
    message.role == "assistant"
        && message.tool_trace.is_none()
        && message.reasoning_details.is_none()
        && previous.role == "user"
        && previous.content.starts_with("/run ")
}

impl App {
    pub(super) fn tool_roots(&self) -> Result<Vec<PathBuf>, String> {
        let mut roots = vec![self.working_dir.as_ref().map_err(Clone::clone)?.clone()];
        if let Ok(store) = &self.folders {
            for folder in &store.roots {
                if !roots.contains(&folder.path) {
                    roots.push(folder.path.clone());
                }
            }
        }
        Ok(roots)
    }
    pub(super) fn tool_channel(
        &mut self,
        source: String,
        cancel: Arc<AtomicBool>,
    ) -> mpsc::Sender<ToolEvent> {
        let (sender, receiver) = mpsc::channel();
        self.tool_streams.push(ToolStream {
            source,
            chat_id: self.chats[self.active].id.clone(),
            cancel,
            receiver,
        });
        sender
    }
    pub(super) fn tool_command(&mut self, name: &str, arg: &str) -> Result<(), String> {
        match name {
            "tools" => {
                self.args.tools = match arg {
                    "ask" | "on" => crate::ToolsSetting::Ask,
                    "auto" => crate::ToolsSetting::Auto,
                    "off" => crate::ToolsSetting::Off,
                    "" => self.args.tools,
                    _ => return Err("Use /tools ask, /tools auto, or /tools off.".into()),
                };
                let explanation = match self.args.tools {
                    crate::ToolsSetting::Ask => {
                        "File reads are available; changes and commands show a review first."
                    }
                    crate::ToolsSetting::Auto => {
                        "File changes and commands run without review for new requests in this session. Commands use your account permissions."
                    }
                    crate::ToolsSetting::Off => {
                        "Model file tools and commands are off. /run still runs commands you enter yourself."
                    }
                };
                let mut lines = vec![explanation.into(), String::new(), "Working folders:".into()];
                lines.extend(self.tool_roots()?.iter().map(|p| p.display().to_string()));
                lines.extend([
                    String::new(),
                    "/run COMMAND runs a command you enter directly.".into(),
                    "/actions shows completed actions and their output.".into(),
                    "/audit checks the decision log. It records approvals, declines, and runs, not file contents or command output.".into(),
                ]);
                self.panel(&format!("Local tools · {}", self.args.tools.label()), lines);
                self.status = format!(
                    "Tools: {} · applies to new requests.",
                    self.args.tools.label()
                );
            }
            "actions" => self.show_actions(),
            "audit" => {
                if !arg.is_empty() {
                    return Err("Use /audit with no arguments.".into());
                }
                self.show_audit();
            }
            "run" | "exec" => self.start_command(arg)?,
            _ => unreachable!(),
        }
        Ok(())
    }
    fn start_command(&mut self, command: &str) -> Result<(), String> {
        if command.trim().is_empty() {
            return Err(
                "Use /run COMMAND, for example /run cargo test or /run python3 script.py.".into(),
            );
        }
        if command.len() > 65_536 {
            return Err("Command exceeds 64 KiB.".into());
        }
        if self.pending.is_some() {
            return Err("Stop or finish the current request first.".into());
        }
        let roots = self.tool_roots()?;
        let cwd = roots[0].clone();
        let command = command.to_string();
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let events = self.tool_channel("Command".into(), cancel.clone());
        self.pending_actions.clear();
        self.pending_tools = true;
        self.pending_stopping = false;
        let chat = &mut self.chats[self.active];
        if chat.messages.is_empty() && chat.title == "New chat" {
            chat.title = format!("Run {}", command.chars().take(55).collect::<String>());
        }
        chat.messages.push(Message::user(format!("/run {command}")));
        self.sent.clear();
        self.input.clear();
        self.panel = None;
        self.transcript_cache = None;
        self.pending = Some(receiver);
        self.cancel = Some(cancel.clone());
        self.pending_started = Some(Instant::now());
        self.status = format!("Running {command} · Ctrl+C stops it.");
        // Redact the API key from output that is saved and sent with later turns.
        let key = crate::load_key(&self.args)
            .ok()
            .filter(|key| !key.is_empty());
        std::thread::spawn(move || {
            let result = (|| {
                let runtime = crate::local_tools::LocalTools::new(roots)?;
                let prepared = runtime.prepare(
                    "run_command",
                    &serde_json::json!({"command":command,"cwd":cwd,"timeout_seconds":180})
                        .to_string(),
                )?;
                let summary = prepared.summary.clone();
                let subject = prepared.audit_subject();
                let _ = events.send(ToolEvent::Progress(summary.clone()));
                let result = runtime.execute(prepared, cancel);
                let decision = if result.is_ok() { "ran" } else { "failed" };
                crate::audit::note(
                    "user",
                    decision,
                    "run_command",
                    &subject,
                    key.as_deref().unwrap_or(""),
                );
                let output = result
                    .as_ref()
                    .cloned()
                    .unwrap_or_else(|e| format!("Error: {e}"));
                let output = match &key {
                    Some(key) => output.replace(key.as_str(), "[REDACTED]"),
                    None => output,
                };
                let _ = events.send(ToolEvent::Completed {
                    summary,
                    output: output.clone(),
                });
                // A direct command's outcome is itself the answer, including
                // failures and cancellation; do not duplicate its audit below it.
                Ok(ChatReply {
                    message: assistant(output),
                    truncated: false,
                })
            })();
            let _ = sender.send(result);
        });
        Ok(())
    }
    pub(super) fn receive_tools(&mut self) -> bool {
        let mut changed = false;
        let mut received = Vec::new();
        self.tool_streams.retain(|stream| {
            loop {
                match stream.receiver.try_recv() {
                    Ok(event) => received.push((
                        stream.source.clone(),
                        stream.chat_id.clone(),
                        stream.cancel.clone(),
                        event,
                    )),
                    Err(mpsc::TryRecvError::Empty) => return true,
                    Err(mpsc::TryRecvError::Disconnected) => return false,
                }
            }
        });
        for (source, chat_id, cancel, event) in received {
            changed = true;
            match event {
                ToolEvent::Progress(summary) => {
                    if !cancel.load(Ordering::SeqCst) {
                        self.status = format!("{source} · {} · Ctrl+C stops", clean(&summary));
                    }
                }
                ToolEvent::Approval {
                    summary,
                    detail,
                    response,
                } => {
                    if cancel.load(Ordering::SeqCst) {
                        let _ = response.send(false);
                        continue;
                    }
                    self.clear_mouse_selection();
                    if self.selection_mode {
                        self.toggle_selection();
                    }
                    self.approvals.push_back(Approval {
                        source,
                        summary,
                        detail,
                        response,
                        cancel,
                    });
                    self.approval_painted = None;
                    self.approval_scroll = 0;
                    self.status =
                        "Review action · Y run once · N or Enter decline · Ctrl+C stops".into();
                }
                ToolEvent::Completed { summary, output } => {
                    if let Some(index) = self.chats.iter().position(|c| c.id == chat_id) {
                        let record = ActionRecord {
                            source,
                            summary: clean(&summary).chars().take(4096).collect(),
                            output: clean(&output).chars().take(16_384).collect(),
                        };
                        if self.pending.is_some()
                            && index == self.active
                            && matches!(record.source.as_str(), "Pitty" | "Command")
                        {
                            self.pending_actions.push(record.clone());
                        }
                        let log = &mut self.chats[index].actions;
                        // Keep the latest records, including newly completed effects.
                        if log.len() >= 256 {
                            log.remove(0);
                        }
                        log.push(record);
                        self.save_conversation(index);
                    }
                }
            }
        }
        while self
            .approvals
            .front()
            .is_some_and(|p| p.cancel.load(Ordering::SeqCst))
        {
            if let Some(p) = self.approvals.pop_front() {
                let _ = p.response.send(false);
            }
            self.approval_painted = None;
            self.approval_scroll = 0;
            changed = true;
        }
        changed
    }
    pub(super) fn handle_approval(&mut self, key: &Key) -> bool {
        if self.approvals.is_empty() || self.confirm_quit {
            return false;
        }
        if matches!(key, Key::Ctrl('c' | 'q') | Key::Eof) {
            return false;
        }
        let ready = self
            .approval_painted
            .is_some_and(|when| when.elapsed() >= Duration::from_millis(300));
        // Keys typed for the composer just before a review appears must not
        // answer it: Y, N, and Enter count only once the review has been shown.
        // Esc is a deliberate key and declines immediately.
        match key {
            Key::Char('y' | 'Y') if ready => self.answer_approval(true),
            Key::Enter | Key::Char('n' | 'N') if ready => self.answer_approval(false),
            Key::Escape => self.answer_approval(false),
            Key::Up => self.approval_scroll = self.approval_scroll.saturating_sub(1),
            Key::Down => self.approval_scroll = self.approval_scroll.saturating_add(1),
            Key::PageUp => self.approval_scroll = self.approval_scroll.saturating_sub(10),
            Key::PageDown => self.approval_scroll = self.approval_scroll.saturating_add(10),
            Key::Scroll { up, .. } => {
                self.approval_scroll = if *up {
                    self.approval_scroll.saturating_sub(3)
                } else {
                    self.approval_scroll.saturating_add(3)
                };
            }
            _ => {}
        }
        true
    }
    fn answer_approval(&mut self, allow: bool) {
        if let Some(approval) = self.approvals.pop_front() {
            let permitted = allow && !approval.cancel.load(Ordering::SeqCst);
            let _ = approval.response.send(permitted);
            self.status = if permitted {
                "Running approved action…"
            } else {
                "Action declined."
            }
            .into();
        }
        self.approval_painted = None;
        self.approval_scroll = 0;
    }
    pub(super) fn draw_approval(
        &mut self,
        frame: &mut Frame,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        colors: &Colors,
    ) {
        let Some(approval) = self.approvals.front() else {
            return;
        };
        if width < 24 || height < 6 {
            self.approval_painted = None;
            frame.text(x, y, "Resize to review this action.", colors.error, width);
            return;
        }
        frame.text(
            x,
            y,
            &format!(
                "Review · {} · {}",
                review_text(&approval.source),
                review_text(&approval.summary)
            ),
            colors.accent,
            width,
        );
        let text = format!(
            "{}\n\nY run once · N / Enter decline\n↑↓ / PgUp / PgDn scroll · Ctrl+C stops",
            review_text(&approval.detail)
        );
        let lines = text
            .lines()
            .flat_map(|line| wrap(line, width))
            .collect::<Vec<_>>();
        let visible = height.saturating_sub(2);
        self.approval_scroll = self
            .approval_scroll
            .min(lines.len().saturating_sub(visible));
        for (row, line) in lines
            .iter()
            .skip(self.approval_scroll)
            .take(visible)
            .enumerate()
        {
            let style = if line.starts_with('+') {
                Style::new(Some((73, 149, 99)), colors.base.bg, false)
            } else if line.starts_with('-') {
                colors.error
            } else {
                colors.base
            };
            frame.text(x, y + row + 2, line, style, width);
        }
        self.approval_painted.get_or_insert_with(Instant::now);
    }
    fn show_audit(&mut self) {
        self.panel("Decision log", crate::audit::explain());
    }
    fn show_actions(&mut self) {
        let mut lines = Vec::new();
        for (index, action) in self.chats[self.active].actions.iter().enumerate() {
            lines.push(format!(
                "{} · {} · {}",
                index + 1,
                action.source,
                action.summary
            ));
            lines.extend(action.output.lines().map(str::to_string));
            lines.push(String::new());
        }
        if lines.is_empty() {
            lines.push("No local actions in this conversation yet.".into());
        }
        self.panel("Local actions · completed results", lines);
    }
    pub(super) fn finish_interrupted_tools(&mut self, reason: &str) {
        let chat = &mut self.chats[self.active];
        if chat.messages.last().is_some_and(|m| m.role == "user") {
            let mut content = format!("{}\n", clean(reason));
            let actions = &self.pending_actions;
            if !actions.is_empty() {
                content.push_str("\nActions recorded during this request:\n");
                for action in actions {
                    content.push_str(&format!("\n{}\n{}\n", action.summary, action.output));
                }
                content.push_str("\nReview these results before continuing.");
            }
            chat.messages.push(assistant(content));
        }
        // A retry must never silently repeat an already approved mutation.
        self.sent.clear();
        self.save();
    }
}

pub(super) fn approvals() -> VecDeque<Approval> {
    VecDeque::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    fn app() -> App {
        App::new(Args::try_parse_from(["pitty", "--no-history"]).unwrap())
    }
    #[test]
    fn review_preserves_draft_and_requires_a_visible_explicit_yes() {
        let mut app = app();
        app.input.set("keep my draft");
        let cancel = Arc::new(AtomicBool::new(false));
        let events = app.tool_channel("Pitty".into(), cancel);
        let (response, result) = mpsc::channel();
        events
            .send(ToolEvent::Approval {
                summary: "Edit hello.rs".into(),
                detail: "Replace one line".into(),
                response,
            })
            .unwrap();
        app.receive_tools();
        app.handle(Key::Char('y'));
        assert!(
            result.try_recv().is_err(),
            "unpainted approval must not run"
        );
        app.draw(90, 30);
        app.handle(Key::Paste("y\n".into()));
        assert!(result.try_recv().is_err(), "pasted input must not approve");
        app.approval_painted = Some(Instant::now() - Duration::from_secs(1));
        app.handle(Key::Char('y'));
        assert!(result.recv().unwrap());
        assert_eq!(app.input.value(), "keep my draft");
    }
    #[test]
    fn tiny_or_resized_windows_cannot_approve_an_unseen_action() {
        let mut app = app();
        let cancel = Arc::new(AtomicBool::new(false));
        let events = app.tool_channel("Pitty".into(), cancel);
        let (response, result) = mpsc::channel();
        events
            .send(ToolEvent::Approval {
                summary: "Write fixture.rs".into(),
                detail: "A complete operation to review".into(),
                response,
            })
            .unwrap();
        app.receive_tools();
        for (width, height) in [(80, 8), (20, 24), (19, 8), (80, 7)] {
            // A previously visible review cannot remain armed after shrinking.
            app.draw(90, 30);
            app.approval_painted = Some(Instant::now() - Duration::from_secs(1));
            app.draw(width, height);
            assert!(app.approval_painted.is_none());
            app.handle(Key::Char('y'));
            assert!(
                result.try_recv().is_err(),
                "{width}x{height} must not approve"
            );
        }
        app.draw(90, 30);
        assert!(app.approval_painted.is_some());
        app.handle(Key::Char('y'));
        assert!(
            result.try_recv().is_err(),
            "resizing back requires a fresh paint interval"
        );
        app.approval_painted = Some(Instant::now() - Duration::from_secs(1));
        app.handle(Key::Char('y'));
        assert!(result.recv().unwrap());
    }

    #[test]
    fn canceled_review_and_enter_never_execute() {
        for canceled in [false, true] {
            let mut app = app();
            let cancel = Arc::new(AtomicBool::new(false));
            let events = app.tool_channel("Pitty".into(), cancel.clone());
            let (response, result) = mpsc::channel();
            events
                .send(ToolEvent::Approval {
                    summary: "Run command".into(),
                    detail: "pwd".into(),
                    response,
                })
                .unwrap();
            app.receive_tools();
            if canceled {
                cancel.store(true, Ordering::SeqCst);
                app.receive_tools();
            } else {
                // An Enter typed before the review is visible answers nothing.
                app.handle(Key::Enter);
                assert!(result.try_recv().is_err(), "unseen review was answered");
                app.approval_painted = Some(Instant::now() - Duration::from_millis(400));
                app.handle(Key::Enter);
            }
            assert!(!result.recv().unwrap());
        }
    }
    #[test]
    fn interrupted_tool_turn_keeps_actions_and_disables_blind_retry() {
        let mut app = app();
        app.chats[0]
            .messages
            .push(Message::user("Change a file".into()));
        app.chats[0].actions.push(ActionRecord {
            source: "Pitty".into(),
            summary: "Edited hello.rs".into(),
            output: "One replacement".into(),
        });
        app.pending_actions = app.chats[0].actions.clone();
        app.sent = "Change a file".into();
        app.finish_interrupted_tools("Request canceled.");
        assert_eq!(app.chats[0].messages.len(), 2);
        assert!(app.chats[0].messages[1].content.contains("Edited hello.rs"));
        assert!(app.command("retry", "").is_err());
    }
    #[test]
    fn direct_command_runs_without_model_credentials() {
        let mut app = app();
        app.folders = Err("No connected folders for test".into());
        app.command("run", "printf pitty-local-test").unwrap();
        let start = Instant::now();
        while app.pending.is_some() && start.elapsed() < Duration::from_secs(5) {
            app.receive();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(app.pending.is_none());
        assert!(
            app.chats[app.active]
                .messages
                .last()
                .unwrap()
                .content
                .contains("pitty-local-test")
        );
        assert_eq!(app.chats[app.active].actions.len(), 1);
    }
}
