//! Local stdio MCP adapter. It never opens a network listener or loads API keys.
//! Protocol: https://modelcontextprotocol.io/specification/2025-11-25
use crate::{Args, ToolsSetting, local_tools::LocalTools};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread::JoinHandle,
    time::Duration,
};

const CURRENT_PROTOCOL: &str = "2025-11-25";
const PROTOCOLS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const INPUT_LIMIT: usize = 8 * 1024 * 1024;
const MAX_WORKERS: usize = 4;
const MAX_BATCH: usize = 32;
const POLL_INTERVAL: Duration = Duration::from_millis(40);
const SENSITIVE_READ: &str = "Declined: this path may contain credentials. Pitty never reads credential-like files over MCP, in any --tools mode, because it cannot ask its user for approval here.";

pub fn run(args: &Args) -> Result<(), String> {
    if args.directory.is_none() {
        return Err(
            "MCP requires an explicit working folder: pitty --mcp --directory /path/to/project"
                .into(),
        );
    }
    let cwd = std::env::current_dir().map_err(|e| format!("Cannot read working folder: {e}"))?;
    let tools = LocalTools::new(vec![cwd])?;
    let stdout = output::Transport::stdout()?;
    // SIGHUP matters when the server was started from a terminal: closing it
    // must stop and reap local commands rather than orphan them.
    let _handlers = crate::signals::install(&[libc::SIGINT, libc::SIGTERM, libc::SIGHUP])?;
    serve(Server::new(tools, args.tools), io::stdin(), stdout, || {
        crate::signals::received().is_some()
    })
}

