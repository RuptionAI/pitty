use clap::{Parser, ValueEnum};
use reqwest::blocking::Client;
use serde_json::{Value, json};
use std::{
    env,
    fs::{self, File},
    io::{self, IsTerminal, Read, Write},
    path::PathBuf,
    time::Duration,
};

mod agents;
mod attachments;
mod audit;
mod audit_cert;
mod chat;
mod cli_tools;
mod engine;
mod folders;
mod interface;
mod local;
mod local_tools;
mod mcp;
mod paths;
mod peer;
mod profile;
mod signals;
mod transcript;
mod tui;
mod web;
use interface::InterfaceKind;
use pitty_core::MODEL;
const RESPONSE_LIMIT: usize = 8 * 1024 * 1024;
const KEY_FILE_LIMIT: usize = 64 * 1024;
const KEY_LIMIT: usize = 4096;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
enum ToolsSetting {
    #[default]
    Ask,
    Auto,
    Off,
}
impl ToolsSetting {
    fn label(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Auto => "auto",
            Self::Off => "off",
        }
    }
    /// A fresh approval policy for this setting; `None` when tools are off.
    fn policy(self) -> Option<chat::Policy> {
        match self {
            Self::Ask => Some(chat::Policy::new(false)),
            Self::Auto => Some(chat::Policy::new(true)),
            Self::Off => None,
        }
    }
}

/// Review the edit or command, then send chat to the interface you choose.
#[derive(Parser, Debug, Clone)]
#[command(
    version,
    after_help = "Examples:\n  pitty                     # open terminal chat\n  pitty \"Explain Rust ownership\"\n  pitty --interface openai --model gpt-4.1-mini \"Explain Rust ownership\"\n  pitty --interface compatible --base-url http://127.0.0.1:11434/v1 --model llama3.2\n  pitty --local             # use a model server on this computer (Ollama, LM Studio, ...)\n  pitty --web --search duckduckgo \"What changed in Rust 1.98?\"\n\nKey: the variable for the interface, or $XDG_CONFIG_HOME/pitty/.env\n     (defaults to ~/.config/pitty/.env).\n     OPENROUTER_API_KEY, OPENAI_API_KEY, or PITTY_API_KEY."
)]
struct Args {
    /// Chat endpoint. openrouter is the default; openai and compatible use the same review
    #[arg(long, value_enum, default_value_t = InterfaceKind::OpenRouter)]
    interface: InterfaceKind,
    /// Base URL for --interface compatible, such as http://127.0.0.1:11434/v1
    #[arg(long, value_name = "URL")]
    base_url: Option<String>,
    /// Use a model server on this computer (Ollama, LM Studio, llama.cpp, vLLM, Jan, ...)
    #[arg(long, conflicts_with_all = ["interface", "base_url", "mcp"])]
    local: bool,
    /// Where web searches go: duckduckgo (no key), searxng (with --search-url),
    /// brave (BRAVE_SEARCH_API_KEY), tavily (TAVILY_API_KEY), or openrouter
    /// (its hosted search). Default: openrouter on OpenRouter, otherwise the
    /// first configured of searxng, brave, tavily, then duckduckgo
    #[arg(long, value_enum, value_name = "PROVIDER")]
    search: Option<SearchChoice>,
    /// SearXNG address for web search, such as http://127.0.0.1:8888
    #[arg(long, value_name = "URL")]
    search_url: Option<String>,
    /// Most model turns one request may use. Ask checks in every 16 turns
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=1000))]
    max_turns: u32,
    /// Serve local file and command tools over MCP stdio (requires --directory)
    #[arg(long, conflicts_with_all = ["chat", "prompt"], requires = "directory")]
    mcp: bool,
    /// Local tools: ask before changes and commands, auto runs them, off disables them
    #[arg(long, value_enum, default_value_t = ToolsSetting::Ask)]
    tools: ToolsSetting,
    /// Working directory for chat, file tools, and commands
    #[arg(short = 'C', long, value_name = "PATH")]
    directory: Option<PathBuf>,
    /// Model id for the selected chat interface
    #[arg(long, default_value = MODEL)]
    model: String,
    /// Let the model search the web (see --search for where searches go)
    #[arg(long)]
    web: bool,
    /// Open the terminal chat (also the default without a prompt)
    #[arg(long)]
    chat: bool,
    /// Leave wheel gestures and mouse selection to the terminal
    #[arg(long)]
    no_mouse: bool,
    /// Keep this chat session in memory without saving history
    #[arg(long)]
    no_history: bool,
    /// Prompt words; piped input is appended when present
    prompt: Vec<String>,
    /// Upper bound on generated tokens (not a spending limit)
    #[arg(long, default_value_t = 4096, value_parser = clap::value_parser!(u32).range(1..))]
    max_tokens: u32,
    /// Optional system instruction
    #[arg(long)]
    system: Option<String>,
    /// Print JSON (ChatReply with tools; raw chat response with --tools off)
    #[arg(long)]
    json: bool,
    /// Request timeout in seconds
    #[arg(long, default_value_t = 180, value_parser = clap::value_parser!(u64).range(1..))]
    timeout: u64,
    /// Read the key from this dotenv file instead of the standard config file
    #[arg(long, value_name = "PATH")]
    key_file: Option<PathBuf>,
}

