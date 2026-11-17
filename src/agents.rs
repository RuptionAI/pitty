//! Explicit, bounded parallel model requests. Workers receive only the supplied
//! conversation and task, plus explicitly configured local tools.
use crate::{Args, chat};
use pitty_core::{ChatReply, ChatRequest, Message};
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub const MAX_RUNNING: usize = 4;
pub const MAX_RECORDS: usize = 32;
const MAX_TASK_BYTES: usize = 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum JobState {
    Running,
    Completed,
    Failed(String),
    Canceled,
}

pub struct Job {
    pub id: usize,
    pub name: String,
    pub model: String,
    pub task: String,
    pub state: JobState,
    pub conversation: Option<String>,
    pub started: Instant,
    pub finished: Option<Duration>,
}
impl Job {
    pub fn elapsed(&self) -> Duration {
        self.finished.unwrap_or_else(|| self.started.elapsed())
    }
}

pub struct Completion {
    pub id: usize,
    pub name: String,
    pub model: String,
    pub task: String,
    pub result: Result<ChatReply, String>,
    pub messages: Vec<Message>,
    pub elapsed: Duration,
}

struct Outcome {
    result: Result<ChatReply, String>,
    messages: Vec<Message>,
    elapsed: Duration,
}
struct Worker {
    id: usize,
    cancel: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Outcome>,
}

pub struct Manager {
    pub jobs: Vec<Job>,
    workers: Vec<Worker>,
    next_id: usize,
}
impl Default for Manager {
    fn default() -> Self {
        Self {
            jobs: Vec::new(),
            workers: Vec::new(),
            next_id: 1,
        }
    }
}
impl Manager {
    pub fn start_with_tools(
        &mut self,
        name: String,
        args: Args,
        messages: Vec<Message>,
        task: String,
        roots: Vec<std::path::PathBuf>,
        events: mpsc::Sender<chat::ToolEvent>,
    ) -> Result<usize, String> {
        self.start_with(name, args, messages, task, move |args, payload, cancel| {
            if let Some(mode) = args.tools.mode() {
                let runtime = crate::local_tools::LocalTools::new(roots)?;
                chat::request_with_tools(&args, payload, cancel, runtime, events, mode)
            } else {
                chat::request(&args, payload, cancel)
            }
        })
    }

    pub fn cancellation(&self, id: usize) -> Option<Arc<AtomicBool>> {
        self.workers
            .iter()
            .find(|worker| worker.id == id)
            .map(|worker| worker.cancel.clone())
    }

    fn start_with<F>(
        &mut self,
        name: String,
        args: Args,
        mut messages: Vec<Message>,
        task: String,
        work: F,
    ) -> Result<usize, String>
    where
        F: FnOnce(Args, &ChatRequest, Arc<AtomicBool>) -> Result<ChatReply, String>
            + Send
            + 'static,
    {
        if self.workers.len() >= MAX_RUNNING {
            return Err("Four agent workers are active. Wait for a worker to finish before starting another.".into());
        }
        if name.trim().is_empty() || name.chars().count() > 40 || name.chars().any(char::is_control)
        {
            return Err("Use an agent name of 1–40 characters without control characters.".into());
        }
        if task.trim().is_empty() || task.len() > MAX_TASK_BYTES {
            return Err("Provide an agent task of 1 byte to 1 MiB.".into());
        }
        if messages
            .last()
            .is_some_and(|message| message.role == "user")
        {
            return Err("The current conversation has an unanswered message. Finish or cancel it before starting an agent.".into());
        }
        messages.push(Message::user(task.clone()));
        let payload = ChatRequest { messages };
        payload.validate().map_err(str::to_string)?;
        within_context_limit(&payload, crate::attachments::chat_limit()?)?;
        let id = self.next_id;
        let next_id = id
            .checked_add(1)
            .ok_or("Agent ID limit reached; restart Pitty.")?;
        let name = name.trim().to_string();
        let model = args.model.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (sender, receiver) = mpsc::channel();
        let started = Instant::now();
        std::thread::Builder::new()
            .name(format!("pitty-agent-{id}"))
            .spawn(move || {
                let result = work(args, &payload, worker_cancel);
                let _ = sender.send(Outcome {
                    result,
                    messages: payload.messages,
                    elapsed: started.elapsed(),
                });
            })
            .map_err(|error| format!("Cannot start agent worker: {error}"))?;
        self.next_id = next_id;
        // Only finished records can be evicted. Canceled but still stopping
        // workers retain their record and concurrency slot until poll reaps them.
        if self.jobs.len() >= MAX_RECORDS
            && let Some(index) = self
                .jobs
                .iter()
                .position(|job| !self.workers.iter().any(|worker| worker.id == job.id))
        {
            self.jobs.remove(index);
        }
        self.jobs.push(Job {
            id,
            name,
            model,
            task,
            state: JobState::Running,
            conversation: None,
            started,
            finished: None,
        });
        self.workers.push(Worker {
            id,
            cancel,
            receiver,
        });
        Ok(id)
    }

    /// Number of occupied worker slots, including canceled requests still stopping.
    pub fn running(&self) -> usize {
        self.workers.len()
    }