/// Serves until input ends and every reply is delivered, `stopped` reports a
/// signal, or the client stops reading. Signals and a closed or broken stdout
/// end quietly; a client that closes only stdin still gets every reply.
fn serve(
    mut server: Server,
    input: impl Read + Send + 'static,
    mut output: output::Transport,
    stopped: impl Fn() -> bool,
) -> Result<(), String> {
    // The bounded queue applies backpressure to clients that write faster
    // than the server can parse. Individual frames are bounded before decode.
    let (input_tx, input_rx) = mpsc::sync_channel(1);
    let reader = std::thread::Builder::new()
        .name("pitty-mcp-input".into())
        .spawn(move || {
            let mut input = io::BufReader::new(input);
            loop {
                let next = read_frame(&mut input);
                let stop = !matches!(&next, Ok(Some(_)));
                if input_tx.send(next).is_err() || stop {
                    return;
                }
            }
        })
        .map_err(|e| format!("Cannot read MCP input: {e}"))?;
    let mut reading = true;
    let result = (|| {
        loop {
            if stopped() || output.disconnected() {
                return Ok(());
            }
            for response in server.poll() {
                output.send(&response, &stopped)?;
            }
            if !reading {
                // Frames arrive in order, so EOF means every earlier message
                // was handled. Deliver the replies of operations still running
                // (each bounded by its own limits), then exit.
                if server.workers.is_empty() {
                    return Ok(());
                }
                std::thread::sleep(POLL_INTERVAL);
                continue;
            }
            match input_rx.recv_timeout(POLL_INTERVAL) {
                Ok(Ok(Some(frame))) => {
                    let responses = match serde_json::from_slice::<Value>(&frame) {
                        Ok(message) => server.handle(message),
                        Err(_) => vec![rpc_error(Value::Null, -32700, "Invalid JSON.")],
                    };
                    for response in responses {
                        output.send(&response, &stopped)?;
                    }
                }
                Ok(Ok(None)) | Err(RecvTimeoutError::Disconnected) => reading = false,
                Ok(Err(error)) => {
                    let response = rpc_error(Value::Null, -32600, &error.to_string());
                    output.send(&response, &stopped)?;
                    return Err(format!("MCP input closed: {error}"));
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    })();
    // Stop all active operations together, then wait for process groups to be
    // reaped before restoring signal handling or returning to the caller.
    server.stop();
    drop(input_rx);
    if !reading || reader.is_finished() {
        let _ = reader.join();
    }
    // A reader blocked on an open stdin may remain only until process exit
    // after a signal or a vanished client. It owns no commands or tools.
    if stopped() || output.vanished() {
        Ok(())
    } else {
        result
    }
}

#[cfg(test)]
fn send(output: &mut impl Write, value: &Value) -> Result<(), String> {
    serde_json::to_writer(&mut *output, value)
        .map_err(|e| format!("Cannot write MCP response: {e}"))?;
    output
        .write_all(b"\n")
        .and_then(|()| output.flush())
        .map_err(|e| format!("Cannot write MCP response: {e}"))
}

// A host that stops consuming stdout must not strand active subprocesses, so
// every reply has a two-second deadline. Writes block on a dedicated thread
// while the protocol loop enforces that deadline. Making the descriptor
// nonblocking instead would change its shared open file description: when
// stdin and stdout are one terminal or socket, as in a manual launch from a
// shell, stdin reads would then fail with EAGAIN.
mod output {
    use super::*;
    use std::{
        fs::File,
        sync::mpsc::{Receiver, SyncSender},
        time::Instant,
    };
    const STALL_LIMIT: Duration = Duration::from_secs(2);
    pub(super) struct Transport {
        file: Arc<File>,
        frames: Option<SyncSender<Vec<u8>>>,
        written: Receiver<io::Result<()>>,
        writer: Option<JoinHandle<()>>,
        busy: bool,
        vanished: bool,
    }
    impl Transport {
        /// Writes through a duplicate of fd 1, so Rust's global stdout lock is
        /// never held by a blocked writer.
        #[cfg(unix)]
        pub(super) fn stdout() -> Result<Self, String> {
            use std::os::fd::BorrowedFd;
            // SAFETY: stdout fd 1 remains owned by the process. Cloning
            // obtains a separately owned descriptor, without closing fd 1.
            let fd = unsafe { BorrowedFd::borrow_raw(1) }
                .try_clone_to_owned()
                .map_err(|e| format!("Cannot open MCP stdout: {e}"))?;
            Self::new(File::from(fd))
        }
        #[cfg(not(unix))]
        pub(super) fn stdout() -> Result<Self, String> {
            Err("MCP local tools currently require macOS or Linux.".into())
        }
        pub(super) fn new(file: File) -> Result<Self, String> {
            let file = Arc::new(file);
            let target = file.clone();
            let (frames, queue) = mpsc::sync_channel::<Vec<u8>>(1);
            let (done, written) = mpsc::sync_channel(1);
            let writer = std::thread::Builder::new()
                .name("pitty-mcp-output".into())
                .spawn(move || {
                    for frame in queue {
                        let result = (&*target).write_all(&frame);
                        let failed = result.is_err();
                        if done.send(result).is_err() || failed {
                            return;
                        }
                    }
                })
                .map_err(|e| format!("Cannot start MCP output: {e}"))?;
            Ok(Self {
                file,
                frames: Some(frames),
                written,
                writer: Some(writer),
                busy: false,
                vanished: false,
            })
        }
        pub(super) fn send(
            &mut self,
            response: &Value,
            stopped: impl Fn() -> bool,
        ) -> Result<(), String> {
            let stalled = "MCP stdout stalled for two seconds; local operations stopped.";
            if self.busy {
                return Err(stalled.into());
            }
            let mut bytes = serde_json::to_vec(response).map_err(|e| e.to_string())?;
            bytes.push(b'\n');
            // The queue is empty whenever no write is outstanding; it is
            // closed only after a failed write has already been reported.
            let queued = match &self.frames {
                Some(frames) => frames.try_send(bytes).is_ok(),
                None => false,
            };
            if !queued {
                return Err("MCP output is closed.".into());
            }
            self.busy = true;
            let deadline = Instant::now() + STALL_LIMIT;
            loop {
                if stopped() {
                    return Err("MCP transport closed.".into());
                }
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(stalled.into());
                }
                match self.written.recv_timeout(left.min(POLL_INTERVAL)) {
                    Ok(Ok(())) => {
                        self.busy = false;
                        return Ok(());
                    }
                    Ok(Err(error)) => {
                        self.busy = false;
                        // The reader is gone: a client that vanished, not a failure.
                        self.vanished = matches!(
                            error.kind(),
                            io::ErrorKind::BrokenPipe
                                | io::ErrorKind::ConnectionReset
                                | io::ErrorKind::NotConnected
                                | io::ErrorKind::WriteZero
                        );
                        return Err(format!("Cannot write MCP response: {error}"));
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        self.busy = false;
                        return Err("MCP output stopped unexpectedly.".into());
                    }
                }
            }
        }
        /// True once the reading end has gone away, checked without writing:
        /// POLLERR (Linux) or POLLHUP (macOS) for a closed pipe or socket.
        /// The descriptor is ours and valid, so POLLNVAL only means it cannot
        /// be polled, as for /dev/null on macOS; that is not a disconnect.
        #[cfg(unix)]
        pub(super) fn disconnected(&self) -> bool {
            use std::os::fd::AsRawFd;
            let mut fd = libc::pollfd {
                fd: self.file.as_raw_fd(),
                events: libc::POLLOUT,
                revents: 0,
            };
            // SAFETY: one live repr(C) pollfd; a zero timeout never blocks.
            let ready = unsafe { libc::poll(&mut fd, 1, 0) };
            ready > 0 && fd.revents & (libc::POLLERR | libc::POLLHUP) != 0
        }
        #[cfg(not(unix))]
        pub(super) fn disconnected(&self) -> bool {
            false
        }
        /// A write failed because the client stopped reading stdout.
        pub(super) fn vanished(&self) -> bool {
            self.vanished
        }
    }
    impl Drop for Transport {
        fn drop(&mut self) {
            // An idle writer exits once its queue closes. A writer still
            // blocked on a stalled descriptor is left to process exit.
            self.frames = None;
            if !self.busy
                && let Some(writer) = self.writer.take()
            {
                let _ = writer.join();
            }
        }
    }
}

fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut frame = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "MCP messages must end with a newline.",
                ))
            };
        }
        let newline = available.iter().position(|&byte| byte == b'\n');
        let count = newline.unwrap_or(available.len());
        if frame.len().saturating_add(count) > INPUT_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "MCP message exceeds 8 MiB.",
            ));
        }
        frame.extend_from_slice(&available[..count]);
        reader.consume(count + usize::from(newline.is_some()));
        if newline.is_some() {
            if frame.last() == Some(&b'\r') {
                frame.pop();
            }
            return Ok(Some(frame));
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    New,
    Negotiated,
    Ready,
}