/// Where web searches go, chosen with --search or /web.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub(crate) enum SearchChoice {
    /// DuckDuckGo's HTML results, no key needed
    DuckDuckGo,
    /// A SearXNG instance, often your own (--search-url)
    SearXng,
    /// Brave Search API (BRAVE_SEARCH_API_KEY)
    Brave,
    /// Tavily API (TAVILY_API_KEY)
    Tavily,
    /// OpenRouter's hosted search (OpenRouter interface only)
    OpenRouter,
}
impl SearchChoice {
    pub(crate) fn parse(name: &str) -> Option<Self> {
        Self::from_str(name, true).ok()
    }
}

/// How a turn searches the web.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WebSearch {
    Off,
    /// OpenRouter's server tool searches during the model request.
    Hosted,
    /// Pitty searches and fetches through this provider.
    Pitty(web::Provider),
}

impl Args {
    pub(crate) fn prepare(&self) -> Result<(), String> {
        let _ = self.chat_endpoint()?;
        if let Some(url) = &self.search_url {
            web::normalize_searxng_url(url)?;
        }
        // Report a missing search key or address before any model request.
        self.web_search()?;
        Ok(())
    }
    /// The SearXNG address from --search-url or PITTY_SEARXNG_URL.
    fn searxng_url(&self) -> Option<String> {
        self.search_url
            .clone()
            .or_else(|| env::var("PITTY_SEARXNG_URL").ok())
            .filter(|url| !url.trim().is_empty())
    }
    pub(crate) fn web_search(&self) -> Result<WebSearch, String> {
        if !self.web {
            return Ok(WebSearch::Off);
        }
        let choice = match self.search {
            Some(choice) => Some(choice),
            None => match env::var("PITTY_SEARCH") {
                Ok(name) if !name.trim().is_empty() => {
                    Some(SearchChoice::parse(name.trim()).ok_or(
                        "PITTY_SEARCH must be duckduckgo, searxng, brave, tavily, or openrouter.",
                    )?)
                }
                _ => None,
            },
        };
        let choice = match choice {
            Some(choice) => choice,
            None if self.interface.extensions() => SearchChoice::OpenRouter,
            None if self.searxng_url().is_some() => SearchChoice::SearXng,
            None if secret(self, "BRAVE_SEARCH_API_KEY").is_some() => SearchChoice::Brave,
            None if secret(self, "TAVILY_API_KEY").is_some() => SearchChoice::Tavily,
            None => SearchChoice::DuckDuckGo,
        };
        let provider = match choice {
            SearchChoice::OpenRouter if self.interface.extensions() => {
                return Ok(WebSearch::Hosted);
            }
            SearchChoice::OpenRouter => {
                return Err("OpenRouter's hosted search needs the OpenRouter interface. Use /web duckduckgo, searxng, brave, or tavily.".into());
            }
            SearchChoice::DuckDuckGo => web::Provider::DuckDuckGo,
            SearchChoice::SearXng => web::Provider::SearXng {
                base: web::normalize_searxng_url(&self.searxng_url().ok_or(
                    "Set the SearXNG address: /web searxng URL, --search-url URL, or PITTY_SEARXNG_URL.",
                )?)?,
            },
            SearchChoice::Brave => web::Provider::Brave {
                key: secret(self, "BRAVE_SEARCH_API_KEY").ok_or(
                    "Set BRAVE_SEARCH_API_KEY in your environment or in ~/.config/pitty/.env.",
                )?,
            },
            SearchChoice::Tavily => web::Provider::Tavily {
                key: secret(self, "TAVILY_API_KEY").ok_or(
                    "Set TAVILY_API_KEY in your environment or in ~/.config/pitty/.env.",
                )?,
            },
        };
        Ok(WebSearch::Pitty(provider))
    }
    pub(crate) fn chat_endpoint(&self) -> Result<String, String> {
        interface::chat_endpoint(self.interface, self.base_url.as_deref())
    }
    pub(crate) fn models_endpoint(&self) -> Result<String, String> {
        interface::models_endpoint(self.interface, self.base_url.as_deref())
    }
    /// The built-in default model belongs to OpenRouter.
    pub(crate) fn needs_explicit_model(&self) -> bool {
        !self.interface.extensions() && self.model == MODEL
    }
}

