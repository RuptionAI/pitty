use crate::{Args, RESPONSE_LIMIT, answer, decode_response, load_key};
use pitty_core::{ChatReply, ChatRequest, Message};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    sync::mpsc::{self, Sender},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
#[derive(Clone, Debug, Deserialize)]
pub struct Model {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, deserialize_with = "nullable_context")]
    pub context_length: u64,
    #[serde(default)]
    pub pricing: Value,
    #[serde(default)]
    pub architecture: Value,
    #[serde(default)]
    pub endpoints: Vec<String>,
}
impl Model {
    pub fn is_free(&self) -> bool {
        ["prompt", "completion"].iter().all(|k| {
            self.pricing[*k]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
                == Some(0.0)
        }) && self.pricing["request"]
            .as_str()
            .unwrap_or("0")
            .parse::<f64>()
            == Ok(0.0)
    }
    pub fn supports_chat(&self) -> bool {
        let endpoint = self.endpoints.is_empty()
            || self
                .endpoints
                .iter()
                .any(|e| e.ends_with("/chat/completions"));
        let text = self.architecture["output_modalities"]
            .as_array()
            .is_none_or(|a| a.iter().any(|v| v == "text"));
        endpoint && text
    }
    pub fn price(&self) -> String {
        let price = |k: &str| {
            self.pricing[k]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .map(|p| format!("${:.3}", p * 1_000_000.))
                .unwrap_or("n/a".into())
        };
        format!(
            "{} / {} per 1M · {}",
            price("prompt"),
            price("completion"),
            if self.supports_chat() {
                "chat"
            } else {
                "specialized endpoint"
            }
        )
    }
}
async fn canceled(flag: Arc<AtomicBool>) {
    while !flag.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}
fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}
/// Shared by the terminal and one-shot CLI, so web-off cannot drift between them.
pub fn request_body(args: &Args, messages: Value) -> Value {
    let model = if args.interface.extensions() {
        args.model
            .split(':')
            .filter(|part| *part != "online")
            .collect::<Vec<_>>()
            .join(":")
    } else {
        args.model.clone()
    };
    let mut body =
        json!({"model":model,"messages":messages,"max_tokens":args.max_tokens,"stream":false});
    // OpenRouter plugins and hosted search are not part of the shared chat protocol.
    if args.interface.extensions() {
        let pdf = messages.as_array().is_some_and(|messages| {
            messages.iter().any(|m| {
                m["content"]
                    .as_array()
                    .is_some_and(|parts| parts.iter().any(|p| p["type"] == "file"))
            })
        });
        body["plugins"] = json!([{"id":"web","enabled":false}]);
        // Native PDF processing uses only the selected model provider; no external OCR processor.
        if pdf {
            body["plugins"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":"file-parser","pdf":{"engine":"native"}}));
        }
        if args.web {
            body["tools"] = json!([{"type":"openrouter:web_search","parameters":{
                "engine":"exa","max_uses":1,"max_results":3,"max_total_results":3
            }}]);
        }
    }
    body
}

pub fn answer_with_sources(value: &Value) -> Result<String, String> {
    let mut text = answer(value)?.to_string();
    let mut sources = Vec::new();
    let mut seen = HashSet::new();
    if let Some(annotations) = value
        .pointer("/choices/0/message/annotations")
        .and_then(Value::as_array)
    {
        for item in annotations {
            if item["type"] != "url_citation" {
                continue;
            }
            if let Some(url) = item["url_citation"]["url"].as_str()
                && (url.starts_with("https://") || url.starts_with("http://"))
                && seen.insert(url)
            {
                sources.push(url);
            }
        }
    }
    if !sources.is_empty() {
        text.push_str("\n\nSources:\n");
        text.push_str(&sources.join("\n"));
    }
    Ok(text)
}

async fn bounded_body(mut response: reqwest::Response, limit: usize) -> Result<String, String> {
    if response
        .content_length()
        .is_some_and(|len| len > limit as u64)
    {
        return Err("Response exceeds the local size limit.".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err("Response exceeds the local size limit.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|_| "Response is not UTF-8.".into())
}

pub fn request(
    args: &Args,
    payload: &ChatRequest,
    cancel: Arc<AtomicBool>,
) -> Result<ChatReply, String> {
    check_canceled(&cancel, "Request canceled.")?;
    crate::engine::validate(payload)?;
    check_canceled(&cancel, "Request canceled.")?;
    let key = load_key(args)?;
    check_canceled(&cancel, "Request canceled.")?;
    let mut messages = crate::attachments::messages(&payload.messages)?;
    if let Some(system) = &args.system {
        messages
            .as_array_mut()
            .unwrap()
            .insert(0, json!({"role":"system","content":system}));
    }
    let body = request_body(args, messages);
    bounded_json_size(&body, crate::attachments::chat_limit()?)?;
    runtime()?.block_on(async {
        let work = async {
            let client = reqwest::Client::builder().timeout(Duration::from_secs(args.timeout))
                .connect_timeout(Duration::from_secs(15)).redirect(reqwest::redirect::Policy::none())
                .build().map_err(|e| e.to_string())?;
            let endpoint = args.chat_endpoint()?;
            let mut request = client.post(endpoint).json(&body);
            if !key.is_empty() {
                request = request.bearer_auth(&key);
            }
            let request = request
                .build()
                .map_err(|e| format!("Cannot prepare request: {e}"))?;
            check_canceled(&cancel, "Request canceled.")?;
            let response = client.execute(request)
                .await.map_err(|e| format!("Request failed: {e}"))?;
            let status = response.status().as_u16();
            let body = bounded_body(response, RESPONSE_LIMIT).await?;
            let value = decode_response(status, &body, &key, args.interface.error_label())?;
            Ok(ChatReply { message: Message { role: "assistant".into(), content: answer_with_sources(&value)?, attachments: vec![],
                reasoning_details: value.pointer("/choices/0/message/reasoning_details").cloned(), tool_trace: None },
                truncated: value.pointer("/choices/0/finish_reason").and_then(Value::as_str) == Some("length") })
        };
        tokio::select! { biased; _ = canceled(cancel.clone()) => Err("Request canceled.".into()), result = work => result }
    })
}
/// Off is represented by calling `request`; Ask prompts for sensitive reads,
/// file changes, and commands. Auto is an explicit caller-selected permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolMode {
    Ask,
    Auto,
}

#[derive(Debug)]
pub enum ToolEvent {
    Progress(String),
    Approval {
        summary: String,
        detail: String,
        response: Sender<bool>,
    },
    Completed {
        summary: String,
        output: String,
    },
}

const TOOL_TURNS: usize = 16;
const TOOL_OUTPUT_LIMIT: usize = 65_536;

/// A local result can be an ordinary tool error without ending the protocol.
/// The outer Result is reserved for cancellation or a disconnected controller.
struct ToolExecution {
    summary: String,
    content: String,
    failed: bool,
}

pub fn request_with_tools(
    args: &Args,
    payload: &ChatRequest,
    cancel: Arc<AtomicBool>,
    tools: crate::local_tools::LocalTools,
    event_sender: Sender<ToolEvent>,
    mode: ToolMode,
) -> Result<ChatReply, String> {
    check_canceled(&cancel, "Request canceled.")?;
    crate::engine::validate(payload)?;
    let mut messages = crate::attachments::messages(&payload.messages)?;
    let rows = messages.as_array_mut().unwrap();
    rows.insert(0, json!({"role":"system","content":tools.context()}));
    if let Some(system) = &args.system {
        rows.insert(0, json!({"role":"system","content":system}));
    }
    let mut body = request_body(args, messages);
    let definitions = body
        .as_object_mut()
        .unwrap()
        .entry("tools")
        .or_insert_with(|| json!([]));
    definitions
        .as_array_mut()
        .unwrap()
        .extend(tools.definitions());
    body["tool_choice"] = json!("auto");
    // We execute locally in order, even when a provider emits multiple calls.
    body["parallel_tool_calls"] = json!(false);
    let limit = crate::attachments::chat_limit()?;
    bounded_json_size(&body, limit)?;
    check_canceled(&cancel, "Request canceled.")?;
    let key = load_key(args)?;
    let endpoint = args.chat_endpoint()?;
    let source = args.interface.error_label();
    let rt = runtime()?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(args.timeout))
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let mut turn = 0;
    let result = run_tool_loop(
        body,
        limit,
        &cancel,
        |body| {
            turn += 1;
            event_sender
                .send(ToolEvent::Progress(format!(
                    "Model turn {turn}/{TOOL_TURNS}"
                )))
                .map_err(|_| "Tool controller disconnected; request stopped.".to_string())?;
            rt.block_on(send_tool_request(
                &client,
                &endpoint,
                &key,
                source,
                body,
                cancel.clone(),
            ))
        },
        |name, arguments| {
            execute_local_tool(&tools, name, arguments, &cancel, &event_sender, mode, &key)
        },
    );
    result
        .map(|mut reply| {
            reply.message.content = redacted_text(&reply.message.content, &key, RESPONSE_LIMIT);
            reply
        })
        .map_err(|error| redacted_text(&error, &key, 8192))
}

async fn send_tool_request(
    client: &reqwest::Client,
    endpoint: &str,
    key: &str,
    source: &str,
    body: &Value,
    cancel: Arc<AtomicBool>,
) -> Result<Value, String> {
    check_canceled(&cancel, "Request canceled.")?;
    let work = async {
        let mut request = client.post(endpoint).json(body);
        if !key.is_empty() {
            request = request.bearer_auth(key);
        }
        let response = request
            .send()
            .await
            .map_err(|error| format!("Request failed: {error}"))?;
        let status = response.status().as_u16();
        let text = bounded_body(response, RESPONSE_LIMIT).await?;
        let value = decode_response(status, &text, key, source)?;
        if !key.is_empty() && contains_secret(&value, key) {
            return Err("Provider response contained the API credential; it was discarded.".into());
        }
        Ok(value)
    };
    tokio::select! { biased; _ = canceled(cancel.clone()) => Err("Request canceled.".into()), result = work => result }
}

fn execute_local_tool(
    tools: &crate::local_tools::LocalTools,
    name: &str,
    arguments: &str,
    cancel: &Arc<AtomicBool>,
    events: &Sender<ToolEvent>,
    mode: ToolMode,
    key: &str,
) -> Result<ToolExecution, String> {
    check_canceled(cancel, "Request canceled.")?;
    let prepared = match tools.prepare(name, arguments) {
        Ok(prepared) => prepared,
        Err(error) => return complete_tool(events, name, Err(error), key),
    };
    let summary = redacted_text(&prepared.summary, key, 1024);
    let subject = prepared.audit_subject();
    let gated = prepared.requires_approval;
    if mode == ToolMode::Ask && gated {
        let detail = match approval_detail(&prepared.detail, key) {
            Ok(detail) => detail,
            Err(error) => return complete_tool(events, &summary, Err(error), key),
        };
        if !await_approval(events, &summary, &detail, cancel)? {
            crate::audit::note("user", "declined", name, &subject, key);
            return complete_tool(
                events,
                &summary,
                Err("User declined this tool request.".into()),
                key,
            );
        }
        crate::audit::note("user", "approved", name, &subject, key);
    } else if mode == ToolMode::Auto && gated {
        crate::audit::note("session", "auto", name, &subject, key);
    }
    check_canceled(cancel, "Request canceled.")?;
    events
        .send(ToolEvent::Progress(format!("Running: {summary}")))
        .map_err(|_| "Tool controller disconnected; request stopped.".to_string())?;
    let output = tools.execute(prepared, cancel.clone());
    if gated {
        let actor = match mode {
            ToolMode::Ask => "user",
            ToolMode::Auto => "session",
        };
        let decision = if output.is_ok() { "ran" } else { "failed" };
        crate::audit::note(actor, decision, name, &subject, key);
    }
    // A file change may finish just as Ctrl+C arrives. Audit that outcome before
    // terminating the loop so the user never loses evidence of a completed action.
    let completed = complete_tool(events, &summary, output, key);
    check_canceled(cancel, "Request canceled.")?;
    completed
}

/// Render the complete proposed operation. Refuse an oversized preview instead
/// of asking the user to approve a command or edit with an invisible tail.
fn approval_detail(text: &str, key: &str) -> Result<String, String> {
    if text.len() > 2 * crate::attachments::TOOL_ARGUMENT_LIMIT {
        return Err("Tool approval preview exceeds 2 MiB; operation was not executed.".into());
    }
    let mut result = String::new();
    let mut append = |part: &str| -> Result<(), String> {
        for ch in part.chars() {
            if (ch.is_control() && ch != '\n')
                || matches!(ch, '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')
            {
                result.extend(ch.escape_default());
            } else {
                result.push(ch);
            }
            if result.len() > 8 * crate::attachments::TOOL_ARGUMENT_LIMIT {
                return Err(
                    "Escaped tool approval preview exceeds 8 MiB; operation was not executed."
                        .into(),
                );
            }
        }
        Ok(())
    };
    if key.is_empty() {
        append(text)?;
    } else {
        for (index, part) in text.split(key).enumerate() {
            if index > 0 {
                append("[REDACTED]")?;
            }
            append(part)?;
        }
    }
    Ok(result)
}

fn await_approval(
    events: &Sender<ToolEvent>,
    summary: &str,
    detail: &str,
    cancel: &AtomicBool,
) -> Result<bool, String> {
    let (response, receiver) = mpsc::channel();
    events
        .send(ToolEvent::Approval {
            summary: summary.into(),
            detail: detail.into(),
            response,
        })
        .map_err(|_| "Tool controller disconnected; request stopped.".to_string())?;
    loop {
        check_canceled(cancel, "Request canceled.")?;
        match receiver.recv_timeout(Duration::from_millis(40)) {
            Ok(approved) => {
                check_canceled(cancel, "Request canceled.")?;
                return Ok(approved);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("Tool approval closed; request stopped.".into());
            }
        }
    }
}

fn complete_tool(
    events: &Sender<ToolEvent>,
    summary: &str,
    result: Result<String, String>,
    key: &str,
) -> Result<ToolExecution, String> {
    let summary = redacted_text(summary, key, 1024);
    let failed = result.is_err();
    let raw = match result {
        Ok(output) => output,
        Err(error) => format!("Tool error: {error}"),
    };
    let content = redacted_text(&raw, key, TOOL_OUTPUT_LIMIT);
    events
        .send(ToolEvent::Completed {
            summary: summary.clone(),
            output: content.clone(),
        })
        .map_err(|_| "Tool controller disconnected; request stopped.".to_string())?;
    Ok(ToolExecution {
        summary,
        content,
        failed,
    })
}

fn run_tool_loop(
    mut body: Value,
    limit: usize,
    cancel: &AtomicBool,
    mut send: impl FnMut(&Value) -> Result<Value, String>,
    mut execute: impl FnMut(&str, &str) -> Result<ToolExecution, String>,
) -> Result<ChatReply, String> {
    let mut trace = Vec::new();
    let mut activity = Vec::new();
    for turn in 0..TOOL_TURNS {
        check_canceled(cancel, "Request canceled.")?;
        bounded_json_size(&body, limit)?;
        let value = match send(&body) {
            Ok(value) => value,
            Err(error) if !trace.is_empty() && !cancel.load(Ordering::SeqCst) => {
                return Ok(tool_stopped(
                    &format!("Tools ran, but the follow-up request failed: {error}"),
                    trace,
                    &activity,
                    false,
                ));
            }
            Err(error) => return Err(error),
        };
        check_canceled(cancel, "Request canceled.")?;
        let assistant = match value
            .pointer("/choices/0/message")
            .filter(|message| message["role"] == "assistant")
        {
            Some(assistant) => assistant,
            None if !trace.is_empty() => {
                return Ok(tool_stopped(
                    "Tools ran, but the model returned an invalid assistant message.",
                    trace,
                    &activity,
                    false,
                ));
            }
            None => return Err("Model response has no valid assistant message.".into()),
        };
        let calls = match crate::attachments::function_calls(assistant) {
            Ok(calls) => calls,
            Err(error) if !trace.is_empty() => {
                return Ok(tool_stopped(
                    &format!("Stopped before an invalid tool request: {error}"),
                    trace,
                    &activity,
                    false,
                ));
            }
            Err(error) => return Err(error),
        };
        let truncated = value
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str)
            == Some("length");
        if calls.is_empty() {
            let content = match answer_with_sources(&value) {
                Ok(content) => content,
                Err(error) if trace.is_empty() => return Err(error),
                Err(_) => tool_summary(
                    "Tools finished, but the model returned no final text.",
                    &activity,
                ),
            };
            return Ok(ChatReply {
                message: Message {
                    role: "assistant".into(),
                    content,
                    attachments: vec![],
                    reasoning_details: assistant.get("reasoning_details").cloned(),
                    tool_trace: (!trace.is_empty()).then_some(Value::Array(trace)),
                },
                truncated,
            });
        }
        if truncated || turn + 1 == TOOL_TURNS {
            let reason = if truncated {
                "The model reached its output token limit. Its final tool requests were not executed."
            } else {
                "Stopped after 16 model turns. The remaining tool requests were not executed."
            };
            return Ok(tool_stopped(reason, trace, &activity, truncated));
        }
        // Reserve space before any operation can change a file. The output cap
        // accounts for worst-case JSON escaping, so all results in a batch fit.
        let rows = body["messages"]
            .as_array_mut()
            .ok_or("Invalid request messages.")?;
        rows.push(assistant.clone());
        let used = match bounded_json_size(&body, limit) {
            Ok(size) => size,
            Err(_) => {
                return Ok(tool_stopped(
                    "The conversation reached its local size limit. New tool requests were not executed.",
                    trace,
                    &activity,
                    false,
                ));
            }
        };
        let output_cap = (limit
            .saturating_sub(used)
            .saturating_sub(calls.len() * 1024)
            / calls.len()
            / 6)
        .min(TOOL_OUTPUT_LIMIT);
        if output_cap < 256 {
            return Ok(tool_stopped(
                "The conversation has no space for more tool results. New tool requests were not executed.",
                trace,
                &activity,
                false,
            ));
        }
        trace.push(assistant.clone());
        for (id, name, arguments) in calls {
            check_canceled(cancel, "Request canceled.")?;
            let result = execute(name, arguments)?;
            check_canceled(cancel, "Request canceled.")?;
            activity.push(format!(
                "{} — {}",
                redacted_text(&result.summary, "", 200),
                if result.failed {
                    "failed or declined"
                } else {
                    "completed"
                }
            ));
            let output = redacted_text(&result.content, "", output_cap);
            let row = json!({"role":"tool","tool_call_id":id,"content":output});
            trace.push(row.clone());
            body["messages"].as_array_mut().unwrap().push(row);
        }
    }
    unreachable!("the last turn returns without executing tools")
}

/// A reply that explains why tool work stopped early. `truncated` is true only
/// when the model hit its output token cap, matching non-tool replies.
fn tool_stopped(
    reason: &str,
    trace: Vec<Value>,
    activity: &[String],
    truncated: bool,
) -> ChatReply {
    ChatReply {
        message: Message {
            role: "assistant".into(),
            content: tool_summary(reason, activity),
            attachments: vec![],
            reasoning_details: None,
            tool_trace: (!trace.is_empty()).then_some(Value::Array(trace)),
        },
        truncated,
    }
}
fn tool_summary(reason: &str, activity: &[String]) -> String {
    let mut text = reason.to_string();
    if !activity.is_empty() {
        text.push_str("\n\nTool activity:\n");
        for item in activity {
            text.push_str("- ");
            text.push_str(item);
            text.push('\n');
        }
    }
    text
}

/// Count serialized bytes without allocating a second copy of large attachments.
fn bounded_json_size(value: &Value, limit: usize) -> Result<usize, String> {
    struct Counter {
        size: usize,
        limit: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.size) {
                return Err(std::io::Error::other("conversation limit"));
            }
            self.size += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { size: 0, limit };
    serde_json::to_writer(&mut counter, value).map_err(|_| "Conversation exceeds the local size limit. Start a new chat or reduce its files/tool output.".to_string())?;
    Ok(counter.size)
}

fn contains_secret(value: &Value, key: &str) -> bool {
    match value {
        Value::String(text) => text.contains(key),
        Value::Array(items) => items.iter().any(|item| contains_secret(item, key)),
        Value::Object(items) => items
            .iter()
            .any(|(name, value)| name.contains(key) || contains_secret(value, key)),
        _ => false,
    }
}

/// Output-only redaction: never rewrite the signed reasoning blocks or call IDs.
fn redacted_text(text: &str, key: &str, limit: usize) -> String {
    const MARKER: &str = "\n[Output truncated by Pitty.]";
    fn push(output: &mut String, text: &str, cap: usize) -> bool {
        let mut end = text.len().min(cap.saturating_sub(output.len()));
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        output.push_str(&text[..end]);
        end < text.len()
    }
    let cap = limit.saturating_sub(MARKER.len());
    let mut output = String::with_capacity(text.len().min(limit));
    let mut truncated = false;
    if key.is_empty() {
        truncated = push(&mut output, text, cap);
    } else {
        for (index, part) in text.split(key).enumerate() {
            if index > 0 && push(&mut output, "[REDACTED]", cap) {
                truncated = true;
                break;
            }
            if push(&mut output, part, cap) {
                truncated = true;
                break;
            }
        }
    }
    if truncated {
        push(&mut output, MARKER, limit);
    }
    output
}

fn check_canceled(cancel: &AtomicBool, message: &str) -> Result<(), String> {
    if cancel.load(Ordering::SeqCst) {
        Err(message.into())
    } else {
        Ok(())
    }
}

pub fn models(url: &str, cancel: Arc<AtomicBool>) -> Result<Vec<Model>, String> {
    check_canceled(&cancel, "Catalog request canceled.")?;
    let url = url.to_string();
    runtime()?.block_on(async {
        let work = async {
            let client = reqwest::Client::builder().timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none()).build().map_err(|e| e.to_string())?;
            let response = client.get(url)
                .send().await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| e.to_string())?;
            let body = bounded_body(response, 16_777_216).await?;
            #[derive(Deserialize)]
            struct Catalog { data: Vec<Model> }
            let mut models = serde_json::from_str::<Catalog>(&body).map_err(|e| e.to_string())?.data;
            models.sort_by(|a,b| b.is_free().cmp(&a.is_free()).then(a.id.cmp(&b.id)));
            Ok(models)
        };
        tokio::select! { biased; _ = canceled(cancel) => Err("Catalog request canceled.".into()), result = work => result }
    })
}