struct Worker {
    id: Value,
    batch: Option<u64>,
    cancel: Arc<AtomicBool>,
    handle: JoinHandle<Result<String, String>>,
}
struct Batch {
    id: u64,
    pending: usize,
    replies: Vec<Value>,
}
enum Dispatch {
    Reply(Value),
    Pending,
    Nothing,
}
struct Server {
    tools: Arc<LocalTools>,
    mode: ToolsSetting,
    definitions: Vec<Value>,
    phase: Phase,
    protocol: String,
    workers: Vec<Worker>,
    batches: Vec<Batch>,
    next_batch: u64,
}

impl Server {
    fn new(tools: LocalTools, mode: ToolsSetting) -> Self {
        let definitions = mcp_definitions(&tools, mode);
        Self {
            tools: Arc::new(tools),
            mode,
            definitions,
            phase: Phase::New,
            protocol: CURRENT_PROTOCOL.into(),
            workers: vec![],
            batches: vec![],
            next_batch: 1,
        }
    }

    fn handle(&mut self, message: Value) -> Vec<Value> {
        if let Value::Array(messages) = message {
            // March 2025 required receiving batches. Newer versions removed
            // them; keep support only for the negotiated older protocols.
            if self.phase == Phase::New
                || !matches!(self.protocol.as_str(), "2025-03-26" | "2024-11-05")
                || messages.is_empty()
                || messages.len() > MAX_BATCH
            {
                return vec![rpc_error(
                    Value::Null,
                    -32600,
                    "Invalid or unsupported MCP batch (maximum 32 messages).",
                )];
            }
            let batch = self.next_batch;
            self.next_batch = self.next_batch.wrapping_add(1);
            let mut replies = Vec::new();
            let mut pending = 0;
            for message in messages {
                match self.dispatch(message, Some(batch)) {
                    Dispatch::Reply(reply) => replies.push(reply),
                    Dispatch::Pending => pending += 1,
                    Dispatch::Nothing => {}
                }
            }
            if pending == 0 {
                return if replies.is_empty() {
                    vec![]
                } else {
                    vec![Value::Array(replies)]
                };
            }
            self.batches.push(Batch {
                id: batch,
                pending,
                replies,
            });
            return vec![];
        }
        match self.dispatch(message, None) {
            Dispatch::Reply(reply) => vec![reply],
            Dispatch::Pending | Dispatch::Nothing => vec![],
        }
    }