fn main() {
    if let Err(error) = run(Args::parse()) {
        eprintln!("pitty: {}", pitty_ui::sanitize(&error));
        std::process::exit(1);
    }
}

fn config_path() -> Result<PathBuf, String> {
    paths::config_dir()
        .map(|dir| dir.join(".env"))
        .map_err(|_| "Set OPENROUTER_API_KEY or pass --key-file PATH.".into())
}

fn limited_text(reader: impl Read, limit: usize, label: &str) -> Result<String, String> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Cannot read {label}: {e}"))?;
    if bytes.len() > limit {
        return Err(format!("{label} exceeds the {} byte limit.", limit));
    }
    String::from_utf8(bytes).map_err(|_| format!("{label} is not valid UTF-8."))
}

pub(crate) fn key_value(value: &str) -> Result<String, String> {
    let key = value.trim();
    if key.is_empty() || key.len() > KEY_LIMIT || !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("API key must be a single nonempty ASCII token, at most 4096 bytes.".into());
    }
    Ok(key.to_string())
}

fn file_key(reader: impl Read, key_var: &str) -> Result<String, String> {
    let text = limited_text(reader, KEY_FILE_LIMIT, "API key file")?;
    for entry in dotenvy::from_read_iter(text.as_bytes()) {
        let (name, value) = entry.map_err(|_| "Invalid dotenv key file.".to_string())?;
        if name == key_var && !value.trim().is_empty() {
            return key_value(&value);
        }
    }
    Err(format!("Key file must contain {key_var}=your-key."))
}

fn open_key_file(path: &std::path::Path, kind: InterfaceKind) -> Result<File, String> {
    let read_error = || {
        format!(
            "Cannot read {}. Set {} or configure this file (see README).",
            path.display(),
            kind.key_var()
        )
    };
    let metadata = fs::metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            match kind {
                InterfaceKind::OpenRouter => format!(
                    "No OpenRouter API key yet. Start pitty and type /key, or set OPENROUTER_API_KEY (looked in {}). Get a key at https://openrouter.ai/keys",
                    path.display()
                ),
                InterfaceKind::OpenAI => format!(
                    "No OpenAI API key yet. Start pitty and type /key, or set OPENAI_API_KEY (looked in {}).",
                    path.display()
                ),
                InterfaceKind::Compatible => format!(
                    "No PITTY_API_KEY yet. A compatible endpoint can run without a key. To save one, type /key (looked in {}).",
                    path.display()
                ),
            }
        } else {
            read_error()
        }
    })?;
    if !metadata.is_file() || metadata.len() > KEY_FILE_LIMIT as u64 {
        return Err(format!(
            "Cannot read {}: expected a regular key file no larger than 64 KiB.",
            path.display()
        ));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    // A path can change after the metadata check. Opening nonblocking avoids
    // hanging on a FIFO substituted for the file; verify the descriptor again.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(target_os = "linux")]
        const NONBLOCK: i32 = 0x800;
        #[cfg(target_os = "macos")]
        const NONBLOCK: i32 = 0x4;
        options.custom_flags(NONBLOCK);
    }
    let file = options.open(path).map_err(|_| read_error())?;
    let metadata = file.metadata().map_err(|_| read_error())?;
    if !metadata.is_file() || metadata.len() > KEY_FILE_LIMIT as u64 {
        return Err(format!(
            "Cannot read {}: expected a regular key file no larger than 64 KiB.",
            path.display()
        ));
    }
    Ok(file)
}