    pub fn cancel(&mut self, id: usize) -> Result<(), String> {
        let job = self
            .jobs
            .iter_mut()
            .find(|job| job.id == id)
            .ok_or("Unknown agent ID. Use /agents to list jobs.")?;
        if job.state == JobState::Canceled {
            return Ok(());
        }
        let worker = self
            .workers
            .iter()
            .find(|worker| worker.id == id)
            .ok_or("This agent has already finished.")?;
        worker.cancel.store(true, Ordering::SeqCst);
        job.state = JobState::Canceled;
        job.finished = Some(job.started.elapsed());
        Ok(())
    }

    pub fn cancel_all(&mut self) {
        for worker in &self.workers {
            worker.cancel.store(true, Ordering::SeqCst);
            if let Some(job) = self.jobs.iter_mut().find(|job| job.id == worker.id)
                && job.state == JobState::Running
            {
                job.state = JobState::Canceled;
                job.finished = Some(job.started.elapsed());
            }
        }
    }

    /// Deliver each completed result once. Late replies from canceled jobs are
    /// discarded, so closing or canceling cannot create a conversation afterward.
    pub fn poll(&mut self) -> Vec<Completion> {
        let mut completions = Vec::new();
        let mut index = 0;
        while index < self.workers.len() {
            let outcome = match self.workers[index].receiver.try_recv() {
                Ok(outcome) => Some(outcome),
                Err(mpsc::TryRecvError::Empty) => {
                    index += 1;
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => None,
            };
            let worker = self.workers.swap_remove(index);
            let Some(job) = self.jobs.iter_mut().find(|job| job.id == worker.id) else {
                continue;
            };
            if job.state == JobState::Canceled {
                continue;
            }
            let outcome = outcome.unwrap_or_else(|| Outcome {
                result: Err("Agent worker stopped unexpectedly.".into()),
                messages: Vec::new(),
                elapsed: job.started.elapsed(),
            });
            job.finished = Some(outcome.elapsed);
            job.state = match &outcome.result {
                Ok(_) => JobState::Completed,
                Err(error) => JobState::Failed(error.clone()),
            };
            completions.push(Completion {
                id: job.id,
                name: job.name.clone(),
                model: job.model.clone(),
                task: job.task.clone(),
                result: outcome.result,
                messages: outcome.messages,
                elapsed: outcome.elapsed,
            });
        }
        completions
    }
}
impl Drop for Manager {
    fn drop(&mut self) {
        self.cancel_all();
    }
}

fn within_context_limit(payload: &ChatRequest, limit: usize) -> Result<(), String> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.bytes = self.bytes.saturating_add(bytes.len());
            if self.bytes > self.limit {
                return Err(io::Error::other("conversation size limit reached"));
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(&mut Counter { bytes: 0, limit }, payload).map_err(|_| {
        "Agent context is too large. Start a new conversation or reduce its attachments.".into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    fn args() -> Args {
        Args::try_parse_from(["pitty", "--max-tokens", "123"]).unwrap()
    }
    fn reply(text: &str) -> ChatReply {
        let mut message = Message::user(text.into());
        message.role = "assistant".into();
        ChatReply {
            message,
            truncated: false,
        }
    }
    fn collect(manager: &mut Manager, running: usize) -> Vec<Completion> {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut completed = Vec::new();
        loop {
            completed.extend(manager.poll());
            if manager.running() == running {
                return completed;
            }
            assert!(Instant::now() < deadline, "worker did not finish");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    #[test]
    fn independent_context_result_and_elapsed_are_delivered_once() {
        let mut manager = Manager::default();
        let mut context = vec![
            Message::user("Original question".into()),
            reply("Original answer").message,
        ];
        context[0].attachments.push(pitty_core::Attachment {
            name: "context.txt".into(),
            kind: "text".into(),
            mime: "text/plain".into(),
            data: "Original requirements".into(),
        });
        context[1].reasoning_details =
            Some(serde_json::json!([{"type":"reasoning.text", "text":"preserved"}]));
        let (tx, rx) = mpsc::channel();
        let id = manager
            .start_with(
                "research".into(),
                args(),
                context.clone(),
                "Compare options".into(),
                move |args, payload, _| {
                    assert_eq!(args.max_tokens, 123);
                    tx.send(payload.clone()).unwrap();
                    Ok(reply("Independent result"))
                },
            )
            .unwrap();
        let payload = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(payload.messages.len(), 3);
        assert_eq!(payload.messages[2].content, "Compare options");
        assert_eq!(context.len(), 2);
        let results = collect(&mut manager, 0);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, id);
        assert_eq!(results[0].name, "research");
        assert_eq!(results[0].model, args().model);
        assert_eq!(results[0].task, "Compare options");
        assert_eq!(results[0].messages.len(), 3);
        assert_eq!(
            results[0].messages[0].attachments[0].data,
            "Original requirements"
        );
        assert_eq!(
            results[0].messages[1].reasoning_details,
            context[1].reasoning_details
        );
        assert_eq!(results[0].messages[2].content, "Compare options");
        assert_eq!(
            results[0].result.as_ref().unwrap().message.content,
            "Independent result"
        );
        assert_eq!(manager.jobs[0].state, JobState::Completed);
        assert_eq!(manager.jobs[0].elapsed(), results[0].elapsed);
        manager.jobs[0].conversation = Some("saved-chat-id".into());
        assert_eq!(
            manager.jobs[0].conversation.as_deref(),
            Some("saved-chat-id")
        );
        assert!(manager.poll().is_empty());
    }
    #[test]
    fn cancellation_retains_capacity_until_worker_stops_and_discards_late_reply() {
        let mut manager = Manager::default();
        let (entered_tx, entered_rx) = mpsc::channel();
        let mut release = Vec::new();
        for index in 0..MAX_RUNNING {
            let (tx, rx) = mpsc::channel();
            let entered = entered_tx.clone();
            manager
                .start_with(
                    format!("job-{index}"),
                    args(),
                    vec![],
                    "task".into(),
                    move |_, _, _| {
                        entered.send(()).unwrap();
                        rx.recv_timeout(Duration::from_secs(3)).unwrap();
                        Ok(reply("late result"))
                    },
                )
                .unwrap();
            release.push(tx);
        }
        for _ in 0..MAX_RUNNING {
            entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        }
        assert_eq!(manager.running(), MAX_RUNNING);
        manager.cancel(1).unwrap();
        let canceled_time = manager.jobs[0].elapsed();
        assert!(
            manager
                .start_with(
                    "extra".into(),
                    args(),
                    vec![],
                    "task".into(),
                    |_, _, _| panic!("must not spawn")
                )
                .is_err()
        );
        release.remove(0).send(()).unwrap();
        assert!(collect(&mut manager, MAX_RUNNING - 1).is_empty());
        assert_eq!(manager.jobs[0].state, JobState::Canceled);
        assert_eq!(manager.jobs[0].elapsed(), canceled_time);
        manager.cancel_all();
        for sender in release {
            sender.send(()).unwrap();
        }
        assert!(collect(&mut manager, 0).is_empty());
        assert!(
            manager
                .jobs
                .iter()
                .all(|job| job.state == JobState::Canceled)
        );
        manager
            .start_with(
                "replacement".into(),
                args(),
                vec![],
                "task".into(),
                |_, _, _| Ok(reply("replacement")),
            )
            .unwrap();
        assert_eq!(collect(&mut manager, 0).len(), 1);
    }
    #[test]
    fn invalid_context_names_tasks_and_limits_do_not_spawn() {
        let mut manager = Manager::default();
        for name in ["", "line\nfeed", &"x".repeat(41)] {
            assert!(
                manager
                    .start_with(
                        name.into(),
                        args(),
                        vec![],
                        "task".into(),
                        |_, _, _| panic!("must not spawn")
                    )
                    .is_err()
            );
        }
        for task in ["".into(), " ".into(), "x".repeat(MAX_TASK_BYTES + 1)] {
            assert!(
                manager
                    .start_with("name".into(), args(), vec![], task, |_, _, _| panic!(
                        "must not spawn"
                    ))
                    .is_err()
            );
        }
        assert!(
            manager
                .start_with(
                    "name".into(),
                    args(),
                    vec![Message::user("pending".into())],
                    "task".into(),
                    |_, _, _| panic!("must not spawn")
                )
                .is_err()
        );
        let payload = ChatRequest {
            messages: vec![Message::user("some text".into())],
        };
        assert!(within_context_limit(&payload, 4).is_err());
        assert!(within_context_limit(&payload, 1000).is_ok());
        assert_eq!(manager.running(), 0);
        assert!(manager.jobs.is_empty());
    }
    #[test]
    fn records_are_bounded_ids_stable_and_failures_delivered() {
        let mut manager = Manager::default();
        for index in 0..MAX_RECORDS + 3 {
            let id = manager
                .start_with("critic".into(), args(), vec![], "task".into(), |_, _, _| {
                    Err("fixture failure".into())
                })
                .unwrap();
            assert_eq!(id, index + 1);
            let results = collect(&mut manager, 0);
            assert_eq!(results[0].result.as_ref().unwrap_err(), "fixture failure");
            assert_eq!(
                manager.jobs.last().unwrap().state,
                JobState::Failed("fixture failure".into())
            );
            assert!(manager.jobs.len() <= MAX_RECORDS);
        }
        assert_eq!(manager.jobs.len(), MAX_RECORDS);
        assert_eq!(manager.jobs[0].id, 4);
        assert!(manager.cancel(1).is_err());
        assert!(manager.cancel(MAX_RECORDS + 3).is_err());
    }
    #[test]
    fn drop_signals_running_workers_without_waiting_for_them() {
        let (tx, rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut manager = Manager::default();
        manager
            .start_with(
                "drafting".into(),
                args(),
                vec![],
                "task".into(),
                move |_, _, cancel| {
                    tx.send(cancel).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
                    Ok(reply("discarded"))
                },
            )
            .unwrap();
        let cancel = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(!cancel.load(Ordering::SeqCst));
        drop(manager);
        assert!(cancel.load(Ordering::SeqCst));
        release_tx.send(()).unwrap();
    }
}
