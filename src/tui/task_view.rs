use super::*;
use crate::agents::JobState;

pub(super) fn elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60)
    }
}
impl App {
    pub(super) fn toggle_selection(&mut self) {
        self.clear_mouse_selection();
        self.selection_mode = !self.selection_mode;
        self.selection_painted = false;
        if self.selection_mode {
            self.selection_previous_mouse = self.args.no_mouse;
            self.args.no_mouse = true;
            self.status = "Select text with the mouse; use your terminal's Copy shortcut. F2 or Esc returns to chat.".into();
        } else {
            self.args.no_mouse = self.selection_previous_mouse;
            self.status = "Ready · Ctrl+Y copies the latest reply · F2 selects text.".into();
        }
    }
    pub(super) fn task_command(&mut self, command: &str, arg: &str) -> Result<(), String> {
        if command == "agents" {
            self.agents_view = true;
            self.update_agents_panel();
            return Ok(());
        }
        let (name, task) = arg
            .split_once(char::is_whitespace)
            .ok_or("Use /agent NAME TASK, /agent open ID, /agent stop ID, or /agents.")?;
        let task = task.trim();
        if name == "stop" {
            if task == "all" {
                self.agents.cancel_all();
            } else {
                self.agents.cancel(
                    task.parse()
                        .map_err(|_| "Use /agent stop ID or /agent stop all.")?,
                )?;
            }
            self.status =
                "Agent cancellation requested. Previously finished results remain available."
                    .into();
            return Ok(());
        }
        if name == "open" {
            let id: usize = task
                .parse()
                .map_err(|_| "Use /agent open ID from /agents.")?;
            let job = self
                .agents
                .jobs
                .iter()
                .find(|j| j.id == id)
                .ok_or("Unknown agent ID. Use /agents.")?;
            let chat_id = job
                .conversation
                .as_ref()
                .ok_or("This agent has no completed result yet.")?;
            let index = self
                .chats
                .iter()
                .position(|c| &c.id == chat_id)
                .ok_or("This result was deleted. Check /chats or your saved history.")?;
            self.command("open", &(index + 1).to_string())?;
            self.agents_view = false;
            return Ok(());
        }
        if self.pending.is_some() {
            return Err("Finish or stop the current main reply before starting another task from this conversation.".into());
        }
        if !self.attachments.is_empty() {
            return Err("Send or detach staged files first. Agents inherit files already in the selected conversation.".into());
        }
        let mut args = self.args.clone();
        let task = if let Some(rest) = task.strip_prefix("--model ") {
            let (model, prompt) = rest
                .trim_start()
                .split_once(char::is_whitespace)
                .ok_or("Use /agent NAME --model provider/model TASK.")?;
            args.model = model.to_string();
            prompt.trim()
        } else {
            task
        };
        let messages = self.chats[self.active].messages.clone();
        let roots = self.tool_roots()?;
        let (events, receiver) = mpsc::channel();
        let id = self.agents.start_with_tools(
            name.to_string(),
            args,
            messages,
            task.to_string(),
            crate::agents::ToolAccess {
                roots,
                events,
                policy: self.policy.clone(),
            },
        )?;
        if let Some(cancel) = self.agents.cancellation(id) {
            self.tool_streams.push(tools_view::ToolStream {
                source: format!("Agent {id} · {name}"),
                chat_id: self.chats[self.active].id.clone(),
                cancel,
                receiver,
            });
        }
        self.agents_view = true;
        self.update_agents_panel();
        self.status = format!(
            "Agent {id} started · /agents shows progress · /agent stop {id} cancels. Each agent uses its own API request."
        );
        Ok(())
    }
    fn update_agents_panel(&mut self) {
        self.agents_panel_tick = self.tick / 4;
        let mut lines = vec![
            "/agent NAME TASK starts a task · /agent open ID shows its result".into(),
            "/agent stop ID cancels one · /agent stop all cancels every running agent".into(),
            format!(
                "Agents inherit this conversation and local tools mode: {}.",
                self.args.tools.label()
            ),
            String::new(),
        ];
        for job in &self.agents.jobs {
            let state = match &job.state {
                JobState::Running => "Thinking".to_string(),
                JobState::Completed => "Complete".to_string(),
                JobState::Failed(error) => {
                    format!("Failed: {}", error.chars().take(180).collect::<String>())
                }
                JobState::Canceled => "Canceled".to_string(),
            };
            lines.push(format!(
                "{}  {} · {} · {}",
                job.id,
                job.name,
                state,
                elapsed(job.elapsed())
            ));
            lines.push(format!(
                "   {} · {}{}",
                job.model,
                job.task.chars().take(180).collect::<String>(),
                if job.task.chars().nth(180).is_some() {
                    "…"
                } else {
                    ""
                }
            ));
            if job.conversation.is_some() {
                lines.push(format!("   /agent open {}", job.id));
            }
            lines.push(String::new());
        }
        if self.agents.jobs.is_empty() {
            lines.push(
                "No agent tasks yet. Example: /agent planner Break this problem into steps.".into(),
            );
        }
        self.panel = Some((
            format!("Agents · {}/4 worker slots occupied", self.agents.running()),
            lines,
        ));
    }
    pub(super) fn receive_agents(&mut self) -> bool {
        let occupied = self.agents.running();
        let results = self.agents.poll();
        let changed = !results.is_empty() || occupied != self.agents.running();
        for completion in results {
            match completion.result {
                Ok(reply) => {
                    let mut conversation = Conversation::new(&completion.model);
                    conversation.title = format!(
                        "Agent {} · {}",
                        completion.name,
                        completion.task.chars().take(40).collect::<String>()
                    );
                    conversation.agent_name = Some(completion.name.clone());
                    let source = format!("Agent {} · {}", completion.id, completion.name);
                    conversation.actions = self
                        .chats
                        .iter()
                        .flat_map(|chat| &chat.actions)
                        .filter(|action| action.source == source)
                        .take(256)
                        .cloned()
                        .collect();
                    conversation.messages = completion.messages;
                    conversation.messages.push(reply.message);
                    if let Some(job) = self.agents.jobs.iter_mut().find(|j| j.id == completion.id) {
                        job.conversation = Some(conversation.id.clone());
                    }
                    self.chats.push(conversation);
                    let saved = self.save_conversation(self.chats.len() - 1);
                    if self.pending.is_none() && saved {
                        self.status = format!(
                            "Agent {} finished{} in {} · /agent open {}",
                            completion.name,
                            if reply.truncated {
                                " (token limit reached)"
                            } else {
                                ""
                            },
                            elapsed(completion.elapsed),
                            completion.id
                        );
                    }
                }
                Err(error) => {
                    if self.pending.is_none() {
                        self.status = format!("Agent {} failed: {}", completion.name, error);
                    }
                }
            }
        }
        if self.agents_view && (changed || self.agents_panel_tick != self.tick / 4) {
            self.update_agents_panel();
        }
        changed
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn selection_preserves_draft_and_restores_mouse_preference() {
        for no_mouse in [false, true] {
            let mut app = App::new(Args::try_parse_from(["pitty", "--no-history"]).unwrap());
            app.args.no_mouse = no_mouse;
            app.input.set("keep this draft");
            app.handle(Key::F2);
            assert!(app.selection_mode && app.args.no_mouse);
            app.handle(Key::Enter);
            assert!(!app.selection_mode);
            assert_eq!(app.args.no_mouse, no_mouse);
            assert_eq!(app.input.value(), "keep this draft");
            assert!(app.pending.is_none());
        }
    }
    #[test]
    fn duration_labels_and_working_folder_are_visible() {
        assert_eq!(elapsed(Duration::from_secs(65)), "1m 05s");
        assert_eq!(elapsed(Duration::from_secs(3660)), "1h 01m");
        let mut app = App::new(Args::try_parse_from(["pitty", "--no-history"]).unwrap());
        app.working_dir = Ok(PathBuf::from("/Volumes/Shared/Project"));
        assert!(
            app.draw(100, 35)
                .plain_text()
                .contains("/Volumes/Shared/Project")
        );
        app.command("pwd", "").unwrap();
        assert!(app.panel.as_ref().unwrap().1[0].contains("/Volumes/Shared/Project"));
    }
}