fn nullable_context<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    Ok(Option::<u64>::deserialize(deserializer)?.unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn function_reply(id: &str, name: &str) -> Value {
        json!({"choices":[{"message":{"role":"assistant","content":null,
            "tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":"{}"}}]},
            "finish_reason":"tool_calls"}]})
    }
    fn final_reply(text: &str) -> Value {
        json!({"choices":[{"message":{"role":"assistant","content":text},"finish_reason":"stop"}]})
    }
    fn fixture_body() -> Value {
        json!({"model":"fixture/model","messages":[{"role":"user","content":"Do the task"}],
            "tools":[{"type":"function","function":{"name":"fixture","parameters":{"type":"object"}}},
                {"type":"openrouter:web_search"}],"max_tokens":10,"stream":false})
    }
    fn fixture_execution(name: &str) -> ToolExecution {
        ToolExecution {
            summary: name.into(),
            content: format!("Result for {name}"),
            failed: false,
        }
    }

    #[test]
    fn ask_denies_file_changes_and_auto_executes_without_an_approval_event() {
        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture = Fixture(
            std::env::temp_dir().join(format!("pitty-chat-tools-{}-{unique}", std::process::id())),
        );
        std::fs::create_dir(&fixture.0).unwrap();
        let tools = crate::local_tools::LocalTools::new(vec![fixture.0.clone()]).unwrap();
        let arguments = json!({"path":"new.txt","content":"approved content"}).to_string();
        let (events, receiver) = mpsc::channel();
        let observer = std::thread::spawn(move || {
            match receiver.recv().unwrap() {
                ToolEvent::Approval {
                    detail, response, ..
                } => {
                    assert!(detail.contains("approved content"));
                    response.send(false).unwrap();
                }
                _ => panic!("file change did not request approval"),
            }
            match receiver.recv().unwrap() {
                ToolEvent::Completed { output, .. } => assert!(output.contains("declined")),
                _ => panic!("decline was not recorded"),
            }
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let denied = execute_local_tool(
            &tools,
            "write_file",
            &arguments,
            &cancel,
            &events,
            ToolMode::Ask,
            "fixture-key",
        )
        .unwrap();
        assert!(denied.failed);
        assert!(!fixture.0.join("new.txt").exists());
        observer.join().unwrap();
        let (events, receiver) = mpsc::channel();
        let written = execute_local_tool(
            &tools,
            "write_file",
            &arguments,
            &cancel,
            &events,
            ToolMode::Auto,
            "fixture-key",
        )
        .unwrap();
        assert!(!written.failed);
        assert_eq!(
            std::fs::read_to_string(fixture.0.join("new.txt")).unwrap(),
            "approved content"
        );
        assert!(
            receiver
                .try_iter()
                .all(|event| !matches!(event, ToolEvent::Approval { .. }))
        );
    }
    #[test]
    fn approvals_keep_long_tails_and_expose_control_characters() {
        let text = format!(
            "{}\nTAIL \u{001b}[2J \r \u{202e} secret-key",
            "x".repeat(20_000)
        );
        let preview = approval_detail(&text, "secret-key").unwrap();
        assert!(preview.contains("TAIL"));
        assert!(preview.contains("\\u{1b}[2J"));
        assert!(preview.contains("\\r"));
        assert!(preview.contains("\\u{202e}"));
        assert!(!preview.contains("secret-key"));
        assert!(!preview.contains('\u{001b}'));
        assert!(approval_detail(&"x".repeat(2_097_153), "").is_err());
    }
    #[test]
    fn tool_protocol_mock_server_preserves_ids_reasoning_and_future_turns() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let mut first = function_reply("call_1", "read_file");
        first["choices"][0]["message"]["tool_calls"].as_array_mut().unwrap().push(
            json!({"id":"call_2","type":"function","function":{"name":"list_files","arguments":"{}"}}));
        first["choices"][0]["message"]["reasoning_details"] = json!([{"type":"reasoning.encrypted","data":"opaque-signed-data","signature":"unchanged"}]);
        let mut second = function_reply("call_3", "edit_file");
        second["choices"][0]["message"]["reasoning"] = json!("fixture-reasoning");
        let expected_first = first["choices"][0]["message"].clone();
        let expected_second = second["choices"][0]["message"].clone();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for reply in [first, second, final_reply("Done.")] {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut input = Vec::new();
                let mut chunk = [0; 4096];
                let header_end = loop {
                    let count = socket.read(&mut chunk).unwrap();
                    assert!(count > 0);
                    input.extend_from_slice(&chunk[..count]);
                    assert!(input.len() < 1_048_576);
                    if let Some(index) = input.windows(4).position(|s| s == b"\r\n\r\n") {
                        break index + 4;
                    }
                };
                let header = String::from_utf8_lossy(&input[..header_end]);
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    })
                    .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                    .unwrap();
                while input.len() < header_end + length {
                    let count = socket.read(&mut chunk).unwrap();
                    assert!(count > 0);
                    input.extend_from_slice(&chunk[..count]);
                }
                requests.push(
                    serde_json::from_slice::<Value>(&input[header_end..header_end + length])
                        .unwrap(),
                );
                let body = serde_json::to_vec(&reply).unwrap();
                write!(socket,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",body.len()).unwrap();
                socket.write_all(&body).unwrap();
            }
            requests
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let rt = runtime().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut ran = Vec::new();
        let reply = run_tool_loop(
            fixture_body(),
            1_048_576,
            &cancel,
            |body| {
                rt.block_on(send_tool_request(
                    &client,
                    &format!("http://{address}/fixture"),
                    "not-a-real-credential",
                    "OpenRouter",
                    body,
                    cancel.clone(),
                ))
            },
            |name, _| {
                ran.push(name.to_string());
                Ok(fixture_execution(name))
            },
        )
        .unwrap();
        let requests = server.join().unwrap();
        assert_eq!(ran, ["read_file", "list_files", "edit_file"]);
        assert_eq!(requests.len(), 3);
        assert!(
            requests
                .iter()
                .all(|request| request["tools"].as_array().unwrap().len() == 2)
        );
        assert_eq!(requests[1]["messages"][1], expected_first);
        assert_eq!(requests[1]["messages"][2]["tool_call_id"], "call_1");
        assert_eq!(requests[1]["messages"][3]["tool_call_id"], "call_2");
        assert_eq!(requests[2]["messages"][4], expected_second);
        assert_eq!(requests[2]["messages"][5]["tool_call_id"], "call_3");
        assert_eq!(reply.message.content, "Done.");
        let trace = reply.message.tool_trace.as_ref().unwrap();
        crate::attachments::validate_tool_trace(trace).unwrap();
        let future = crate::attachments::messages(&[
            Message::user("Do the task".into()),
            reply.message,
            Message::user("What changed?".into()),
        ])
        .unwrap();
        assert_eq!(future.as_array().unwrap().len(), 8);
        assert_eq!(future[1], expected_first);
        assert_eq!(future[4], expected_second);
        assert_eq!(future[6]["content"], "Done.");
    }

    #[test]
    fn tool_approval_denial_disconnect_and_cancellation_are_explicit() {
        let (events, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            if let ToolEvent::Approval { response, .. } = receiver.recv().unwrap() {
                response.send(false).unwrap();
            } else {
                panic!("missing approval");
            }
        });
        assert!(
            !await_approval(
                &events,
                "Write a file",
                "Exact proposed change",
                &AtomicBool::new(false)
            )
            .unwrap()
        );
        worker.join().unwrap();
        let (events, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let worker = std::thread::spawn(move || {
            let event = receiver.recv().unwrap();
            flag.store(true, Ordering::SeqCst);
            // Keep the response channel alive to prove cancellation is polled.
            std::thread::sleep(Duration::from_millis(80));
            drop(event);
        });
        assert_eq!(
            await_approval(&events, "Run", "command", &cancel).unwrap_err(),
            "Request canceled."
        );
        worker.join().unwrap();
        let (events, receiver) = mpsc::channel();
        drop(receiver);
        assert!(
            await_approval(&events, "Run", "command", &AtomicBool::new(false))
                .unwrap_err()
                .contains("disconnected")
        );
    }

    #[test]
    fn denied_tools_return_a_tool_result_and_prose_never_executes() {
        let mut turn = 0;
        let reply = run_tool_loop(
            fixture_body(),
            1_048_576,
            &AtomicBool::new(false),
            |body| {
                turn += 1;
                if turn == 1 {
                    Ok(function_reply("denied", "write_file"))
                } else {
                    assert!(
                        body["messages"][2]["content"]
                            .as_str()
                            .unwrap()
                            .contains("declined")
                    );
                    Ok(final_reply("The change was declined."))
                }
            },
            |name, _| {
                Ok(ToolExecution {
                    summary: name.into(),
                    content: "Tool error: User declined this tool request.".into(),
                    failed: true,
                })
            },
        )
        .unwrap();
        assert_eq!(reply.message.content, "The change was declined.");
        let reply = run_tool_loop(
            fixture_body(),
            1_048_576,
            &AtomicBool::new(false),
            |_| {
                Ok(final_reply(
                    "Please run rm -rf example; {\"tool_calls\": []}",
                ))
            },
            |_, _| panic!("prose executed as tool"),
        )
        .unwrap();
        assert!(reply.message.tool_trace.is_none());
    }

    #[test]
    fn tool_loop_caps_turns_batches_and_context_before_execution() {
        let mut sent = 0;
        let mut executed = 0;
        let reply = run_tool_loop(
            fixture_body(),
            1_048_576,
            &AtomicBool::new(false),
            |_| {
                sent += 1;
                Ok(function_reply(&format!("call_{sent}"), "read_file"))
            },
            |name, _| {
                executed += 1;
                Ok(fixture_execution(name))
            },
        )
        .unwrap();
        assert_eq!(sent, 16);
        assert_eq!(executed, 15);
        // The turn cap is not a token limit.
        assert!(!reply.truncated);
        assert!(reply.message.content.contains("16 model turns"));
        crate::attachments::validate_tool_trace(reply.message.tool_trace.as_ref().unwrap())
            .unwrap();
        let mut batch = function_reply("call_0", "read_file");
        for i in 1..9 {
            batch["choices"][0]["message"]["tool_calls"].as_array_mut().unwrap().push(json!({"id":format!("call_{i}"),"type":"function","function":{"name":"read_file","arguments":"{}"}}));
        }
        assert!(
            run_tool_loop(
                fixture_body(),
                1_048_576,
                &AtomicBool::new(false),
                |_| Ok(batch.clone()),
                |_, _| panic!("oversized batch executed")
            )
            .unwrap_err()
            .contains("eight")
        );
        let body = fixture_body();
        let limit = serde_json::to_vec(&body).unwrap().len() + 100;
        let reply = run_tool_loop(
            body,
            limit,
            &AtomicBool::new(false),
            |_| Ok(function_reply("limited", "read_file")),
            |_, _| panic!("no room for tool result"),
        )
        .unwrap();
        assert!(!reply.truncated);
        assert!(reply.message.tool_trace.is_none());
    }

    #[test]
    fn tool_network_failure_keeps_completed_trace_and_output_bounds_redact_keys() {
        let mut turn = 0;
        let reply = run_tool_loop(
            fixture_body(),
            1_048_576,
            &AtomicBool::new(false),
            |_| {
                turn += 1;
                if turn == 1 {
                    Ok(function_reply("call_1", "write_file"))
                } else {
                    Err("fixture network failure".into())
                }
            },
            |name, _| Ok(fixture_execution(name)),
        )
        .unwrap();
        assert!(reply.message.content.contains("follow-up request failed"));
        assert!(reply.message.tool_trace.is_some());
        let (events, receiver) = mpsc::channel();
        let output = complete_tool(
            &events,
            "read secret-key",
            Ok("secret-key".repeat(100_000)),
            "secret-key",
        )
        .unwrap();
        assert!(!output.content.contains("secret-key"));
        assert!(output.content.len() <= TOOL_OUTPUT_LIMIT);
        if let ToolEvent::Completed { summary, output } = receiver.recv().unwrap() {
            assert!(!summary.contains("secret-key"));
            assert!(!output.contains("secret-key"));
        } else {
            panic!("missing completion");
        }
        for cap in [0, 1, 20, 100] {
            assert!(redacted_text("é秘密a".repeat(100).as_str(), "a", cap).len() <= cap);
        }
        assert!(contains_secret(
            &json!({"reasoning_details":[{"data":"secret-key"}]}),
            "secret-key"
        ));
    }
    #[test]
    fn canceled_jobs_exit_before_validation_credentials_or_network() {
        use clap::Parser;
        let args =
            Args::try_parse_from(["pitty", "--key-file", "/missing/pitty-test-key"]).unwrap();
        let flag = Arc::new(AtomicBool::new(true));
        assert_eq!(
            request(&args, &ChatRequest { messages: vec![] }, flag.clone()).unwrap_err(),
            "Request canceled."
        );
        assert_eq!(
            models("http://127.0.0.1/models", flag).unwrap_err(),
            "Catalog request canceled."
        );
    }
    #[test]
    fn response_limits_cover_content_length_chunking_and_utf8() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        fn fetch(wire: &'static [u8], limit: usize) -> Result<String, String> {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut buffer = [0; 1024];
                assert!(
                    socket.read(&mut buffer).unwrap() > 0,
                    "client sent no request"
                );
                let _ = socket.write_all(wire);
            });
            let result = runtime().unwrap().block_on(async {
                let response = reqwest::Client::builder()
                    .no_proxy()
                    .timeout(Duration::from_secs(3))
                    .build()
                    .unwrap()
                    .get(format!("http://{address}/fixture"))
                    .send()
                    .await
                    .unwrap();
                bounded_body(response, limit).await
            });
            server.join().unwrap();
            result
        }
        assert_eq!(
            fetch(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nokay", 4).unwrap(),
            "okay"
        );
        // Announced overflow is rejected before waiting for an absent body.
        assert!(
            fetch(b"HTTP/1.1 200 OK\r\nContent-Length: 10000000\r\n\r\n", 4)
                .unwrap_err()
                .contains("size limit")
        );
        assert!(fetch(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n3\r\ndef\r\n0\r\n\r\n", 4).unwrap_err().contains("size limit"));
        assert!(
            fetch(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\n\xff", 4)
                .unwrap_err()
                .contains("UTF-8")
        );
    }
    #[test]
    fn catalog_handles_specialized_models_and_unknown_prices() {
        let model:Model=serde_json::from_value(json!({"id":"example/rerank","context_length":null,"endpoints":["/api/v1/rerank"],"architecture":{"output_modalities":["rerank"]},"pricing":{"prompt":"0","completion":"0"}})).unwrap();
        assert!(!model.supports_chat());
        assert!(model.is_free());
        assert_eq!(model.context_length, 0);
        let unknown: Model = serde_json::from_value(json!({"id":"example/model"})).unwrap();
        assert!(!unknown.is_free());
    }
    #[test]
    fn web_off_removes_online_suffix_and_web_on_has_bounded_search() {
        use clap::Parser;
        let mut args =
            Args::try_parse_from(["pitty", "--model", "example/model:free:online"]).unwrap();
        let off = request_body(&args, json!([]));
        assert_eq!(off["model"], "example/model:free");
        assert_eq!(off["plugins"][0]["enabled"], false);
        assert!(off.get("tools").is_none());
        args.web = true;
        let on = request_body(&args, json!([]));
        assert_eq!(on["tools"][0]["type"], "openrouter:web_search");
        assert_eq!(on["tools"][0]["parameters"]["max_uses"], 1);
        let openai = Args::try_parse_from([
            "pitty",
            "--interface",
            "openai",
            "--model",
            "gpt-4.1-mini:online",
            "--web",
        ])
        .unwrap();
        assert!(openai.prepare().is_err());
        let mut openai = Args::try_parse_from([
            "pitty",
            "--interface",
            "openai",
            "--model",
            "gpt-4.1-mini:online",
        ])
        .unwrap();
        openai.web = true;
        let body = request_body(&openai, json!([]));
        assert_eq!(body["model"], "gpt-4.1-mini:online");
        assert!(body.get("plugins").is_none());
        assert!(body.get("tools").is_none());
        assert_eq!(
            openai.chat_endpoint().unwrap(),
            "https://api.openai.com/v1/chat/completions"
        );
    }
    #[test]
    fn returned_sources_are_visible_and_deduplicated() {
        let value = json!({"choices":[{"message":{"content":"An answer", "annotations":[
            {"type":"url_citation","url_citation":{"url":"https://example.com/a"}},
            {"type":"url_citation","url_citation":{"url":"https://example.com/a"}},
            {"type":"url_citation","url_citation":{"url":"javascript:alert(1)"}}
        ]}}]});
        let text = answer_with_sources(&value).unwrap();
        assert_eq!(text.matches("https://example.com/a").count(), 1);
        assert!(!text.contains("javascript:"));
    }
    #[test]
    fn pdf_processing_is_pinned_to_selected_provider() {
        use clap::Parser;
        let args = Args::try_parse_from(["pitty"]).unwrap();
        let body = request_body(
            &args,
            json!([{"role":"user","content":[{"type":"file","file":{"filename":"example.pdf","file_data":"data:application/pdf;base64,ZXhhbXBsZQ=="}}]}]),
        );
        assert_eq!(body["plugins"][1]["pdf"]["engine"], "native");
    }
}