fn load_key(args: &Args) -> Result<String, String> {
    let key_var = args.interface.key_var();
    // An explicit file takes precedence; never load a file from the working directory.
    if args.key_file.is_none()
        && let Ok(key) = env::var(key_var)
        && !key.trim().is_empty()
    {
        return key_value(&key);
    }
    let path = match args.key_file.clone().map(Ok).unwrap_or_else(config_path) {
        Ok(path) => path,
        Err(_) if args.interface.allows_missing_key() && args.key_file.is_none() => {
            return Ok(String::new());
        }
        Err(error) => return Err(error),
    };
    let file = match open_key_file(&path, args.interface) {
        Ok(file) => file,
        Err(error)
            if args.interface.allows_missing_key() && error.starts_with("No PITTY_API_KEY yet") =>
        {
            return Ok(String::new());
        }
        Err(error) => return Err(error),
    };
    match file_key(file, key_var) {
        Ok(key) => Ok(key),
        // A present but invalid key is still an error. Only a missing name is optional.
        Err(error)
            if args.interface.allows_missing_key()
                && error.starts_with("Key file must contain") =>
        {
            Ok(String::new())
        }
        Err(error) => Err(error),
    }
}

/// A secondary credential, such as a search API key: the environment first,
/// then the same key file as the chat key. Missing or unreadable is `None`.
pub(crate) fn secret(args: &Args, var: &str) -> Option<String> {
    if let Ok(value) = env::var(var)
        && !value.trim().is_empty()
    {
        return key_value(&value).ok();
    }
    let path = args
        .key_file
        .clone()
        .map(Ok)
        .unwrap_or_else(config_path)
        .ok()?;
    let file = open_key_file(&path, InterfaceKind::Compatible).ok()?;
    file_key(file, var).ok()
}

pub(crate) const NO_LOCAL_SERVER: &str = "No model server answered on this computer. Start one, for example `ollama serve` and `ollama pull llama3.2`, or the local server in LM Studio, then try again. Pitty looks for Ollama (port 11434, or OLLAMA_HOST), LM Studio (1234), llama.cpp (8080), vLLM (8000), Jan (1337), GPT4All (4891), and Docker Model Runner (12434). For another address, use --interface compatible --base-url URL.";

/// `--local`: the server on this computer that serves the requested model, or
/// the first one that serves any model.
fn use_local(mut args: Args) -> Result<Args, String> {
    let servers = local::detect(&std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
        false,
    )));
    let chosen = if args.model == MODEL {
        servers.iter().find(|server| !server.models.is_empty())
    } else {
        servers
            .iter()
            .find(|server| server.models.contains(&args.model))
    };
    let Some(server) = chosen else {
        return Err(match servers.first() {
            None => NO_LOCAL_SERVER.into(),
            Some(server) if args.model == MODEL => format!(
                "{} is running at {} but serves no models yet. With Ollama, run `ollama pull llama3.2`.",
                server.name, server.base_url
            ),
            Some(_) => format!(
                "No model server on this computer serves {}. Run pitty --local without --model to use the first local model.",
                args.model
            ),
        });
    };
    if args.model == MODEL {
        args.model = server.models[0].clone();
    }
    args.interface = InterfaceKind::Compatible;
    args.base_url = Some(server.base_url.clone());
    Ok(args)
}

fn combine_prompt(words: &[String], stdin: &str) -> Result<String, String> {
    let instruction = words.join(" ");
    match (instruction.trim().is_empty(), stdin.trim().is_empty()) {
        (true, true) => {
            Err("Prompt is empty. Use pitty \"your question\" or pipe text into pitty.".into())
        }
        (false, true) => Ok(instruction),
        (true, false) => Ok(stdin.to_string()),
        (false, false) => Ok(format!("{instruction}\n\n{stdin}")),
    }
}