    fn dispatch(&mut self, message: Value, batch: Option<u64>) -> Dispatch {
        let Some(object) = message.as_object() else {
            return Dispatch::Reply(rpc_error(
                Value::Null,
                -32600,
                "Expected a JSON-RPC object.",
            ));
        };
        if message["jsonrpc"] != "2.0" {
            return Dispatch::Reply(rpc_error(Value::Null, -32600, "Expected JSON-RPC 2.0."));
        }
        let id = object.get("id").cloned();
        if id.as_ref().is_some_and(|id| !valid_id(id)) {
            return Dispatch::Reply(rpc_error(
                Value::Null,
                -32600,
                "Request ID must be an integer or a string of at most 1024 bytes.",
            ));
        }
        let Some(method) = message["method"].as_str() else {
            // This server sends no requests, so unsolicited responses are
            // ignored instead of creating an error-response feedback loop.
            if object.contains_key("result") || object.contains_key("error") {
                return Dispatch::Nothing;
            }
            return Dispatch::Reply(rpc_error(
                id.unwrap_or(Value::Null),
                -32600,
                "Missing method.",
            ));
        };
        let params = object.get("params");
        if params.is_some_and(|params| !params.is_object()) {
            return id.map_or(Dispatch::Nothing, |id| {
                Dispatch::Reply(rpc_error(id, -32602, "Parameters must be an object."))
            });
        }
        if id.is_none() {
            match method {
                "notifications/initialized" if self.phase == Phase::Negotiated => {
                    self.phase = Phase::Ready;
                }
                "notifications/cancelled" => {
                    if let Some(id) = params.and_then(|p| p.get("requestId")) {
                        for worker in &self.workers {
                            if &worker.id == id {
                                worker.cancel.store(true, Ordering::SeqCst);
                            }
                        }
                    }
                }
                _ => {}
            }
            // A notification must never execute a tool or receive a reply.
            return Dispatch::Nothing;
        }
        let id = id.unwrap();
        if self.workers.iter().any(|worker| worker.id == id) {
            return Dispatch::Reply(rpc_error(id, -32600, "Request ID is already in use."));
        }
        match method {
            "initialize" => self.initialize(id, params),
            "ping" => Dispatch::Reply(rpc_result(id, json!({}))),
            _ if self.phase != Phase::Ready => Dispatch::Reply(rpc_error(
                id,
                -32002,
                "Initialize the server and send notifications/initialized first.",
            )),
            "tools/list" => {
                if params.and_then(|p| p.get("cursor")).is_some() {
                    Dispatch::Reply(rpc_error(
                        id,
                        -32602,
                        "This tool list has no pagination cursor.",
                    ))
                } else {
                    Dispatch::Reply(rpc_result(id, json!({"tools":self.definitions})))
                }
            }
            "tools/call" => self.call(id, params, batch),
            _ => Dispatch::Reply(rpc_error(id, -32601, "Method not found.")),
        }
    }

    fn initialize(&mut self, id: Value, params: Option<&Value>) -> Dispatch {
        if self.phase != Phase::New {
            return Dispatch::Reply(rpc_error(id, -32600, "Server is already initialized."));
        }
        let Some(params) = params else {
            return Dispatch::Reply(rpc_error(id, -32602, "Missing initialization parameters."));
        };
        let Some(version) = params["protocolVersion"].as_str() else {
            return Dispatch::Reply(rpc_error(id, -32602, "Missing protocolVersion."));
        };
        if !params["capabilities"].is_object()
            || !params["clientInfo"]["name"].is_string()
            || !params["clientInfo"]["version"].is_string()
        {
            return Dispatch::Reply(rpc_error(
                id,
                -32602,
                "Initialization requires capabilities and clientInfo name/version.",
            ));
        }
        self.protocol = if PROTOCOLS.contains(&version) {
            version
        } else {
            CURRENT_PROTOCOL
        }
        .into();
        self.phase = Phase::Negotiated;
        let policy = match self.mode {
            ToolsSetting::Ask => {
                "Read-only mode. Credential-like files are never read over MCP. To expose changes and shell execution, restart with --tools auto and use host-side approvals."
            }
            ToolsSetting::Auto => {
                "Changes and shell execution are enabled. The host must obtain user approval. Credential-like files are never read over MCP. Shell commands are not OS-sandboxed and can access files or the network outside the working folder."
            }
            ToolsSetting::Off => "Local tools are disabled.",
        };
        Dispatch::Reply(rpc_result(
            id,
            json!({
                "protocolVersion":self.protocol,
                "capabilities":{"tools":{"listChanged":false}},
                "serverInfo":{"name":"pitty","version":env!("CARGO_PKG_VERSION")},
                "instructions":format!("{}\n{policy}", self.tools.context())
            }),
        ))
    }

