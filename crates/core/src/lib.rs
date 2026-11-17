use serde::{Deserialize, Serialize};
use serde_json::Value;
pub const MODEL: &str = "nvidia/nemotron-3-ultra-550b-a55b:free";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub kind: String,
    pub mime: String,
    pub data: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_trace: Option<Value>,
}
impl Message {
    pub fn user(content: String) -> Self {
        Self {
            role: "user".into(),
            content,
            reasoning_details: None,
            tool_trace: None,
            attachments: vec![],
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<Message>,
}
impl ChatRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.messages.is_empty() || self.messages.len() > 200 {
            return Err("A conversation must contain between 1 and 200 messages.");
        }
        for (i, message) in self.messages.iter().enumerate() {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            if message.role != role || message.content.trim().is_empty() {
                return Err(
                    "Messages must alternate between user and assistant, with nonempty text.",
                );
            }
        }
        if self.messages.last().unwrap().role != "user" {
            return Err("A conversation must end with a user message.");
        }
        Ok(())
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct ChatReply {
    pub message: Message,
    pub truncated: bool,
}
/// name, arguments, description; used by both the native UI and Wasm parser.
pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("help", "", "Show all commands and keyboard shortcuts"),
    (
        "tools",
        "[ask|auto|off]",
        "Choose how file tools and commands run",
    ),
    (
        "run",
        "COMMAND",
        "Run a shell command in the working folder",
    ),
    (
        "exec",
        "COMMAND",
        "Run a program or script in the working folder",
    ),
    (
        "actions",
        "",
        "Review file changes, commands, and their output",
    ),
    (
        "audit",
        "",
        "Check the local decision log and its hash chain",
    ),
    ("new", "", "Start a fresh conversation"),
    ("chats", "", "Browse saved conversations"),
    ("open", "NUMBER", "Open a conversation from /chats"),
    ("rename", "TITLE", "Rename this conversation"),
    ("delete", "confirm", "Delete the current conversation"),
    (
        "clear",
        "confirm",
        "Clear messages in the current conversation",
    ),
    (
        "copy",
        "[last|chat|draft|message N|code N]",
        "Copy text to your system clipboard",
    ),
    ("export", "PATH", "Export this conversation as Markdown"),
    ("save", "", "Save this conversation now"),
    ("history", "[on|off]", "Show or change history saving"),
    ("tokens", "[NUMBER]", "Show or set the output token limit"),
    ("timeout", "[SECONDS]", "Show or set the request timeout"),
    (
        "system",
        "[PROMPT|off]",
        "Show, set, or clear the system instruction",
    ),
    (
        "interface",
        "[openrouter|openai|compatible URL]",
        "Choose the chat endpoint; starts a new conversation",
    ),
    (
        "model",
        "[ID|NUMBER]",
        "Select a model; starts a new conversation",
    ),
    (
        "models",
        "[free|QUERY]",
        "Browse the model catalog for this chat interface",
    ),
    (
        "web",
        "[on|off]",
        "Toggle web search; additional search charges may apply",
    ),
    (
        "attach",
        "PATH [PATH ...]",
        "Stage a file for the next message; nothing sent yet",
    ),
    (
        "connect",
        "PATH [PATH ...]",
        "Remember Desktop or mounted network folders",
    ),
    ("folders", "", "List connected folders; nothing uploaded"),
    (
        "files",
        "[NUMBER]",
        "Browse connected folders and choose files to attach",
    ),
    (
        "disconnect",
        "NUMBER",
        "Forget a connected folder; keep its files untouched",
    ),
    ("attachments", "", "List staged files and their numbers"),
    ("detach", "NUMBER|all", "Remove staged attachments"),
    ("key", "", "Paste your API key in a masked field"),
    (
        "pwd",
        "",
        "Show the full working directory and connected folders",
    ),
    ("name", "[NAME]", "Set your local display name"),
    ("team", "[NAME|off]", "Set an optional local team label"),
    (
        "peer",
        "[host [ADDR]|join ADDR|art [NAME]|move DIR|pick N|leave]",
        "Chat with nearby Pitty terminals and move ASCII art; no server and no database",
    ),
    (
        "agent",
        "NAME [--model ID] TASK | open ID | stop ID",
        "Run a named parallel task; uses the current model",
    ),
    ("agents", "", "Show parallel task progress and results"),
    (
        "select",
        "",
        "Toggle native text selection (F2); Ctrl+Y copies latest reply",
    ),
    ("search", "WORDS", "Find text in this conversation"),
    ("status", "", "Show settings and conversation details"),
    (
        "theme",
        "[auto|light|dark]",
        "Choose readable terminal colors",
    ),
    (
        "sidebar",
        "[show|hide]",
        "Show or hide the conversation sidebar",
    ),
    (
        "mouse",
        "[on|off]",
        "Toggle wheel scrolling or native terminal selection",
    ),
    ("compact", "[on|off]", "Adjust spacing between messages"),
    ("top", "", "Scroll to the start of the conversation"),
    ("bottom", "", "Jump to the latest message"),
    ("retry", "", "Retry the most recent failed prompt"),
    ("quit", "", "Close Pitty and return to your shell"),
];

pub fn parse_input(text: &str) -> Value {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix("//") {
        return serde_json::json!({"kind":"prompt","text":format!("/{rest}")});
    }
    if let Some(command) = text.strip_prefix('/') {
        let (name, args) = command
            .split_once(char::is_whitespace)
            .unwrap_or((command, ""));
        let name = name.to_lowercase();
        if COMMANDS.iter().any(|(known, _, _)| *known == name) {
            serde_json::json!({"kind":"command","name":name,"args":args.trim()})
        } else {
            serde_json::json!({"kind":"error","message":"Unknown command. Type /help, or // to send a literal slash."})
        }
    } else {
        serde_json::json!({"kind":"prompt","text":text})
    }
}

pub fn engine_request(input: &[u8]) -> Value {
    let Ok(request) = serde_json::from_slice::<Value>(input) else {
        return serde_json::json!({"error":"Invalid engine request"});
    };
    match request["op"].as_str() {
        Some("parse") => parse_input(request["text"].as_str().unwrap_or_default()),
        Some("validate") => match serde_json::from_value::<ChatRequest>(request["chat"].clone()) {
            Ok(chat) => match chat.validate() {
                Ok(()) => serde_json::json!({"ok":true}),
                Err(error) => serde_json::json!({"error":error}),
            },
            Err(_) => serde_json::json!({"error":"Invalid conversation"}),
        },
        _ => serde_json::json!({"error":"Unknown engine operation"}),
    }
}

// The native host creates a fresh Wasm instance per operation, then drops its
// entire memory. No imports, filesystem, networking, or JavaScript are needed.
#[cfg(target_arch = "wasm32")]
mod wasm {
    #[unsafe(no_mangle)]
    pub extern "C" fn pitty_alloc(len: u32) -> u32 {
        Box::into_raw(vec![0u8; len as usize].into_boxed_slice()) as *mut u8 as u32
    }
    /// # Safety
    /// The host must provide a valid allocation created by pitty_alloc.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn pitty_run(ptr: u32, len: u32) -> u64 {
        let input = unsafe { std::slice::from_raw_parts(ptr as *const u8, len as usize) };
        let output = serde_json::to_vec(&super::engine_request(input))
            .unwrap()
            .into_boxed_slice();
        let len = output.len() as u64;
        let ptr = Box::into_raw(output) as *mut u8 as u64;
        (ptr << 32) | len
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_injected_roles_and_invalid_turn_order() {
        let mut request = ChatRequest {
            messages: vec![Message::user("Hello".into())],
        };
        assert!(request.validate().is_ok());
        request.messages[0].role = "system".into();
        assert!(request.validate().is_err());
        request.messages[0].role = "assistant".into();
        assert!(request.validate().is_err());
    }
    #[test]
    fn preserves_reasoning_details_between_turns() {
        let message = Message {
            role: "assistant".into(),
            content: "Hello".into(),
            attachments: vec![],
            reasoning_details: Some(
                serde_json::json!([{"type":"reasoning.text", "text":"example"}]),
            ),
            tool_trace: None,
        };
        let roundtrip: Message =
            serde_json::from_str(&serde_json::to_string(&message).unwrap()).unwrap();
        assert_eq!(message.reasoning_details, roundtrip.reasoning_details);
    }
}