fn request_body(args: &Args, prompt: &str) -> Value {
    let mut messages = Vec::new();
    if let Some(system) = &args.system {
        messages.push(json!({"role":"system", "content":system}));
    }
    messages.push(json!({"role":"user", "content":prompt}));
    chat::request_body(args, json!(messages))
}

fn decode_response(status: u16, body: &str, key: &str, source: &str) -> Result<Value, String> {
    let value: Value = serde_json::from_str(body)
        .map_err(|_| format!("{source} returned HTTP {status} with an invalid JSON response."))?;
    if !(200..300).contains(&status) || value.get("error").is_some() {
        let message = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("Request failed; check your key, balance, and model availability.");
        // Build only the displayed prefix. Replacing an empty or very short
        // key across a large upstream error must not amplify memory usage.
        let safe_message: String = if key.is_empty() {
            message.chars().take(1000).collect()
        } else {
            message
                .split(key)
                .enumerate()
                .flat_map(|(index, part)| {
                    (if index > 0 { "[REDACTED]" } else { "" })
                        .chars()
                        .chain(part.chars())
                })
                .take(1000)
                .collect()
        };
        return Err(format!("{source} HTTP {status}: {safe_message}"));
    }
    Ok(value)
}

fn answer(value: &Value) -> Result<&str, String> {
    value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| {
            "Model returned no text. Try a larger --max-tokens value or inspect --json.".into()
        })
}