    fn call(&mut self, id: Value, params: Option<&Value>, batch: Option<u64>) -> Dispatch {
        let Some(params) = params else {
            return Dispatch::Reply(rpc_error(id, -32602, "Missing tool parameters."));
        };
        let Some(name) = params["name"].as_str() else {
            return Dispatch::Reply(rpc_error(id, -32602, "Tool name must be a string."));
        };
        if !self
            .definitions
            .iter()
            .any(|definition| definition["name"] == name)
        {
            return Dispatch::Reply(rpc_error(id, -32602, "Unknown or disabled tool."));
        }
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !arguments.is_object() {
            return Dispatch::Reply(rpc_error(id, -32602, "Tool arguments must be an object."));
        }
        if self.workers.len() >= MAX_WORKERS {
            return Dispatch::Reply(rpc_error(
                id,
                -32000,
                "Four local operations are already active; wait for one to finish.",
            ));
        }
        let arguments = arguments.to_string();
        let name = name.to_string();
        let tools = self.tools.clone();
        let mode = self.mode;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let handle = match std::thread::Builder::new().name("pitty-mcp-tool".into()).spawn(move || {
            if worker_cancel.load(Ordering::SeqCst) {
                return Err("Operation canceled.".into());
            }
            let prepared = tools.prepare(&name, &arguments)?;
            let subject = prepared.audit_subject();
            let gated = prepared.requires_approval;
            // Pitty cannot ask its user here. Reads are annotated read-only,
            // so hosts may run them unprompted: a read that needs approval
            // (a credential-like file) is declined in every mode. --tools
            // auto still leaves changes and commands to the host's approval.
            if gated && read_only_name(&name) {
                crate::audit::note("mcp", "refused", &name, &subject, "");
                return Err(SENSITIVE_READ.into());
            }
            if mode != ToolsSetting::Auto && gated {
                crate::audit::note("mcp", "refused", &name, &subject, "");
                return Err("This action requires approval and is unavailable in read-only MCP mode. Restart with --tools auto only when the host can approve it.".into());
            }
            if worker_cancel.load(Ordering::SeqCst) {
                return Err("Operation canceled.".into());
            }
            if gated {
                crate::audit::note("mcp", "auto", &name, &subject, "");
            }
            let result = tools.execute(prepared, worker_cancel);
            if gated {
                let decision = if result.is_ok() { "ran" } else { "failed" };
                crate::audit::note("mcp", decision, &name, &subject, "");
            }
            result
        }) {
            Ok(handle) => handle,
            Err(_) => return Dispatch::Reply(rpc_error(id, -32603, "Cannot start local operation.")),
        };
        self.workers.push(Worker {
            id,
            batch,
            cancel,
            handle,
        });
        Dispatch::Pending
    }

    fn poll(&mut self) -> Vec<Value> {
        let mut replies = Vec::new();
        let mut index = 0;
        while index < self.workers.len() {
            if !self.workers[index].handle.is_finished() {
                index += 1;
                continue;
            }
            let worker = self.workers.swap_remove(index);
            let canceled = worker.cancel.load(Ordering::SeqCst);
            let outcome = worker
                .handle
                .join()
                .unwrap_or_else(|_| Err("Local operation stopped unexpectedly.".into()));
            let reply = (!canceled).then(|| tool_result(worker.id, outcome));
            if let Some(batch_id) = worker.batch {
                if let Some(batch) = self.batches.iter_mut().find(|batch| batch.id == batch_id) {
                    batch.pending = batch.pending.saturating_sub(1);
                    if let Some(reply) = reply {
                        batch.replies.push(reply);
                    }
                }
            } else if let Some(reply) = reply {
                replies.push(reply);
            }
        }
        let mut index = 0;
        while index < self.batches.len() {
            if self.batches[index].pending != 0 {
                index += 1;
                continue;
            }
            let batch = self.batches.swap_remove(index);
            if !batch.replies.is_empty() {
                replies.push(Value::Array(batch.replies));
            }
        }
        replies
    }

    fn stop(&mut self) {
        for worker in &self.workers {
            worker.cancel.store(true, Ordering::SeqCst);
        }
        for worker in self.workers.drain(..) {
            let _ = worker.handle.join();
        }
        self.batches.clear();
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

fn valid_id(id: &Value) -> bool {
    match id {
        Value::String(text) => text.len() <= 1024,
        Value::Number(number) => number.is_i64() || number.is_u64(),
        _ => false,
    }
}

fn read_only_name(name: &str) -> bool {
    matches!(name, "list_files" | "read_file" | "search_files")
}
fn mcp_definitions(tools: &LocalTools, mode: ToolsSetting) -> Vec<Value> {
    if mode == ToolsSetting::Off {
        return vec![];
    }
    tools
        .definitions()
        .into_iter()
        .filter_map(|definition| {
            let function = definition.get("function")?;
            let name = function["name"].as_str()?;
            let read_only = read_only_name(name);
            if mode != ToolsSetting::Auto && !read_only {
                return None;
            }
            // The shared description promises an approval MCP cannot ask for.
            let description = if name == "read_file" {
                json!(
                    "Read UTF-8 text from a connected folder (1 MiB maximum). Credential-like files (such as .env, .ssh, private keys, and secrets files) are always declined over MCP."
                )
            } else {
                function["description"].clone()
            };
            Some(json!({
                "name":name,
                "description":description,
                "inputSchema":function["parameters"],
                "annotations":{
                    "readOnlyHint":read_only,
                    "destructiveHint":!read_only,
                    "idempotentHint":read_only,
                    "openWorldHint":name == "run_command"
                }
            }))
        })
        .collect()
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}
fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
fn tool_result(id: Value, outcome: Result<String, String>) -> Value {
    let (text, failed) = match outcome {
        Ok(text) => (text, false),
        Err(error) => (error, true),
    };
    rpc_result(
        id,
        json!({"content":[{"type":"text","text":text}],"isError":failed}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::File,
        io::Cursor,
        os::fd::{AsRawFd, FromRawFd},
        time::Instant,
    };

    fn server(mode: ToolsSetting) -> Server {
        Server::new(
            LocalTools::new(vec![std::env::current_dir().unwrap()]).unwrap(),
            mode,
        )
    }
    fn initialize(server: &mut Server, protocol: &str) -> Value {
        let response = server.handle(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
            "protocolVersion":protocol,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}
        }})).pop().unwrap();
        assert!(
            server
                .handle(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
                .is_empty()
        );
        response
    }

    #[test]
    fn input_is_bounded_before_json_decoding() {
        assert_eq!(
            read_frame(&mut Cursor::new(b"{}\n")).unwrap().unwrap(),
            b"{}"
        );
        assert_eq!(
            read_frame(&mut Cursor::new(b"{}\r\n")).unwrap().unwrap(),
            b"{}"
        );
        assert!(read_frame(&mut Cursor::new(b"{}")).is_err());
        assert!(read_frame(&mut Cursor::new(vec![b'x'; INPUT_LIMIT + 1])).is_err());
        let mut at_limit = vec![b'x'; INPUT_LIMIT];
        at_limit.push(b'\n');
        assert_eq!(
            read_frame(&mut Cursor::new(at_limit))
                .unwrap()
                .unwrap()
                .len(),
            INPUT_LIMIT
        );
    }

    #[test]
    fn lifecycle_negotiates_versions_and_requires_initialized_notification() {
        for protocol in PROTOCOLS {
            let mut server = server(ToolsSetting::Ask);
            let before = server.handle(json!({"jsonrpc":"2.0","id":7,"method":"tools/list"}));
            assert_eq!(before[0]["error"]["code"], -32002);
            let result = initialize(&mut server, protocol);
            assert_eq!(result["result"]["protocolVersion"], *protocol);
            assert_eq!(
                result["result"]["capabilities"]["tools"]["listChanged"],
                false
            );
            let listing = server.handle(json!({"jsonrpc":"2.0","id":"list","method":"tools/list"}));
            assert!(listing[0]["result"]["tools"].is_array());
        }
        assert_eq!(
            initialize(&mut server(ToolsSetting::Ask), "future")["result"]["protocolVersion"],
            CURRENT_PROTOCOL
        );
    }

    #[test]
    fn schemas_and_policies_expose_only_the_intended_tools() {
        let ask = server(ToolsSetting::Ask);
        assert!(!ask.definitions.is_empty());
        for definition in &ask.definitions {
            assert!(read_only_name(definition["name"].as_str().unwrap()));
            assert_eq!(definition["annotations"]["readOnlyHint"], true);
            assert_eq!(definition["annotations"]["destructiveHint"], false);
            assert_eq!(definition["inputSchema"]["type"], "object");
        }
        let auto = server(ToolsSetting::Auto);
        assert!(auto.definitions.len() > ask.definitions.len());
        assert!(server(ToolsSetting::Off).definitions.is_empty());
    }