fn run(args: Args) -> Result<(), String> {
    let args = if args.local { use_local(args)? } else { args };
    args.prepare()?;
    if let Some(directory) = &args.directory {
        env::set_current_dir(directory)
            .map_err(|e| format!("Cannot use folder {}: {e}", directory.display()))?;
    }
    if args.mcp {
        return mcp::run(&args);
    }
    if args.chat {
        return tui::run(args);
    }
    let mut input = String::new();
    if !io::stdin().is_terminal() {
        input = limited_text(io::stdin(), 2_097_152, "Piped input")?;
    }
    if args.prompt.is_empty() && input.trim().is_empty() {
        return tui::run(args);
    }
    let prompt = combine_prompt(&args.prompt, &input)?;
    if prompt.len() > 2_097_152 {
        return Err("Prompt exceeds the 2 MiB limit.".into());
    }
    if args.needs_explicit_model() {
        return Err(format!(
            "The {} interface needs --model. {} is the OpenRouter default.",
            args.interface.label(),
            MODEL
        ));
    }
    if args.tools != ToolsSetting::Off {
        return cli_tools::run(&args, prompt);
    }
    if matches!(args.web_search()?, WebSearch::Pitty(_)) {
        // Pitty searches before asking the model, so the reply is a ChatReply.
        let payload = pitty_core::ChatRequest {
            messages: vec![pitty_core::Message::user(prompt)],
        };
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reply = chat::request(&args, &payload, cancel)?;
        let output = cli_tools::format_reply(&reply, args.json, io::stdout().is_terminal())?;
        return writeln!(io::stdout().lock(), "{output}")
            .map_err(|e| format!("Cannot write response: {e}"));
    }
    let key = load_key(&args)?;
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(args.timeout))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("pitty/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("Cannot initialize HTTP client: {e}"))?;
    if io::stderr().is_terminal() {
        eprintln!(
            "Asking {} on {} (web {})…",
            pitty_ui::sanitize(&args.model),
            args.interface.label(),
            if args.web { "on" } else { "off" }
        );
    }
    let endpoint = args.chat_endpoint()?;
    let mut request = client.post(endpoint).json(&request_body(&args, &prompt));
    if !key.is_empty() {
        request = request.bearer_auth(&key);
    }
    let response = request.send().map_err(|e| format!("Request failed: {e}"))?;
    let status = response.status().as_u16();
    if response
        .content_length()
        .is_some_and(|len| len > RESPONSE_LIMIT as u64)
    {
        return Err("Response exceeds the 8 MiB limit.".into());
    }
    let body = limited_text(response, RESPONSE_LIMIT, "Response")?;
    let value = decode_response(status, &body, &key, args.interface.error_label())?;
    let output = if args.json {
        serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?
    } else {
        chat::answer_with_sources(&value)?
    };
    if value
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str)
        == Some("length")
    {
        eprintln!(
            "pitty: Token limit reached; output may be incomplete. Increase --max-tokens if needed."
        );
    }
    let output = if io::stdout().is_terminal() {
        pitty_ui::sanitize(&output)
    } else {
        output
    };
    match writeln!(io::stdout().lock(), "{output}") {
        Err(e) if e.kind() != io::ErrorKind::BrokenPipe => Err(format!("Cannot write output: {e}")),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn piping_preserves_input_and_adds_instruction() {
        assert_eq!(
            combine_prompt(&["Review".into()], "fn main() {}\n").unwrap(),
            "Review\n\nfn main() {}\n"
        );
        assert_eq!(combine_prompt(&[], "hello").unwrap(), "hello");
        assert!(combine_prompt(&[], " \n").is_err());
    }
    #[test]
    fn key_file_accepts_comments_and_quotes_without_executing_shell() {
        assert_eq!(
            file_key(
                b"# config\nOPENROUTER_API_KEY='test-key'\n".as_slice(),
                "OPENROUTER_API_KEY",
            )
            .unwrap(),
            "test-key"
        );
        assert!(file_key(b"OPENROUTER_API_KEY=\n".as_slice(), "OPENROUTER_API_KEY").is_err());
        assert!(
            file_key(
                b"OPENROUTER_API_KEY=test-key\n".as_slice(),
                "OPENAI_API_KEY",
            )
            .is_err()
        );
    }
    #[test]
    fn compatible_endpoint_omits_a_missing_key_and_keeps_a_bad_one() {
        use clap::Parser;
        let missing = env::temp_dir().join(format!(
            "pitty-missing-compatible-key-{}",
            std::process::id()
        ));
        let _ = fs::remove_file(&missing);
        let args = Args::try_parse_from([
            "pitty",
            "--interface",
            "compatible",
            "--base-url",
            "http://127.0.0.1:11434/v1",
            "--model",
            "llama3.2",
            "--key-file",
            missing.to_str().unwrap(),
        ])
        .unwrap();
        assert_eq!(load_key(&args).unwrap(), "");
        let folder = env::temp_dir().join(format!(
            "pitty-compatible-key-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join(".env");
        fs::write(&path, b"OPENROUTER_API_KEY=sk-or-kept\n").unwrap();
        let stored = Args::try_parse_from([
            "pitty",
            "--interface",
            "compatible",
            "--base-url",
            "http://127.0.0.1:11434/v1",
            "--model",
            "llama3.2",
            "--key-file",
            path.to_str().unwrap(),
        ])
        .unwrap();
        assert_eq!(load_key(&stored).unwrap(), "");
        fs::write(
            &path,
            b"PITTY_API_KEY=local-key\nOPENROUTER_API_KEY=sk-or-kept\n",
        )
        .unwrap();
        assert_eq!(load_key(&stored).unwrap(), "local-key");
        fs::write(&path, b"PITTY_API_KEY=bad key\n").unwrap();
        assert!(load_key(&stored).is_err());
        fs::remove_dir_all(folder).unwrap();
    }
    #[test]
    fn bounded_reads_stop_without_waiting_for_eof() {
        struct Endless {
            read: usize,
        }
        impl Read for Endless {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                buffer.fill(b'x');
                self.read += buffer.len();
                Ok(buffer.len())
            }
        }
        let mut input = Endless { read: 0 };
        assert!(
            limited_text(&mut input, 16, "Response")
                .unwrap_err()
                .contains("limit")
        );
        assert_eq!(input.read, 17);
        assert_eq!(limited_text("hé".as_bytes(), 3, "Response").unwrap(), "hé");
        assert!(
            limited_text(&[0xff][..], 3, "Response")
                .unwrap_err()
                .contains("UTF-8")
        );
    }
    #[test]
    fn key_files_and_values_are_bounded_without_echoing_secrets() {
        assert!(
            file_key(
                vec![b'#'; KEY_FILE_LIMIT + 1].as_slice(),
                "OPENROUTER_API_KEY"
            )
            .unwrap_err()
            .contains("limit")
        );
        for invalid in [
            "",
            "secret\r\nInjected: yes",
            "secrét",
            &"s".repeat(KEY_LIMIT + 1),
        ] {
            let error = key_value(invalid).unwrap_err();
            assert!(!error.contains("secret"));
            assert!(!error.contains("Injected"));
        }
        assert_eq!(key_value("  test-key \n").unwrap(), "test-key");
        assert!(
            file_key(
                b"OPENROUTER_API_KEY='secret\nInjected'\n".as_slice(),
                "OPENROUTER_API_KEY",
            )
            .is_err()
        );
    }
    #[test]
    fn key_file_must_be_a_small_regular_file() {
        let folder = env::temp_dir().join(format!(
            "pitty-key-check-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join("key.env");
        fs::write(&path, b"OPENROUTER_API_KEY=test-key\n").unwrap();
        assert_eq!(
            file_key(
                open_key_file(&path, InterfaceKind::OpenRouter).unwrap(),
                "OPENROUTER_API_KEY",
            )
            .unwrap(),
            "test-key"
        );
        assert!(
            open_key_file(&folder, InterfaceKind::OpenRouter)
                .unwrap_err()
                .contains("regular key file")
        );
        fs::write(&path, vec![b'#'; KEY_FILE_LIMIT + 1]).unwrap();
        assert!(
            open_key_file(&path, InterfaceKind::OpenRouter)
                .unwrap_err()
                .contains("64 KiB")
        );
        #[cfg(unix)]
        {
            let fifo = folder.join("pipe.env");
            assert!(
                std::process::Command::new("mkfifo")
                    .arg(&fifo)
                    .status()
                    .unwrap()
                    .success()
            );
            assert!(
                open_key_file(&fifo, InterfaceKind::OpenRouter)
                    .unwrap_err()
                    .contains("regular key file")
            );
        }
        fs::remove_dir_all(folder).unwrap();
    }
    #[test]
    fn requests_pin_model_and_respect_token_cap() {
        let args = Args::try_parse_from([
            "pitty",
            "--max-tokens",
            "100",
            "--system",
            "Be brief",
            "Hello",
        ])
        .unwrap();
        let body = request_body(&args, "Hello");
        assert_eq!(body["model"], MODEL);
        assert_eq!(body["max_tokens"], 100);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "Hello");
        assert!(Args::try_parse_from(["pitty", "--max-tokens", "0"]).is_err());
    }
    #[test]
    fn upstream_errors_redact_keys_and_handle_non_json() {
        let error = decode_response(
            401,
            r#"{"error":{"message":"Bad key secret"}}"#,
            "secret",
            "OpenRouter",
        )
        .unwrap_err();
        assert!(!error.contains("secret"));
        assert!(error.contains("[REDACTED]"));
        assert!(decode_response(502, "<html>failure</html>", "secret", "OpenRouter").is_err());
        assert!(
            decode_response(
                200,
                r#"{"error":{"message":"failed"}}"#,
                "secret",
                "OpenRouter"
            )
            .is_err()
        );
    }
    #[test]
    fn empty_key_and_large_unicode_errors_do_not_amplify_output() {
        let body = json!({"error":{"message":"Failure without a key"}}).to_string();
        let error = decode_response(400, &body, "", "OpenRouter").unwrap_err();
        assert_eq!(error, "OpenRouter HTTP 400: Failure without a key");
        let body = json!({"error":{"message":format!("{}secret{}", "é".repeat(990), "secret".repeat(5000))}}).to_string();
        let error = decode_response(400, &body, "secret", "OpenRouter").unwrap_err();
        assert!(!error.contains("secret"));
        assert!(error.ends_with("[REDACTED]"));
        assert_eq!(error.chars().count(), "OpenRouter HTTP 400: ".len() + 1000);
    }
    #[test]
    fn normal_and_empty_answers() {
        let value = decode_response(
            200,
            r#"{"choices":[{"message":{"content":"Hello"}}]}"#,
            "secret",
            "OpenRouter",
        )
        .unwrap();
        assert_eq!(answer(&value).unwrap(), "Hello");
        assert!(answer(&json!({"choices":[{"message":{"content":null}}]})).is_err());
    }
}