    #[test]
    fn notifications_never_execute_tools_and_bad_ids_are_rejected() {
        let mut server = server(ToolsSetting::Auto);
        initialize(&mut server, CURRENT_PROTOCOL);
        assert!(server.handle(json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"run_command","arguments":{"command":"false"}}})).is_empty());
        assert!(server.workers.is_empty());
        for id in [Value::Null, json!(0.5), json!(true)] {
            let result = server.handle(json!({"jsonrpc":"2.0","id":id,"method":"ping"}));
            assert_eq!(result[0]["error"]["code"], -32600);
        }
    }

    #[test]
    fn read_only_mode_rejects_write_requests_before_starting_a_worker() {
        let mut server = server(ToolsSetting::Ask);
        initialize(&mut server, CURRENT_PROTOCOL);
        let response = server.handle(
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":"write_file","arguments":{"path":"must-not-exist","content":"fixture"}
            }}),
        );
        assert_eq!(response[0]["error"]["code"], -32602);
        assert!(server.workers.is_empty());
    }

    #[test]
    fn tool_errors_are_content_results_not_transport_errors() {
        let error = tool_result(json!(8), Err("File does not exist.".into()));
        assert_eq!(error["id"], 8);
        assert_eq!(error["result"]["isError"], true);
        assert_eq!(
            error["result"]["content"][0]["text"],
            "File does not exist."
        );
        assert!(error.get("error").is_none());
        let mut output = Vec::new();
        send(&mut output, &error).unwrap();
        assert_eq!(output.iter().filter(|&&byte| byte == b'\n').count(), 1);
        assert_eq!(serde_json::from_slice::<Value>(&output).unwrap(), error);
    }

    fn parked_worker(id: usize) -> Worker {
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        Worker {
            id: json!(id),
            batch: None,
            cancel,
            handle: std::thread::spawn(move || {
                while !flag.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Ok("Canceled fixture; no operation ran.".into())
            }),
        }
    }

    #[test]
    fn cancellation_keeps_worker_slot_until_reaped_and_suppresses_reply() {
        let mut server = server(ToolsSetting::Auto);
        initialize(&mut server, CURRENT_PROTOCOL);
        server.workers.push(parked_worker(1));
        assert!(server.handle(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}})).is_empty());
        assert_eq!(server.workers.len(), 1);
        assert!(server.workers[0].cancel.load(Ordering::SeqCst));
        while !server.workers[0].handle.is_finished() {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(server.poll().is_empty());
        assert!(server.workers.is_empty());
    }

    #[test]
    fn four_worker_limit_and_shutdown_join_are_enforced() {
        let mut server = server(ToolsSetting::Auto);
        initialize(&mut server, CURRENT_PROTOCOL);
        for id in 1..=MAX_WORKERS {
            server.workers.push(parked_worker(id));
        }
        let name = server.definitions[0]["name"].clone();
        let response = server.handle(json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":name,"arguments":{}}}));
        assert_eq!(response[0]["error"]["code"], -32000);
        assert_eq!(server.workers.len(), MAX_WORKERS);
        server.stop();
        assert!(server.workers.is_empty());
    }

    #[test]
    fn legacy_batches_are_bounded_and_new_protocol_uses_single_messages() {
        let messages = json!([
            {"jsonrpc":"2.0","id":1,"method":"ping"},
            {"jsonrpc":"2.0","method":"notifications/unknown"},
            {"jsonrpc":"2.0","id":2,"method":"tools/list"}
        ]);
        let mut legacy = server(ToolsSetting::Ask);
        initialize(&mut legacy, "2025-03-26");
        let response = legacy.handle(messages.clone());
        assert_eq!(response.len(), 1);
        assert_eq!(response[0].as_array().unwrap().len(), 2);
        assert_eq!(response[0][0]["id"], 1);
        let excessive = Value::Array(vec![
            json!({"jsonrpc":"2.0","id":3,"method":"ping"});
            MAX_BATCH + 1
        ]);
        assert_eq!(legacy.handle(excessive)[0]["error"]["code"], -32600);
        let mut current = server(ToolsSetting::Ask);
        initialize(&mut current, CURRENT_PROTOCOL);
        assert_eq!(current.handle(messages)[0]["error"]["code"], -32600);
    }

    const HANDSHAKE: &str = concat!(
        r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":"list","method":"tools/list"}"#,
        "\n",
    );
    fn pipe() -> (File, File) {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) }
    }
    fn replies(text: &str) -> Vec<Value> {
        text.lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn closing_input_still_delivers_every_reply() {
        let (mut read_end, write_end) = pipe();
        let collector = std::thread::spawn(move || {
            let mut text = String::new();
            read_end.read_to_string(&mut text).map(|_| text)
        });
        let call = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_files","arguments":{"max_entries":1}}});
        let input = Cursor::new(format!("{HANDSHAKE}{call}\n").into_bytes());
        let transport = output::Transport::new(write_end).unwrap();
        serve(server(ToolsSetting::Ask), input, transport, || false).unwrap();
        let replies = replies(&collector.join().unwrap().unwrap());
        assert_eq!(replies.len(), 3, "{replies:?}");
        assert_eq!(replies[0]["id"], 0);
        assert_eq!(replies[0]["result"]["protocolVersion"], CURRENT_PROTOCOL);
        assert_eq!(replies[1]["id"], "list");
        assert!(
            replies[1]["result"]["tools"]
                .as_array()
                .is_some_and(|tools| !tools.is_empty())
        );
        // An operation still running at EOF finishes and its reply is written.
        assert_eq!(replies[2]["id"], 2);
        assert_eq!(replies[2]["result"]["isError"], false);
    }

    #[test]
    fn shared_terminal_style_descriptor_keeps_input_blocking() {
        let mut fds = [0; 2];
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) },
            0
        );
        let (mut client, endpoint) =
            unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };
        // As with a terminal, input and output share one open file description.
        let input = endpoint.try_clone().unwrap();
        let transport = output::Transport::new(endpoint).unwrap();
        let flags = unsafe { libc::fcntl(input.as_raw_fd(), libc::F_GETFL) };
        assert_eq!(flags & libc::O_NONBLOCK, 0);
        let worker = std::thread::spawn(move || {
            serve(server(ToolsSetting::Ask), input, transport, || false)
        });
        // Before any input arrives, the server waits instead of failing with EAGAIN.
        std::thread::sleep(Duration::from_millis(200));
        assert!(!worker.is_finished());
        client.write_all(HANDSHAKE.as_bytes()).unwrap();
        assert_eq!(
            unsafe { libc::shutdown(client.as_raw_fd(), libc::SHUT_WR) },
            0
        );
        let mut text = String::new();
        client.read_to_string(&mut text).unwrap();
        worker.join().unwrap().unwrap();
        let replies = replies(&text);
        assert_eq!(replies.len(), 2, "{replies:?}");
        assert_eq!(
            (&replies[0]["id"], &replies[1]["id"]),
            (&json!(0), &json!("list"))
        );
    }

    #[test]
    fn closed_output_is_a_vanished_client() {
        let (read_end, write_end) = pipe();
        drop(read_end);
        let mut transport = output::Transport::new(write_end).unwrap();
        assert!(transport.disconnected());
        assert!(
            transport
                .send(&json!({"jsonrpc":"2.0","id":1,"result":{}}), || false)
                .is_err()
        );
        assert!(transport.vanished());
        let (read_end, write_end) = pipe();
        drop(read_end);
        let input = Cursor::new(HANDSHAKE.as_bytes().to_vec());
        let transport = output::Transport::new(write_end).unwrap();
        assert!(serve(server(ToolsSetting::Ask), input, transport, || false).is_ok());
        // An output poll cannot watch (macOS reports POLLNVAL for /dev/null)
        // is still a working output, not a vanished client.
        let null = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/null")
            .unwrap();
        let mut transport = output::Transport::new(null).unwrap();
        assert!(!transport.disconnected());
        assert!(
            transport
                .send(&json!({"jsonrpc":"2.0","id":1,"result":{}}), || false)
                .is_ok()
        );
    }

    #[test]
    fn credential_reads_are_declined_in_every_mode() {
        struct Folder(std::path::PathBuf);
        impl Drop for Folder {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let folder = Folder(
            std::env::temp_dir().join(format!("pitty-mcp-reads-{}-{unique}", std::process::id())),
        );
        std::fs::create_dir(&folder.0).unwrap();
        std::fs::write(folder.0.join(".env"), "TOKEN=fixture-secret").unwrap();
        std::fs::write(folder.0.join("notes.txt"), "plain notes").unwrap();
        for mode in [ToolsSetting::Ask, ToolsSetting::Auto] {
            let mut server = Server::new(LocalTools::new(vec![folder.0.clone()]).unwrap(), mode);
            initialize(&mut server, CURRENT_PROTOCOL);
            let read_file = server
                .definitions
                .iter()
                .find(|definition| definition["name"] == "read_file")
                .unwrap();
            assert!(
                read_file["description"]
                    .as_str()
                    .unwrap()
                    .contains("declined over MCP")
            );
            let mut calls = vec![(".env", "read_file"), ("notes.txt", "read_file")];
            if mode == ToolsSetting::Auto {
                calls.push(("created.txt", "write_file"));
            }
            for (id, (path, name)) in calls.iter().enumerate() {
                let arguments = json!({"path":path,"content":"made over MCP"});
                let arguments = if *name == "read_file" {
                    json!({"path":path})
                } else {
                    arguments
                };
                let call = json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}});
                assert!(server.handle(call).is_empty());
            }
            let mut replies = Vec::new();
            let deadline = Instant::now() + Duration::from_secs(10);
            while replies.len() < calls.len() {
                assert!(Instant::now() < deadline);
                replies.extend(server.poll());
                std::thread::sleep(Duration::from_millis(5));
            }
            let reply = |id: usize| {
                let reply = replies.iter().find(|reply| reply["id"] == id).unwrap();
                let text = reply["result"]["content"][0]["text"].as_str().unwrap();
                (reply["result"]["isError"] == true, text.to_string())
            };
            let (failed, text) = reply(0);
            assert!(failed && text.contains("credentials") && !text.contains("fixture-secret"));
            let (failed, text) = reply(1);
            assert!(!failed && text.contains("plain notes"), "{text}");
            if mode == ToolsSetting::Auto {
                // Changes stay available in auto mode, approved by the host.
                assert!(!reply(2).0);
                let created = std::fs::read_to_string(folder.0.join("created.txt"));
                assert_eq!(created.unwrap(), "made over MCP");
            }
        }
    }
}
