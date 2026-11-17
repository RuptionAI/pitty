use crate::{Args, chat, engine, interface::InterfaceKind};
mod folder_browser;
mod mouse_selection;
mod task_view;
mod tools_view;
mod transcript_view;
use folder_browser::FolderResult;
use pitty_core::{COMMANDS, ChatReply, ChatRequest, MODEL, Message};
use pitty_ui::{Editor, Frame, Key, Style, Terminal, sanitize, width, wrap, wrap_words};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    io::{self, IsTerminal, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use task_view::elapsed;
fn clean(s: &str) -> String {
    sanitize(s)
}
fn history_file(path: &std::path::Path) -> io::Result<fs::File> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(io::Error::other("not a regular history file"));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(target_os = "macos")]
        const FLAGS: i32 = 0x4 | 0x100;
        #[cfg(target_os = "linux")]
        const FLAGS: i32 = 0x800 | 0x20000;
        options.custom_flags(FLAGS);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("history file changed type"));
    }
    Ok(file)
}
fn default_model() -> String {
    MODEL.into()
}
/// Fallback when the terminal does not report its background: COLORFGBG is
/// "foreground;background" (sometimes with a middle field), using ANSI color
/// numbers where 0–6 and 8 are dark.
fn colorfgbg_dark() -> bool {
    env::var("COLORFGBG")
        .ok()
        .and_then(|value| value.rsplit(';').next()?.parse::<u8>().ok())
        .is_some_and(|background| background <= 6 || background == 8)
}
/// Show a path under the home directory as ~/…, the way shells do.
fn home_relative(path: &std::path::Path) -> String {
    if let Some(home) = env::var_os("HOME").filter(|home| !home.is_empty())
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() {
            "~".into()
        } else {
            format!("~/{}", rest.display())
        };
    }
    path.display().to_string()
}
/// Fit text into `max` columns by replacing its middle with an ellipsis,
/// keeping more of the end, where a path's most specific part is.
fn shorten_middle(text: &str, max: usize) -> String {
    if width(text) <= max {
        return text.to_string();
    }
    let budget = max.saturating_sub(1);
    let head_budget = budget / 3;
    let mut head = String::new();
    for c in text.chars() {
        if width(&head) + pitty_ui::char_width(c) > head_budget {
            break;
        }
        head.push(c);
    }
    let mut tail: Vec<char> = Vec::new();
    for c in text.chars().rev() {
        let used: usize = tail.iter().map(|c| pitty_ui::char_width(*c)).sum();
        if width(&head) + used + pitty_ui::char_width(c) > budget {
            break;
        }
        tail.push(c);
    }
    tail.reverse();
    format!("{head}…{}", tail.into_iter().collect::<String>())
}
/// Header wordmark. Each letter is a saturated rainbow color on a light ground
/// of the same hue, so PITTY reads as a rainbow in the terminal.
fn wordmark(dark: bool) -> [(char, Style); 5] {
    let (ink, grounds) = if dark {
        (
            [
                (255, 128, 122),
                (255, 186, 92),
                (126, 220, 138),
                (122, 176, 255),
                (198, 150, 255),
            ],
            [
                (112, 42, 48),
                (112, 78, 32),
                (32, 88, 48),
                (32, 58, 112),
                (78, 42, 112),
            ],
        )
    } else {
        (
            [
                (186, 36, 42),
                (176, 92, 8),
                (18, 128, 58),
                (24, 78, 186),
                (122, 42, 176),
            ],
            [
                (255, 214, 210),
                (255, 232, 196),
                (214, 242, 220),
                (210, 230, 255),
                (236, 214, 255),
            ],
        )
    };
    let letters = ['P', 'I', 'T', 'T', 'Y'];
    let mut cells = [(letters[0], Style::default()); 5];
    for (index, letter) in letters.into_iter().enumerate() {
        cells[index] = (
            letter,
            Style::new(Some(ink[index]), Some(grounds[index]), true),
        );
    }
    cells
}
/// Replace one key line of a dotenv file, keeping every other line.
fn key_file_contents(existing: &str, key_var: &str, key: &str) -> String {
    let mut lines = Vec::new();
    let mut replaced = false;
    for line in existing.lines() {
        let entry = line.trim_start();
        let entry = entry.strip_prefix("export ").unwrap_or(entry).trim_start();
        let is_key = entry
            .strip_prefix(key_var)
            .is_some_and(|rest| rest.trim_start().starts_with('='));
        if is_key {
            if !replaced {
                lines.push(format!("{key_var}={key}"));
                replaced = true;
            }
        } else {
            lines.push(line.to_string());
        }
    }
    if !replaced {
        lines.push(format!("{key_var}={key}"));
    }
    let mut contents = lines.join("\n");
    contents.push('\n');
    contents
}
/// Whether a key source exists, mirroring `load_key` without reading the key.
fn key_configured(args: &Args) -> bool {
    if args.interface.allows_missing_key() {
        return true;
    }
    if args.key_file.is_none()
        && env::var(args.interface.key_var()).is_ok_and(|key| !key.trim().is_empty())
    {
        return true;
    }
    args.key_file
        .clone()
        .map(Ok)
        .unwrap_or_else(crate::config_path)
        .is_ok_and(|path| path.is_file())
}
fn accept_key(kind: InterfaceKind, key: &str) -> Result<(), String> {
    match kind {
        InterfaceKind::OpenRouter => {
            if !key.starts_with("sk-or-")
                || key.len() < 24
                || key
                    .chars()
                    .any(|c| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
            {
                return Err("That does not look like an OpenRouter API key.".into());
            }
        }
        InterfaceKind::OpenAI => {
            if !key.starts_with("sk-")
                || key.len() < 20
                || key
                    .chars()
                    .any(|c| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
            {
                return Err("That does not look like an OpenAI API key.".into());
            }
        }
        InterfaceKind::Compatible => {
            crate::key_value(key)?;
        }
    }
    Ok(())
}
#[derive(Clone, Serialize, Deserialize)]
struct Conversation {
    id: String,
    title: String,
    messages: Vec<Message>,
    updated: u128,
    #[serde(default = "default_model")]
    model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_name: Option<String>,
    #[serde(default)]
    actions: Vec<tools_view::ActionRecord>,
}
impl Conversation {
    fn new(model: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self {
            id: format!("{now}-{}", std::process::id()),
            title: "New chat".into(),
            messages: vec![],
            updated: now,
            model: model.into(),
            agent_name: None,
            actions: Vec::new(),
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Theme {
    Auto,
    Light,
    Dark,
}
struct Colors {
    base: Style,
    accent: Style,
    muted: Style,
    input: Style,
    selected: Style,
    error: Style,
}
impl Theme {
    /// Auto on a dark terminal: keep the terminal's own text colors, but use
    /// dark shading for the composer, code, and selections.
    fn auto_dark() -> Colors {
        Colors {
            base: Style::default(),
            accent: Style::new(None, None, true),
            muted: Style::default(),
            input: Style::new(Some((226, 232, 240)), Some((40, 46, 56)), false),
            selected: Style::new(Some((205, 226, 255)), Some((37, 61, 95)), true),
            error: Style::new(Some((255, 138, 146)), None, true),
        }
    }
    fn colors(self) -> Colors {
        match self {
            Self::Light => Colors {
                base: Style::new(Some((29, 37, 48)), Some((248, 249, 251)), false),
                accent: Style::new(Some((25, 86, 151)), Some((248, 249, 251)), true),
                muted: Style::new(Some((75, 86, 102)), Some((248, 249, 251)), false),
                input: Style::new(Some((23, 35, 50)), Some((231, 237, 245)), false),
                selected: Style::new(Some((20, 63, 115)), Some((218, 230, 247)), true),
                error: Style::new(Some((164, 39, 46)), Some((248, 249, 251)), true),
            },
            Self::Dark => Colors {
                base: Style::new(Some((225, 232, 241)), Some((20, 25, 34)), false),
                accent: Style::new(Some((119, 185, 255)), Some((20, 25, 34)), true),
                muted: Style::new(Some((169, 183, 201)), Some((20, 25, 34)), false),
                input: Style::new(Some((238, 244, 255)), Some((34, 44, 59)), false),
                selected: Style::new(Some((195, 222, 255)), Some((42, 64, 91)), true),
                error: Style::new(Some((255, 150, 154)), Some((20, 25, 34)), true),
            },
            Self::Auto => Colors {
                base: Style::default(),
                accent: Style::new(None, None, true),
                muted: Style::default(),
                input: Style::new(Some((29, 37, 48)), Some((232, 237, 242)), false),
                selected: Style::new(Some((24, 65, 111)), Some((218, 230, 242)), true),
                error: Style::new(Some((190, 65, 75)), None, true),
            },
        }
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Picker {
    Models,
    Copy,
    Files,
}

struct App {
    args: Args,
    chats: Vec<Conversation>,
    active: usize,
    input: Editor,
    pending: Option<Receiver<Result<ChatReply, String>>>,
    pending_started: Option<Instant>,
    pending_tools: bool,
    pending_stopping: bool,
    pending_actions: Vec<tools_view::ActionRecord>,
    tool_streams: Vec<tools_view::ToolStream>,
    approvals: std::collections::VecDeque<tools_view::Approval>,
    approval_painted: Option<Instant>,
    approval_size: Option<(usize, usize)>,
    approval_scroll: usize,
    profile: Result<crate::profile::Profile, String>,
    working_dir: Result<PathBuf, String>,
    agents: crate::agents::Manager,
    agents_view: bool,
    agents_panel_tick: usize,
    selection_mode: bool,
    selection_painted: bool,
    selection_previous_mouse: bool,
    mouse_surface: Option<mouse_selection::Surface>,
    mouse_selection: Option<mouse_selection::Selection>,
    transcript_cache: Option<transcript_view::Cache>,
    cancel: Option<Arc<AtomicBool>>,
    sent: String,
    attachments: Vec<pitty_core::Attachment>,
    status: String,
    scroll_back: usize,
    scroll_max: usize,
    scroll_anchor: Option<usize>,
    viewport: Option<(usize, usize, usize, usize)>,
    tick: usize,
    history_dir: Option<PathBuf>,
    key_mode: bool,
    /// A draft held while the key field asks for a missing API key.
    key_return: Option<String>,
    /// The terminal reported (or COLORFGBG implies) a dark background.
    dark_terminal: bool,
    theme: Theme,
    sidebar: bool,
    compact: bool,
    panel: Option<(String, Vec<String>)>,
    panel_scroll: usize,
    command_index: usize,
    quit: bool,
    confirm_quit: bool,
    interrupt_armed: bool,
    models: Vec<chat::Model>,
    models_rx: Option<Receiver<Result<Vec<chat::Model>, String>>>,
    picker: Option<Picker>,
    picker_index: usize,
    picker_rows: usize,
    picker_preview: usize,
    picker_draft: String,
    free_only: bool,
    folders: Result<crate::folders::Store, String>,
    folder_rx: Option<Receiver<Result<FolderResult, String>>>,
    folder_abandoned: bool,
    checked_drop: Option<String>,
    browse_root: Option<usize>,
    listing: Option<crate::folders::Listing>,
    welcome_started: Instant,
    welcome_fade: Option<Instant>,
    peer: Option<crate::peer::Link>,
    peer_room: Option<crate::peer::Room>,
    peer_open: bool,
    peer_art: bool,
}
fn history_dir() -> Option<PathBuf> {
    crate::paths::data_dir().map(|dir| dir.join("chats"))
}
impl App {
    fn new(args: Args) -> Self {
        let dir = if args.no_history { None } else { history_dir() };
        let mut chats = vec![];
        let mut loaded_bytes = 0usize;
        if let Some(dir) = &dir
            && let Ok(entries) = fs::read_dir(dir)
        {
            let mut entries: Vec<_> = entries.flatten().collect();
            entries
                .sort_by_key(|e| std::cmp::Reverse(e.metadata().and_then(|m| m.modified()).ok()));
            for entry in entries.into_iter().take(100) {
                if loaded_bytes >= 192 * 1024 * 1024 {
                    break;
                }
                if entry.path().extension().is_some_and(|s| s == "json")
                    && let Ok(file) = history_file(&entry.path())
                {
                    let mut body = String::new();
                    if file.take(92_274_689).read_to_string(&mut body).is_ok()
                        && body.len() <= 92_274_688
                        && let Ok(mut chat) = serde_json::from_str::<Conversation>(&body)
                        && !chat.id.is_empty()
                        && chat.id.chars().all(|c| c.is_ascii_digit() || c == '-')
                    {
                        loaded_bytes += body.len();
                        if chat.messages.last().is_some_and(|m| m.role == "user") {
                            let mut recovered =
                                "The previous request ended before its final reply.".to_string();
                            if !chat.actions.is_empty() {
                                recovered.push_str("\n\nRecorded local actions:\n");
                                for action in chat.actions.iter().rev().take(16).rev() {
                                    recovered.push_str(&format!(
                                        "\n{}\n{}\n",
                                        action.summary, action.output
                                    ));
                                }
                            }
                            chat.messages.push(tools_view::assistant(recovered));
                        }
                        chats.push(chat);
                    }
                }
            }
        }
        chats.sort_by_key(|c| c.updated);
        chats.push(Conversation::new(&args.model));
        let active = chats.len() - 1;
        Self {
            args,
            chats,
            active,
            input: Editor::default(),
            pending: None,
            pending_started: None,
            pending_tools: false,
            pending_stopping: false,
            pending_actions: Vec::new(),
            tool_streams: Vec::new(),
            approvals: tools_view::approvals(),
            approval_painted: None,
            approval_size: None,
            approval_scroll: 0,
            profile: crate::profile::Profile::load(),
            working_dir: env::current_dir().map_err(|e| e.to_string()),
            agents: crate::agents::Manager::default(),
            agents_view: false,
            agents_panel_tick: 0,
            selection_mode: false,
            selection_painted: false,
            selection_previous_mouse: false,
            mouse_surface: None,
            mouse_selection: None,
            transcript_cache: None,
            cancel: None,
            sent: String::new(),
            attachments: vec![],
            status: "Type a message, or / for commands.".into(),
            scroll_back: 0,
            scroll_max: 0,
            scroll_anchor: None,
            viewport: None,
            tick: 0,
            history_dir: dir,
            key_mode: false,
            key_return: None,
            dark_terminal: false,
            theme: Theme::Auto,
            sidebar: false,
            compact: false,
            panel: None,
            panel_scroll: 0,
            command_index: 0,
            quit: false,
            confirm_quit: false,
            interrupt_armed: false,
            models: vec![],
            models_rx: None,
            picker: None,
            picker_index: 0,
            picker_rows: 5,
            picker_preview: 0,
            picker_draft: String::new(),
            free_only: false,
            folders: crate::folders::Store::load(),
            folder_rx: None,
            folder_abandoned: false,
            checked_drop: None,
            browse_root: None,
            listing: None,
            welcome_started: Instant::now(),
            welcome_fade: None,
            peer: None,
            peer_room: None,
            peer_open: false,
            peer_art: false,
        }
    }
    fn save(&mut self) {
        self.save_conversation(self.active);
    }
    /// Colors for the current theme and terminal background.
    fn colors(&self) -> Colors {
        if self.theme == Theme::Auto && self.dark_terminal {
            Theme::auto_dark()
        } else {
            self.theme.colors()
        }
    }
    /// Whether text is drawn on a dark background.
    fn dark(&self) -> bool {
        self.theme == Theme::Dark || (self.theme == Theme::Auto && self.dark_terminal)
    }
    /// Next steps for an empty conversation; the key comes first until one exists.
    fn start_hints(&self) -> Vec<String> {
        let mut hints = Vec::with_capacity(5);
        if !key_configured(&self.args) {
            hints.push(match self.args.interface {
                InterfaceKind::OpenRouter => {
                    "/key           add your OpenRouter API key (openrouter.ai/keys)".into()
                }
                InterfaceKind::OpenAI => "/key           add your OpenAI API key".into(),
                InterfaceKind::Compatible => {
                    "/key           add a key if this endpoint asks for one".into()
                }
            });
        }
        hints.push(if self.args.interface.extensions() {
            "/models free   choose a model".into()
        } else {
            "/model ID      choose a model for this interface".into()
        });
        hints.push("/interface     openrouter, openai, or a compatible URL".into());
        hints.push("/help          every command and shortcut".into());
        hints.push("/peer          chat with another Pitty; nothing is stored".into());
        hints
    }
    fn save_conversation(&mut self, index: usize) -> bool {
        let Some(dir) = &self.history_dir else {
            return true;
        };
        let chat = &self.chats[index];
        if chat.messages.is_empty() {
            return true;
        }
        let result = (|| -> io::Result<()> {
            fs::create_dir_all(dir)?;
            let path = dir.join(format!("{}.json", chat.id));
            let temp = dir.join(format!(
                "{}.{}.{}.tmp",
                chat.id,
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            let write = (|| -> io::Result<()> {
                file.write_all(&serde_json::to_vec_pretty(chat)?)?;
                file.sync_all()?;
                fs::rename(&temp, &path)
            })();
            if write.is_err() {
                let _ = fs::remove_file(&temp);
            }
            write
        })();
        if let Err(error) = result {
            self.status = format!("Could not save history: {error}");
            false
        } else {
            true
        }
    }
    fn set_interface(&mut self, arg: &str) -> Result<(), String> {
        if self.pending.is_some() {
            return Err("A request is active. Ctrl+C stops it.".into());
        }
        if arg.is_empty() {
            let detail = match self.args.interface {
                InterfaceKind::Compatible => self
                    .args
                    .base_url
                    .clone()
                    .unwrap_or_else(|| "set a base URL".into()),
                kind => kind.label().into(),
            };
            self.status = format!(
                "Chat interface: {detail}. Use /interface openrouter, openai, or compatible BASE_URL."
            );
            return Ok(());
        }
        let (name, rest) = arg.split_once(char::is_whitespace).unwrap_or((arg, ""));
        let rest = rest.trim();
        let (kind, base) = match name {
            "openrouter" if rest.is_empty() => (InterfaceKind::OpenRouter, None),
            "openai" if rest.is_empty() => (InterfaceKind::OpenAI, None),
            "compatible" if !rest.is_empty() => {
                let _ = crate::interface::chat_endpoint(InterfaceKind::Compatible, Some(rest))?;
                (InterfaceKind::Compatible, Some(rest.to_string()))
            }
            "compatible" => return Err("Use /interface compatible BASE_URL.".into()),
            "openrouter" | "openai" => {
                return Err(format!("Use /interface {name} with no extra text."));
            }
            _ => {
                return Err("Use /interface openrouter, openai, or compatible BASE_URL.".into());
            }
        };
        if self.args.interface == kind && self.args.base_url.as_deref() == base.as_deref() {
            self.status = format!("Chat interface {} is already selected.", kind.label());
            return Ok(());
        }
        self.args.interface = kind;
        self.args.base_url = base;
        if !kind.extensions() {
            self.args.web = false;
        }
        self.models.clear();
        self.new_chat();
        let place = self
            .args
            .base_url
            .clone()
            .unwrap_or_else(|| kind.label().into());
        self.status = format!(
            "Chat interface {place}. Choose /model for this interface. New conversation started; no prompt sent."
        );
        Ok(())
    }
    fn new_chat(&mut self) {
        if self.pending.is_some() {
            return;
        }
        if self.chats[self.active].messages.is_empty() {
            self.chats[self.active] = Conversation::new(&self.args.model);
        } else {
            self.chats.push(Conversation::new(&self.args.model));
            self.active = self.chats.len() - 1;
        }
        // An unsent draft and staged files carry over to the new conversation.
        self.folder_abandoned = true;
        self.agents_view = false;
        self.panel = None;
        self.scroll_anchor = None;
        self.scroll_back = 0;
        self.status = if self.input.is_empty() && self.attachments.is_empty() {
            "New conversation. Type / to explore commands.".into()
        } else {
            "New conversation. Your draft and staged files are still here.".into()
        };
    }
    fn panel(&mut self, title: &str, lines: Vec<String>) {
        self.agents_view = false;
        self.panel = Some((title.into(), lines));
        self.panel_scroll = 0;
    }
    fn suggestions(&self) -> Vec<&'static (&'static str, &'static str, &'static str)> {
        let input = self.input.value();
        if self.picker.is_some()
            || !input.starts_with('/')
            || input.starts_with("//")
            || input.chars().any(char::is_whitespace)
        {
            return vec![];
        }
        let prefix = &input[1..];
        COMMANDS
            .iter()
            .filter(|(name, _, _)| name.starts_with(prefix))
            .collect()
    }
    fn transcript(&self) -> String {
        let chat = &self.chats[self.active];
        let mut text = format!("# {}\n\nModel: {}\n\n", chat.title, chat.model);
        for (index, m) in chat.messages.iter().enumerate() {
            text += &format!(
                "## {}\n\n{}\n\n",
                if m.role == "user" {
                    "You"
                } else if tools_view::is_command_result(&chat.messages, index) {
                    "Command"
                } else {
                    "Assistant"
                },
                clean(&m.content)
            );
        }
        text
    }
    fn copy(&mut self, what: &str) -> Result<(), String> {
        if what.is_empty() {
            if self.chats[self.active].messages.is_empty() {
                return Err("No messages to copy yet.".into());
            }
            self.open_picker(Picker::Copy, "");
            return Ok(());
        }
        let text = match what {
            "last" => self.chats[self.active]
                .messages
                .iter()
                .rev()
                .find(|m| m.role == "assistant")
                .map(|m| m.content.clone())
                .ok_or("No assistant reply to copy yet.")?,
            "chat" => self.transcript(),
            "draft" if self.sent.is_empty() => {
                return Err("No sent prompt to copy yet.".into());
            }
            "draft" => self.sent.clone(),
            _ => crate::transcript::copy_items(&self.chats[self.active].messages)
                .into_iter()
                .find(|item| item.command == what)
                .map(|item| item.text)
                .ok_or("Use /copy to choose an element, /copy message N, or /copy code N.")?,
        };
        self.copy_text(&text)
    }
    fn copy_text(&mut self, text: &str) -> Result<(), String> {
        let text = pitty_ui::sanitize_copy(text);
        // Never replace the user's clipboard with nothing.
        if text.is_empty() {
            return Err("Nothing to copy.".into());
        }
        let commands: Vec<(&str, Vec<&str>)> = if cfg!(target_os = "macos") {
            vec![("pbcopy", vec![])]
        } else {
            vec![
                ("wl-copy", vec![]),
                ("xclip", vec!["-selection", "clipboard"]),
            ]
        };
        for (program, args) in commands {
            if let Ok(mut child) = Command::new(program)
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                let wrote = child
                    .stdin
                    .take()
                    .is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
                let success = child.wait().is_ok_and(|status| status.success());
                if wrote && success {
                    self.status = format!(
                        "Copied {} characters to the clipboard.",
                        text.chars().count()
                    );
                    return Ok(());
                }
            }
        }
        Err("Clipboard unavailable. Use /export PATH or select text in your terminal.".into())
    }
    fn open_picker(&mut self, picker: Picker, query: &str) {
        self.picker_draft = self.input.value();
        self.picker = Some(picker);
        self.picker_index = 0;
        self.picker_preview = 0;
        self.panel = None;
        self.agents_view = false;
        self.free_only = picker == Picker::Models && query == "free";
        self.input.set(if self.free_only { "" } else { query });
        self.status = match picker {
            Picker::Models => "↑↓ choose · Enter select · Tab free/all · Ctrl+R refresh · Esc back",
            Picker::Copy => "↑↓ choose · Enter copy · PgUp/PgDn preview · Esc back",
            Picker::Files => "↑↓ choose · Enter open/attach · ← parent · Ctrl+R refresh · Esc back",
        }
        .into();
    }
    fn close_picker(&mut self) {
        if self.picker == Some(Picker::Files) {
            self.folder_abandoned = true;
            self.status = if self.folder_rx.is_some() {
                "Stopped waiting for the folder. Your draft is ready; the drive operation may still be finishing.".into()
            } else if self.attachments.is_empty() {
                "Ready · /files browses connected folders.".into()
            } else {
                format!(
                    "{} files staged · /attachments to review · Enter sends with your message.",
                    self.attachments.len()
                )
            };
        }
        if self.models_rx.is_some() {
            self.cancel_request();
        }
        self.picker = None;
        self.input.set(&self.picker_draft);
        self.picker_draft.clear();
    }
    fn model_matches(&self) -> Vec<usize> {
        let query = self.input.value().to_lowercase();
        self.models
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                m.supports_chat()
                    && (!self.free_only || m.is_free())
                    && format!("{} {}", m.id, m.name)
                        .to_lowercase()
                        .contains(&query)
            })
            .map(|(i, _)| i)
            .collect()
    }
    fn copy_matches(&self) -> Vec<crate::transcript::CopyItem> {
        let query = self.input.value().to_lowercase();
        crate::transcript::copy_items(&self.chats[self.active].messages)
            .into_iter()
            .filter(|item| {
                item.label.to_lowercase().contains(&query)
                    || item.text.to_lowercase().contains(&query)
            })
            .collect()
    }
    fn load_models(&mut self) {
        if self.models_rx.is_some() {
            return;
        }
        let url = match self.args.models_endpoint() {
            Ok(url) => url,
            Err(error) => {
                self.status = error;
                return;
            }
        };
        let (tx, rx) = mpsc::channel();
        self.models_rx = Some(rx);
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        std::thread::spawn(move || {
            let _ = tx.send(chat::models(&url, cancel));
        });
        self.status = "Loading the model catalog… Esc cancels.".into();
    }
    fn handle_picker(&mut self, key: Key) {
        if self.picker == Some(Picker::Files) {
            self.handle_files(key);
            return;
        }
        let count = if self.picker == Some(Picker::Models) {
            self.model_matches().len()
        } else {
            self.copy_matches().len()
        };
        match key {
            Key::Escape => self.close_picker(),
            Key::Up => {
                self.picker_index = self.picker_index.saturating_sub(1);
                self.picker_preview = 0;
            }
            Key::Down => {
                self.picker_index = (self.picker_index + 1).min(count.saturating_sub(1));
                self.picker_preview = 0;
            }
            Key::PageUp | Key::PageDown if self.picker == Some(Picker::Copy) => {
                self.picker_preview = if key == Key::PageUp {
                    self.picker_preview.saturating_sub(5)
                } else {
                    self.picker_preview.saturating_add(5)
                };
            }
            Key::PageUp => self.picker_index = self.picker_index.saturating_sub(self.picker_rows),
            Key::PageDown => {
                self.picker_index =
                    (self.picker_index + self.picker_rows).min(count.saturating_sub(1))
            }
            Key::Tab if self.picker == Some(Picker::Models) => {
                self.free_only = !self.free_only;
                self.picker_index = 0;
            }
            Key::Ctrl('r') if self.picker == Some(Picker::Models) => self.load_models(),
            Key::Enter => {
                if self.picker == Some(Picker::Models) {
                    if let Some(&index) = self.model_matches().get(self.picker_index) {
                        let id = self.models[index].id.clone();
                        self.close_picker();
                        if let Err(e) = self.command("model", &id) {
                            self.status = e;
                        }
                    }
                } else if let Some(item) = self.copy_matches().get(self.picker_index) {
                    let text = item.text.clone();
                    match self.copy_text(&text) {
                        Ok(()) => self.close_picker(),
                        Err(e) => self.status = e,
                    }
                }
            }
            Key::PasteRejected => self.status = "Paste exceeds 1 MB; nothing inserted.".into(),
            other => {
                self.input.edit(&other);
                self.picker_index = 0;
                self.picker_preview = 0;
            }
        }
    }
    fn command(&mut self, name: &str, arg: &str) -> Result<(), String> {
        match name {
            "tools" | "run" | "exec" | "actions" | "audit" => self.tool_command(name, arg)?,
            "agent" | "agents" => self.task_command(name, arg)?,
            "pwd" => {
                let mut lines = vec![
                    self.working_dir
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|e| format!("Working folder unavailable: {e}")),
                    String::new(),
                    "Connected folders:".into(),
                ];
                if let Ok(store) = &self.folders {
                    lines.extend(store.roots.iter().map(|f| f.path.display().to_string()));
                }
                self.panel("Working directory", lines);
            }
            "name" | "team" => {
                let profile = self.profile.as_mut().map_err(|e| e.clone())?;
                if !arg.is_empty() {
                    if name == "name" {
                        profile.set_name(arg)?;
                    } else {
                        profile.set_team(arg)?;
                    }
                }
                self.status = format!(
                    "Name: {} · Team: {} · display labels stay local.",
                    profile.name,
                    if profile.team.is_empty() {
                        "none"
                    } else {
                        &profile.team
                    }
                );
            }
            "select" => self.toggle_selection(),
            "connect" | "disconnect" | "folders" | "files" => self.folder_command(name, arg)?,
            "attach" => {
                if arg.is_empty() {
                    return Err("Use /attach PATH, or drag a file into the composer.".into());
                }
                self.attach_text_in_background(arg.to_string())?;
            }
            "attachments" => self.panel(
                "Staged attachments · /detach NUMBER or /detach all",
                self.attachments
                    .iter()
                    .enumerate()
                    .map(|(i, f)| {
                        format!(
                            "{}  {} · {} · {} KiB encoded",
                            i + 1,
                            f.name,
                            f.kind,
                            f.data.len() / 1024
                        )
                    })
                    .collect(),
            ),
            "detach" => {
                if arg == "all" {
                    self.attachments.clear();
                } else {
                    let index = arg
                        .parse::<usize>()
                        .ok()
                        .filter(|n| *n > 0 && *n <= self.attachments.len())
                        .ok_or("Use /detach NUMBER from /attachments, or /detach all.")?;
                    self.attachments.remove(index - 1);
                }
                self.status = "Staged attachments updated.".into();
            }
            "key" => {
                if !arg.is_empty() {
                    return Err("Type /key alone, then paste into the masked field.".into());
                }
                self.key_mode = true;
                self.input.clear();
                self.status = match self.args.interface {
                    InterfaceKind::OpenRouter => {
                        "Paste your OpenRouter API key. Enter saves; Esc cancels.".into()
                    }
                    InterfaceKind::OpenAI => {
                        "Paste your OpenAI API key. Enter saves; Esc cancels.".into()
                    }
                    InterfaceKind::Compatible => {
                        "Paste the key for this endpoint. Enter saves; Esc cancels.".into()
                    }
                };
            }
            "interface" => self.set_interface(arg)?,
            "help" => {
                let mut lines = vec![
                    "COMMANDS — use ↑/↓ to scroll; Esc to return".into(),
                    String::new(),
                ];
                for (n, a, d) in COMMANDS {
                    lines.push(format!("/{n} {a}"));
                    lines.push(format!("  {d}"));
                }
                lines.extend([
                    String::new(),
                    "Enter send · Ctrl+J or Alt+Enter newline · Ctrl+U clear draft".into(),
                    "Ctrl+K deletes to the end of the line · Ctrl+L redraws the screen".into(),
                    "Drag over chat text · click [Copy selection] or Ctrl+Y".into(),
                    "Esc clears a selection · F2 uses native terminal selection".into(),
                    "Ctrl+C stop; press again to request closing".into(),
                    "In a peer room, Enter talks to people; arrows move ASCII art.".into(),
                    "// starts a message with a literal slash".into(),
                ]);
                self.panel("Help", lines);
            }
            "new" => self.new_chat(),
            "chats" => {
                let lines = self
                    .chats
                    .iter()
                    .enumerate()
                    .map(|(i, c)| format!("{}  {}  [{}]", i + 1, c.title, c.model))
                    .collect();
                self.panel("Conversations — /open NUMBER", lines);
            }
            "open" => {
                let n = arg
                    .parse::<usize>()
                    .map_err(|_| "Use /open NUMBER from /chats.")?;
                if n == 0 || n > self.chats.len() {
                    return Err("Conversation number is out of range.".into());
                }
                self.agents_view = false;
                self.active = n - 1;
                self.args.model = self.chats[self.active].model.clone();
                self.agents_view = false;
                self.panel = None;
                self.scroll_anchor = None;
                self.scroll_back = 0;
                self.status = format!("Opened {}", self.chats[self.active].title);
            }
            "rename" => {
                if arg.is_empty() {
                    return Err("Use /rename TITLE.".into());
                }
                self.chats[self.active].title = clean(arg).chars().take(80).collect();
                self.status = "Conversation renamed.".into();
                self.save();
            }
            "delete" | "clear" => {
                if arg != "confirm" {
                    return Err(format!(
                        "Use /{name} confirm to {} this conversation.",
                        if name == "delete" { "delete" } else { "clear" }
                    ));
                }
                // Remove the saved copy even if /history off was used later
                // in this session; a deleted chat must not return on restart.
                if let Some(dir) = self.history_dir.clone().or_else(history_dir) {
                    let p = dir.join(format!("{}.json", self.chats[self.active].id));
                    match fs::remove_file(p) {
                        Ok(()) => {}
                        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e.to_string()),
                    }
                }
                if name == "delete" {
                    self.chats.remove(self.active);
                    if self.chats.is_empty() {
                        self.chats.push(Conversation::new(&self.args.model));
                    }
                    self.active = self.active.min(self.chats.len() - 1);
                    self.args.model = self.chats[self.active].model.clone();
                } else {
                    self.chats[self.active] = Conversation::new(&self.args.model);
                }
                self.agents_view = false;
                self.panel = None;
                self.scroll_anchor = None;
                self.scroll_back = 0;
                self.status = format!(
                    "Conversation {}.",
                    if name == "delete" {
                        "deleted"
                    } else {
                        "cleared"
                    }
                );
            }
            "copy" => self.copy(arg)?,
            "export" => {
                if arg.is_empty() {
                    return Err(
                        "Use /export PATH.md. Existing files will not be overwritten.".into(),
                    );
                }
                // Accept ~, quotes, and escaped spaces like /attach and /connect.
                let paths = crate::attachments::path_tokens(arg)?;
                let [path] = paths.as_slice() else {
                    return Err("Use /export with one path; quote a path that has spaces.".into());
                };
                let mut options = fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut f = options
                    .open(path)
                    .map_err(|e| format!("Cannot export to {}: {e}", path.display()))?;
                f.write_all(self.transcript().as_bytes())
                    .map_err(|e| e.to_string())?;
                self.status = format!("Exported to {}", path.display());
            }
            "save" => {
                if self.history_dir.is_none() {
                    return Err("History saving is off. Use /history on first.".into());
                }
                self.status = "Conversation saved.".into();
                self.save();
            }
            "history" => {
                match arg {
                    "on" => {
                        self.history_dir = history_dir();
                        self.args.no_history = false;
                    }
                    "off" => {
                        self.history_dir = None;
                        self.args.no_history = true;
                    }
                    "" => {}
                    _ => return Err("Use /history on or /history off.".into()),
                }
                self.status = format!(
                    "History saving: {}",
                    if self.history_dir.is_some() {
                        "on"
                    } else {
                        "off"
                    }
                );
            }
            "tokens" => {
                if !arg.is_empty() {
                    self.args.max_tokens = arg
                        .parse::<u32>()
                        .ok()
                        .filter(|v| *v > 0)
                        .ok_or("Token limit must be a positive integer.")?;
                }
                self.status = if self.args.interface.extensions() {
                    format!(
                        "Output limit: {} tokens. Your key's dollar limit is configured in OpenRouter.",
                        self.args.max_tokens
                    )
                } else {
                    format!(
                        "Output limit: {} tokens. The token cap is not a dollar budget.",
                        self.args.max_tokens
                    )
                };
            }
            "timeout" => {
                if !arg.is_empty() {
                    self.args.timeout = arg
                        .parse::<u64>()
                        .ok()
                        .filter(|v| *v > 0 && *v <= 3600)
                        .ok_or("Timeout must be 1–3600 seconds.")?;
                }
                self.status = format!("Request timeout: {} seconds", self.args.timeout);
            }
            "system" => {
                match arg {
                    "" => {}
                    "off" => self.args.system = None,
                    _ => self.args.system = Some(arg.into()),
                }
                self.panel(
                    "System instruction",
                    vec![
                        self.args
                            .system
                            .clone()
                            .unwrap_or_else(|| "No system instruction set.".into()),
                    ],
                );
            }
            "model" => {
                if arg.is_empty() {
                    self.open_picker(Picker::Models, "");
                    if self.models.is_empty() {
                        self.load_models();
                    }
                } else {
                    let id = if let Ok(n) = arg.parse::<usize>() {
                        self.models
                            .get(n.saturating_sub(1))
                            .filter(|_| n > 0)
                            .map(|m| m.id.clone())
                            .ok_or("Use a number shown in /models.")?
                    } else {
                        arg.into()
                    };
                    crate::interface::check_model_id(self.args.interface, &id)?;
                    if self.models.iter().any(|m| m.id == id && !m.supports_chat()) {
                        return Err("This model uses a specialized endpoint, not text chat. Choose a chat model.".into());
                    }
                    if self.args.model == id {
                        self.status =
                            "This model is already selected; conversation preserved.".into();
                        return Ok(());
                    }
                    self.args.model = id;
                    let draft = self.input.value();
                    let attachments = std::mem::take(&mut self.attachments);
                    self.new_chat();
                    self.input.set(&draft);
                    self.attachments = attachments;
                    self.status = format!(
                        "Selected {}. New conversation started; no prompt sent.",
                        self.args.model
                    );
                }
            }
            "models" => {
                self.open_picker(Picker::Models, arg);
                if self.models.is_empty() {
                    self.load_models();
                }
            }
            "web" => {
                if !self.args.interface.extensions() {
                    return Err(
                        "Web search belongs to the OpenRouter interface. Use /interface openrouter."
                            .into(),
                    );
                }
                self.args.web = match arg {
                    "on" => true,
                    "off" => false,
                    "" => !self.args.web,
                    _ => return Err("Use /web on or /web off.".into()),
                };
                self.status = if self.args.web {
                    "Web search ON · searches may add charges. Applies to the next message."
                } else {
                    "Web search OFF · no search tool will be sent."
                }
                .into();
            }
            "status" => self.panel(
                "Session settings",
                vec![
                    format!(
                        "Chat interface: {}{}",
                        self.args.interface.label(),
                        self.args
                            .base_url
                            .as_deref()
                            .map(|url| format!(" · {url}"))
                            .unwrap_or_default()
                    ),
                    format!("Model: {}", self.args.model),
                    format!("Local tools: {}", self.args.tools.label()),
                    format!(
                        "Web search: {}",
                        if self.args.web {
                            "on (additional charges may apply)"
                        } else {
                            "off"
                        }
                    ),
                    format!("Output limit: {} tokens", self.args.max_tokens),
                    format!("Timeout: {} seconds", self.args.timeout),
                    format!("Messages: {}", self.chats[self.active].messages.len()),
                    format!(
                        "Saved history: {}",
                        self.history_dir
                            .as_ref()
                            .map_or("off".into(), |d| d.display().to_string())
                    ),
                    "Interface: Pitty (Rust, zero dependencies)".into(),
                    "Commands and validation: embedded Rust WebAssembly".into(),
                ],
            ),
            "theme" => {
                self.theme = match arg {
                    "auto" => Theme::Auto,
                    "light" => Theme::Light,
                    "dark" => Theme::Dark,
                    "" => {
                        self.status = "Themes: /theme auto, /theme light, /theme dark".into();
                        return Ok(());
                    }
                    _ => return Err("Use /theme auto, light, or dark.".into()),
                };
                self.status = format!("Theme: {arg}");
            }
            "sidebar" => {
                self.sidebar = match arg {
                    "show" => true,
                    "hide" => false,
                    "" => !self.sidebar,
                    _ => return Err("Use /sidebar show or hide.".into()),
                };
                self.status = "Layout updated.".into();
            }
            "mouse" => {
                self.args.no_mouse = match arg {
                    "on" => false,
                    "off" => true,
                    "" => !self.args.no_mouse,
                    _ => return Err("Use /mouse on or /mouse off.".into()),
                };
                self.status = if self.args.no_mouse { "Mouse reporting off. Native selection enabled; PgUp/PgDn scroll chat." } else { "Mouse scrolling on. Scroll over the chat; /mouse off restores native selection." }.into();
            }
            "compact" => {
                self.compact = match arg {
                    "on" => true,
                    "off" => false,
                    "" => !self.compact,
                    _ => return Err("Use /compact on or off.".into()),
                };
                self.status = "Message spacing updated.".into();
            }
            "top" => {
                self.scroll_anchor = None;
                self.scroll_back = usize::MAX;
            }
            "bottom" => {
                self.scroll_anchor = None;
                self.scroll_back = 0;
            }
            "search" => {
                if arg.is_empty() {
                    return Err("Use /search WORDS.".into());
                }
                let query = arg.to_lowercase();
                let lines = self.chats[self.active]
                    .messages
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| m.content.to_lowercase().contains(&query))
                    .map(|(i, m)| format!("Message {} · {}\n{}", i + 1, m.role, clean(&m.content)))
                    .collect();
                self.panel("Search results", lines);
            }
            "retry" => {
                if self.sent.is_empty() {
                    return Err("No previous prompt to retry.".into());
                }
                self.input.set(&if self.sent.starts_with('/') {
                    format!("/{}", self.sent)
                } else {
                    self.sent.clone()
                });
                self.send();
            }
            "peer" => self.peer_command(arg)?,
            "quit" => self.confirm_quit = true,
            _ => return Err("Unknown command. Use /help.".into()),
        }
        Ok(())
    }
    fn peer_name(&self) -> String {
        self.profile
            .as_ref()
            .ok()
            .map(|profile| profile.name.trim())
            .filter(|name| !name.is_empty())
            .unwrap_or("pitty")
            .chars()
            .take(40)
            .collect()
    }
    fn peer_connected(&self) -> bool {
        self.peer
            .as_ref()
            .is_some_and(crate::peer::Link::is_connected)
    }
    fn ensure_peer(&self) -> Result<(), String> {
        if self.peer_connected() {
            Ok(())
        } else {
            Err("Host or join a room first. /peer host starts one; /peer join ROOM walks in. Nothing is stored on a server.".into())
        }
    }
    fn begin_peer(&mut self, link: crate::peer::Link, status: String) {
        self.peer = Some(link);
        self.peer_room = None;
        self.peer_open = true;
        self.peer_art = false;
        self.panel = None;
        self.agents_view = false;
        self.status = status;
    }
    fn peer_shutdown(&mut self) {
        self.peer_open = false;
        self.peer_art = false;
        self.peer_room = None;
        if let Some(link) = self.peer.take() {
            drop(link);
        }
    }
    fn peer_command(&mut self, arg: &str) -> Result<(), String> {
        let arg = arg.trim();
        let (verb, rest) = if arg.is_empty() {
            ("", "")
        } else {
            arg.split_once(char::is_whitespace).unwrap_or((arg, ""))
        };
        let rest = rest.trim();
        match verb {
            "" => self.open_peer(),
            "host" => self.host_peer(rest),
            "join" => self.join_peer(rest),
            "leave" => {
                if self.peer.is_none() && !self.peer_open {
                    self.status = "No peer room. Nothing is stored on a server.".into();
                    return Ok(());
                }
                self.peer_shutdown();
                self.status = "Left the room. Nothing was stored.".into();
                Ok(())
            }
            "art" if rest.is_empty() => {
                self.ensure_peer()?;
                self.peer_open = true;
                self.peer_art = true;
                self.panel = None;
                self.input.clear();
                self.status = "ASCII art · Alt+Enter newline · Enter places · Esc cancels".into();
                Ok(())
            }
            "art" => {
                self.ensure_peer()?;
                let rows = crate::peer::builtin(rest).ok_or_else(|| {
                    "Use /peer art, or /peer art cat, ship, crab, or spark.".to_string()
                })?;
                let name = rest.to_ascii_lowercase();
                self.peer
                    .as_ref()
                    .ok_or_else(|| {
                        "Host or join a room first. Nothing is stored on a server.".to_string()
                    })?
                    .place(&name, rows)?;
                self.peer_open = true;
                self.peer_art = false;
                self.panel = None;
                self.status = format!("Shared {name}. Arrows move it when the draft is empty.");
                Ok(())
            }
            "move" => {
                self.ensure_peer()?;
                let (dx, dy) = crate::peer::direction(rest).ok_or_else(|| {
                    "Use /peer move left, right, up, down, h, j, k, or l.".to_string()
                })?;
                self.nudge_selected(dx, dy)?;
                Ok(())
            }
            "pick" => self.pick_peer(rest),
            _ => Err("Use /peer, /peer host, /peer join ROOM, /peer art, /peer move, /peer pick N, or /peer leave.".into()),
        }
    }
    fn open_peer(&mut self) -> Result<(), String> {
        if self.peer.as_ref().is_some_and(|link| !link.is_finished()) {
            self.peer_open = true;
            self.peer_art = false;
            self.panel = None;
            self.status = "Peer room open. Nothing is stored on a server.".into();
            return Ok(());
        }
        let link = crate::peer::Link::discover(&self.peer_name())?;
        self.begin_peer(
            link,
            "Listening for nearby Pitty terminals. Nothing is stored on a server.".into(),
        );
        Ok(())
    }
    fn host_peer(&mut self, rest: &str) -> Result<(), String> {
        if self
            .peer
            .as_ref()
            .is_some_and(|link| link.is_connected() && link.role() == crate::peer::RoleKind::Host)
        {
            self.peer_open = true;
            self.peer_art = false;
            self.panel = None;
            if let Some(link) = self.peer.as_ref() {
                self.status = peer_host_status(link);
            }
            return Ok(());
        }
        if self.peer_connected() {
            return Err("Leave this room first with /peer leave.".into());
        }
        let link = crate::peer::Link::host(&self.peer_name(), rest)?;
        let status = peer_host_status(&link);
        self.begin_peer(link, status);
        Ok(())
    }
    fn join_peer(&mut self, rest: &str) -> Result<(), String> {
        if rest.is_empty() {
            return Err("Use /peer join ROOM.".into());
        }
        if self.peer_connected() {
            return Err("Leave this room first with /peer leave.".into());
        }
        if rest.chars().all(|ch| ch.is_ascii_digit()) {
            let number: u16 = rest
                .parse()
                .map_err(|_| "Use a room number from 1 to 999.".to_string())?;
            let addr = self
                .peer_room
                .as_ref()
                .and_then(|room| {
                    room.nearby
                        .iter()
                        .find(|near| near.number == number)
                        .map(|near| near.address.clone())
                })
                .map(Ok)
                .unwrap_or_else(|| crate::peer::locate_room(number))?;
            let link = crate::peer::Link::join_room(&self.peer_name(), &addr, number)
                .map_err(|_| format!("Cannot join room {number}."))?;
            self.begin_peer(
                link,
                format!("Joined room {number}. Nothing is stored on a server."),
            );
            return Ok(());
        }
        if rest.contains(':') {
            let link = crate::peer::Link::join(&self.peer_name(), rest)
                .map_err(|_| "Cannot join that room.".to_string())?;
            self.begin_peer(
                link,
                "Joined the room. Nothing is stored on a server.".into(),
            );
            return Ok(());
        }
        Err("Use /peer join ROOM.".into())
    }
    fn pick_peer(&mut self, rest: &str) -> Result<(), String> {
        self.ensure_peer()?;
        let number: usize = rest
            .parse()
            .map_err(|_| "Use /peer pick N from the drawing list.".to_string())?;
        let id = self
            .peer_room
            .as_ref()
            .and_then(|room| room.arts.get(number.wrapping_sub(1)))
            .map(|art| art.id)
            .ok_or_else(|| "That drawing is not in the room.".to_string())?;
        if let Some(room) = &mut self.peer_room {
            room.selected = Some(id);
        }
        if let Some(link) = self.peer.as_ref() {
            link.pick(id)?;
        }
        self.peer_open = true;
        self.status = format!("Selected drawing {number}. Arrows move it.");
        Ok(())
    }
    fn nudge_selected(&mut self, dx: i16, dy: i16) -> Result<(), String> {
        let id = self
            .peer_room
            .as_ref()
            .and_then(|room| room.selected)
            .ok_or_else(|| "Pick a drawing with /peer pick N.".to_string())?;
        let sent = self.peer.as_ref().map(|link| link.nudge(id, dx, dy));
        match sent {
            Some(Ok(())) => {}
            Some(Err(error)) => return Err(error),
            None => {
                return Err("Host or join a room first. Nothing is stored on a server.".into());
            }
        }
        if let Some(room) = &mut self.peer_room
            && let Some(index) = room.arts.iter().position(|art| art.id == id)
        {
            let (x, y) = crate::peer::nudge(room.arts[index].x, room.arts[index].y, dx, dy);
            let mut art = room.arts.remove(index);
            art.x = x;
            art.y = y;
            room.arts.push(art);
        }
        self.peer_open = true;
        Ok(())
    }
    fn place_peer_art(&mut self, text: &str) {
        let rows: Vec<String> = text.split('\n').map(str::to_string).collect();
        match crate::peer::prepare_rows(&rows) {
            Ok(rows) => {
                let placed = self.peer.as_ref().map(|link| link.place("art", rows));
                match placed {
                    Some(Ok(())) => {
                        self.input.clear();
                        self.peer_art = false;
                        self.peer_open = true;
                        self.status = "Placed. Arrows move it when the draft is empty.".into();
                    }
                    Some(Err(error)) => self.status = error,
                    None => {
                        self.status = "Host or join a room before sharing art.".into();
                    }
                }
            }
            Err(error) => self.status = error,
        }
    }
    fn say_to_peer(&mut self, text: &str) {
        if !self.attachments.is_empty() {
            self.status =
                "Peer chat sends text only. /detach files before messaging the room.".into();
            return;
        }
        if text.trim().is_empty() {
            return;
        }
        match self.peer.as_ref().map(|link| link.say(text)) {
            Some(Ok(())) => {
                self.input.clear();
                self.peer_open = true;
                self.status = "Sent to the room.".into();
            }
            Some(Err(error)) => self.status = error,
            None => {
                self.status = "Host or join a room first. Nothing is stored on a server.".into();
            }
        }
    }
    fn peer_arrows(&self) -> bool {
        self.peer_open
            && !self.peer_art
            && self.input.is_empty()
            && self.panel.is_none()
            && self.picker.is_none()
            && self.suggestions().is_empty()
            && self.peer_connected()
            && self
                .peer_room
                .as_ref()
                .is_some_and(|room| room.connected() && room.selected.is_some())
    }
    fn receive_peer(&mut self) -> bool {
        let Some(link) = self.peer.as_ref() else {
            return false;
        };
        let update = link.poll();
        if update.room.is_none() && update.notes.is_empty() {
            return false;
        }
        let was_closed = self.peer_room.as_ref().is_some_and(|room| room.closed);
        let mut hidden_say = None;
        if let Some(room) = update.room {
            let previous = self.peer_room.as_ref().and_then(|existing| {
                existing
                    .says
                    .last()
                    .map(|say| (say.by.clone(), say.text.clone()))
            });
            let latest = room
                .says
                .last()
                .map(|say| (say.by.clone(), say.text.clone()));
            if !self.peer_open && latest != previous {
                hidden_say = latest;
            }
            let closed = room.closed;
            self.peer_room = Some(room);
            if closed && !was_closed && update.notes.is_empty() {
                self.status = "The host left. The room ended.".into();
            }
        }
        if let Some(note) = update.notes.last() {
            self.status = note.clone();
        } else if let Some((by, text)) = hidden_say {
            let preview: String = text.chars().take(80).collect();
            self.status = format!("Peer · {by}: {preview} — /peer opens the room");
        }
        true
    }
    fn draw_peer(
        &self,
        frame: &mut Frame,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        colors: &Colors,
    ) {
        if w == 0 || h == 0 {
            return;
        }
        frame.text(
            x,
            y,
            "PEER · nothing is stored on a server",
            colors.accent,
            w,
        );
        let Some(room) = self.peer_room.as_ref() else {
            if h > 2 {
                frame.text(x, y + 2, "Opening the room…", colors.muted, w);
            }
            return;
        };
        if h == 1 {
            return;
        }
        let people = if room.peers.len() <= 1 {
            "Just you".to_string()
        } else {
            format!("With {}", room.peers.join(", "))
        };
        let mut row = 1;
        if room.number > 0 || room.connected() {
            let heading = if room.number > 0 {
                format!("Room {} · {people}", room.number)
            } else {
                people
            };
            frame.text(x, y + row, &heading, colors.muted, w);
            row += 1;
        }
        if row >= h {
            return;
        }
        if room.connected() {
            if room.peers.len() <= 1 && room.number > 0 {
                frame.text(
                    x,
                    y + row,
                    &format!("Another Pitty joins with /peer join {}", room.number),
                    colors.base,
                    w,
                );
                row += 1;
            }
            let ink = Self::ink_rows(&crate::peer::canvas(&room.arts));
            let min_chat = if room.says.is_empty() {
                1
            } else {
                room.says.len().clamp(1, 6)
            };
            let art_budget = h.saturating_sub(row + min_chat + 1);
            let art_show = if room.arts.is_empty() {
                0
            } else {
                ink.len().min(6).min(art_budget.saturating_sub(1))
            };
            let art_block = if room.arts.is_empty() || art_show == 0 {
                0
            } else {
                1 + art_show
            };
            let chat_rows = h.saturating_sub(row + art_block);
            Self::draw_peer_says(frame, x, y + row, w, chat_rows, room, colors);
            row += chat_rows;
            if art_block > 0 && row < h {
                let listing = room
                    .arts
                    .iter()
                    .enumerate()
                    .map(|(index, art)| {
                        let mark = if room.selected == Some(art.id) {
                            "›"
                        } else {
                            " "
                        };
                        format!("{mark}{} {} ({},{})", index + 1, art.name, art.x, art.y)
                    })
                    .collect::<Vec<_>>()
                    .join("  ");
                frame.text(x, y + row, &listing, colors.muted, w);
                row += 1;
                let rainbow = if self.dark() {
                    [
                        (255, 117, 131),
                        (255, 181, 91),
                        (178, 215, 85),
                        (95, 190, 255),
                        (198, 149, 255),
                    ]
                } else {
                    [
                        (191, 41, 58),
                        (164, 82, 5),
                        (49, 125, 56),
                        (33, 100, 178),
                        (133, 57, 164),
                    ]
                };
                let start = ink.len().saturating_sub(art_show);
                for (index, line) in ink.iter().skip(start).take(art_show).enumerate() {
                    if row + index >= h {
                        break;
                    }
                    frame.text(
                        x,
                        y + row + index,
                        line,
                        Style::new(Some(rainbow[index % 5]), colors.base.bg, true),
                        w,
                    );
                }
            }
            return;
        }
        frame.text(x, y + row, "Rooms", colors.accent, w);
        row += 1;
        if room.nearby.is_empty() {
            let hint = if room.discovery {
                "None yet. /peer host starts a room. /peer join ROOM walks in."
            } else {
                "Nearby rooms are not visible. /peer host starts one. /peer join ROOM walks in."
            };
            if row < h {
                frame.text(x, y + row, hint, colors.muted, w);
            }
            return;
        }
        for (index, near) in room.nearby.iter().enumerate() {
            if row + index >= h {
                break;
            }
            frame.text(
                x,
                y + row + index,
                &format!("  {}  {}", near.number, near.name),
                colors.base,
                w,
            );
        }
    }
    fn ink_rows(stage: &[String]) -> Vec<String> {
        let Some(start) = stage.iter().position(|line| !line.trim().is_empty()) else {
            return Vec::new();
        };
        let end = stage
            .iter()
            .rposition(|line| !line.trim().is_empty())
            .unwrap_or(start);
        stage[start..=end].to_vec()
    }
    fn draw_peer_says(
        frame: &mut Frame,
        x: usize,
        y: usize,
        w: usize,
        rows: usize,
        room: &crate::peer::Room,
        colors: &Colors,
    ) {
        if rows == 0 || w == 0 {
            return;
        }
        if room.says.is_empty() {
            frame.text(
                x,
                y,
                "No messages yet. Type below and press Enter.",
                colors.muted,
                w,
            );
            return;
        }
        let mut lines: Vec<(Style, String, Style, String)> = Vec::new();
        for say in &room.says {
            let style = if say.by == "room" {
                colors.muted
            } else {
                colors.accent
            };
            let prefix = format!("{} · ", say.by);
            let prefix_w = width(&prefix);
            let body_w = w.saturating_sub(prefix_w).max(1);
            let wrapped = wrap(&say.text, body_w);
            let first = wrapped.first().map(String::as_str).unwrap_or("");
            lines.push((style, prefix, colors.base, first.to_string()));
            for extra in wrapped.iter().skip(1) {
                lines.push((
                    colors.base,
                    " ".repeat(prefix_w.min(w)),
                    colors.base,
                    extra.clone(),
                ));
            }
        }
        let start = lines.len().saturating_sub(rows);
        for (index, (name_style, prefix, body_style, body)) in lines.iter().skip(start).enumerate()
        {
            frame.text(x, y + index, prefix, *name_style, w);
            let prefix_w = width(prefix);
            if prefix_w < w {
                frame.text(x + prefix_w, y + index, body, *body_style, w - prefix_w);
            }
        }
    }
    fn send(&mut self) {
        let control = self.input.value();
        let control = control.trim();
        if self.pending.is_some()
            && (control == "/actions"
                || control == "/agents"
                || control.starts_with("/agent stop "))
        {
            let rest = control.strip_prefix('/').unwrap();
            let (command, arg) = rest.split_once(' ').unwrap_or((rest, ""));
            let command = command.to_string();
            let arg = arg.to_string();
            self.input.clear();
            if let Err(e) = self.command(&command, &arg) {
                self.status = e;
            }
            return;
        }
        let room_message = self.peer_art
            || (self.peer_connected() && (!control.starts_with('/') || control.starts_with("//")));
        let peer_command = control == "/peer" || control.starts_with("/peer ");
        if !room_message
            && !peer_command
            && (self.pending.is_some()
                || self.models_rx.is_some()
                || (self.folder_rx.is_some() && !self.folder_abandoned))
        {
            self.status = "A request is active. Ctrl+C stops it.".into();
            return;
        }
        if self.peer_art {
            let raw = self.input.value();
            if raw.trim().is_empty() {
                self.status = "Draw at least one line. Alt+Enter starts a new line.".into();
                return;
            }
            self.place_peer_art(&raw);
            return;
        }
        let raw = self.input.value();
        let text = raw.trim();
        if crate::attachments::looks_like_drop(text)
            && self.checked_drop.as_deref() != Some(raw.as_str())
        {
            match self.drop_in_background(raw.clone()) {
                Ok(()) => self.input.clear(),
                Err(e) => self.status = e,
            }
            return;
        }
        if text.is_empty() && self.attachments.is_empty() {
            return;
        }
        self.interrupt_armed = false;
        self.command_index = 0;
        let parsed = match engine::parse(text) {
            Ok(v) => v,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        match parsed["kind"].as_str() {
            Some("command") => {
                self.input.clear();
                if let Err(e) = self.command(
                    parsed["name"].as_str().unwrap_or_default(),
                    parsed["args"].as_str().unwrap_or_default(),
                ) {
                    self.status = e;
                    self.input.set(&raw);
                }
                return;
            }
            Some("error") => {
                self.status = parsed["message"]
                    .as_str()
                    .unwrap_or("Invalid command")
                    .into();
                return;
            }
            _ => {}
        }
        let text = parsed["text"].as_str().unwrap_or(text).to_string();
        if self.peer_connected() {
            self.say_to_peer(&text);
            return;
        }
        if self.agents.running() >= crate::agents::MAX_RUNNING {
            self.status = "Four agent workers are occupied. Wait for one to finish or /agent stop ID before sending another request.".into();
            return;
        }
        let text = if text.is_empty() {
            "Please describe the attached files.".into()
        } else {
            text
        };
        if let Some(model) = self.models.iter().find(|m| m.id == self.args.model)
            && let Some(inputs) = model.architecture["input_modalities"].as_array()
        {
            for file in &self.attachments {
                if file.kind != "text" && !inputs.iter().any(|v| v == &file.kind) {
                    self.status = format!(
                        "Cannot send {}: {} does not accept {} input. Choose a compatible /model or /detach the file.",
                        file.name, model.id, file.kind
                    );
                    return;
                }
            }
        }
        // First run: ask for the key in the masked field, keeping the message.
        if self.args.needs_explicit_model() {
            self.status = format!(
                "The {} interface needs a model id. Use /model ID. No prompt sent.",
                self.args.interface.label()
            );
            return;
        }
        if !key_configured(&self.args) {
            self.key_return = Some(raw);
            self.input.clear();
            self.key_mode = true;
            self.panel = None;
            self.status = match self.args.interface {
                InterfaceKind::OpenRouter => {
                    "Paste your OpenRouter API key to send this (openrouter.ai/keys). Enter saves; Esc cancels.".into()
                }
                InterfaceKind::OpenAI => {
                    "Paste your OpenAI API key to send this. Enter saves; Esc cancels.".into()
                }
                InterfaceKind::Compatible => {
                    "Paste the key for this endpoint to send this. Enter saves; Esc cancels.".into()
                }
            };
            return;
        }
        self.transcript_cache = None;
        let chat = &mut self.chats[self.active];
        let mut message = Message::user(text.clone());
        message.attachments = self.attachments.clone();
        chat.messages.push(message);
        let request = ChatRequest {
            messages: chat.messages.clone(),
        };
        if let Err(e) = request.validate() {
            chat.messages.pop();
            self.status = e.into();
            return;
        }
        let limit = match crate::attachments::chat_limit() {
            Ok(limit) => limit,
            Err(e) => {
                chat.messages.pop();
                self.status = e;
                return;
            }
        };
        if serde_json::to_vec(&request).map_or(true, |b| b.len() > limit) {
            chat.messages.pop();
            self.status = "Conversation is too large. Use /new.".into();
            return;
        }
        // Keep a title chosen with /rename before the first message.
        if chat.messages.len() == 1 && chat.title == "New chat" {
            chat.title = clean(&text).replace('\n', " ").chars().take(60).collect();
        }
        self.sent = text;
        self.attachments.clear();
        self.input.clear();
        self.panel = None;
        self.scroll_anchor = None;
        self.scroll_back = 0;
        self.agents_view = false;
        self.status = "Thinking… Ctrl+C to stop.".into();
        let args = self.args.clone();
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.pending = Some(rx);
        self.pending_started = Some(Instant::now());
        self.pending_tools = args.tools != crate::ToolsSetting::Off;
        self.pending_stopping = false;
        self.pending_actions.clear();
        let roots = self.tool_roots();
        let events = self.tool_channel("Pitty".into(), cancel.clone());
        std::thread::spawn(move || {
            let result = if let Some(mode) = args.tools.mode() {
                roots
                    .and_then(crate::local_tools::LocalTools::new)
                    .and_then(|runtime| {
                        chat::request_with_tools(&args, &request, cancel, runtime, events, mode)
                    })
            } else {
                chat::request(&args, &request, cancel)
            };
            let _ = tx.send(result);
        });
    }
    fn cancel_request(&mut self) {
        if self.pending_tools && self.pending.is_some() {
            if let Some(cancel) = &self.cancel {
                cancel.store(true, Ordering::SeqCst);
            }
            self.pending_stopping = true;
            self.models_rx = None;
            self.folder_abandoned = true;
            self.status = "Stopping local work…".into();
            return;
        }
        self.pending_started = None;
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::SeqCst);
        }
        if self.pending.take().is_some() {
            self.transcript_cache = None;
            if let Some(message) = self.chats[self.active].messages.pop() {
                self.attachments = message.attachments;
            }
            if self.input.is_empty() {
                self.input.set(&if self.sent.starts_with('/') {
                    format!("/{}", self.sent)
                } else {
                    self.sent.clone()
                });
            }
        }
        self.models_rx = None;
        self.folder_abandoned = true;
    }
    fn receive(&mut self) -> bool {
        let mut changed = self.receive_peer();
        changed |= self.receive_tools();
        changed |= self.receive_folder();
        changed |= self.receive_agents();
        if let Some(result) = self.pending.as_ref().and_then(|r| match r.try_recv() {
            Ok(v) => Some(v),
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("Request stopped unexpectedly.".into()))
            }
            _ => None,
        }) {
            changed = true;
            // A worker sends action events before its final result; drain them
            // again to preserve every completed effect on interruption.
            self.receive_tools();
            self.pending = None;
            self.transcript_cache = None;
            let duration = self.pending_started.take().map(|t| t.elapsed());
            self.cancel = None;
            match result {
                Ok(reply) => {
                    if self.pending_tools && !self.pending_actions.is_empty() {
                        self.sent.clear();
                    }
                    if self.scroll_back > 0 && self.panel.is_none() && self.picker.is_none() {
                        self.scroll_anchor = Some(self.scroll_max.saturating_sub(self.scroll_back));
                    }
                    self.chats[self.active].messages.push(reply.message);
                    self.chats[self.active].updated = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos();
                    self.status = if self.pending_stopping {
                        "Stopped · /actions shows completed work.".into()
                    } else if reply.truncated {
                        "Token limit reached; /tokens changes the limit.".into()
                    } else {
                        format!(
                            "Ready{} · Ctrl+Y copies the last reply · F2 selects text.",
                            duration
                                .map(|d| format!(" in {}", elapsed(d)))
                                .unwrap_or_default()
                        )
                    };
                    self.save();
                }
                // Record an interrupted turn only when local actions ran, so
                // their effects stay in context. Any other failure (no key, a
                // network error, Ctrl+C before a tool ran, a model without tool
                // support) restores the draft and files like an ordinary chat.
                Err(e) if self.pending_tools && !self.pending_actions.is_empty() => {
                    self.finish_interrupted_tools(&e);
                    self.status = format!("{} · /actions shows completed work.", clean(&e));
                }
                Err(e) => {
                    if let Some(message) = self.chats[self.active].messages.pop() {
                        self.attachments = message.attachments;
                    }
                    if self.input.is_empty() {
                        self.input.set(&if self.sent.starts_with('/') {
                            format!("/{}", self.sent)
                        } else {
                            self.sent.clone()
                        });
                    }
                    self.status = if self.attachments.is_empty() {
                        clean(&e)
                    } else {
                        format!(
                            "Request failed with files [{}]: {}",
                            self.attachments
                                .iter()
                                .map(|f| f.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", "),
                            clean(&e)
                        )
                    };
                }
            }
            self.pending_tools = false;
            self.pending_stopping = false;
            if self.scroll_anchor.is_none() {
                self.scroll_back = 0;
            }
        }
        if let Some(result) = self.models_rx.as_ref().and_then(|r| match r.try_recv() {
            Ok(v) => Some(v),
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("Model catalog request stopped.".into()))
            }
            _ => None,
        }) {
            changed = true;
            self.models_rx = None;
            self.cancel = None;
            match result {
                Ok(models) => {
                    self.models = models;
                    self.status =
                        "↑↓ choose · Enter select · Tab free/all · Ctrl+R refresh · Esc back"
                            .into();
                }
                Err(e) => self.status = e,
            }
        }
        changed
    }
    fn save_key(&mut self) -> Result<(), String> {
        let key = self.input.value().trim().to_string();
        if key.len() > 4096 {
            return Err("API key exceeds the 4096-byte limit.".into());
        }
        accept_key(self.args.interface, &key)?;
        let chosen = self
            .args
            .key_file
            .clone()
            .map(Ok)
            .unwrap_or_else(crate::config_path)?;
        // Write through a symlink to the file it names, keeping the link.
        let path = if fs::symlink_metadata(&chosen).is_ok_and(|m| m.file_type().is_symlink()) {
            fs::canonicalize(&chosen).map_err(|e| e.to_string())?
        } else {
            chosen
        };
        // Keep every other entry in an existing dotenv file; replace only the key.
        let existing = match fs::metadata(&path) {
            Ok(meta) if meta.is_file() && meta.len() <= 65_536 => {
                fs::read_to_string(&path).map_err(|e| e.to_string())?
            }
            Ok(_) => return Err("The key path is not a small regular file.".into()),
            Err(_) => String::new(),
        };
        let contents = key_file_contents(&existing, self.args.interface.key_var(), &key);
        let parent = path.parent().ok_or("Invalid key path")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let temp = parent.join(format!(
            ".pitty-key-{}-{}.tmp",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut f = options.open(&temp).map_err(|e| e.to_string())?;
        let result = (|| -> io::Result<()> {
            f.write_all(contents.as_bytes())?;
            f.sync_all()?;
            fs::rename(&temp, &path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result.map_err(|e| e.to_string())?;
        self.args.key_file = Some(path);
        self.input.clear();
        self.key_mode = false;
        self.status = if let Some(draft) = self.key_return.take() {
            self.input.set(&draft);
            "API key saved privately. Press Enter to send your message.".into()
        } else {
            "API key saved privately. It was not sent anywhere.".into()
        };
        Ok(())
    }
    fn scroll_view(&mut self, up: bool, rows: usize) {
        if self.panel.is_some() {
            self.panel_scroll = if up {
                self.panel_scroll.saturating_sub(rows)
            } else {
                self.panel_scroll.saturating_add(rows)
            };
        } else {
            self.scroll_anchor = None;
            self.scroll_back = if up {
                self.scroll_back.saturating_add(rows).min(self.scroll_max)
            } else {
                self.scroll_back.saturating_sub(rows)
            };
        }
    }
    fn scroll_at(&mut self, up: bool, x: usize, y: usize) {
        if self.args.no_mouse || self.key_mode || self.confirm_quit {
            return;
        }
        let Some((left, top, width, height)) = self.viewport else {
            return;
        };
        if x < left || x >= left + width || y < top || y >= top + height {
            return;
        }
        self.clear_mouse_selection();
        match self.picker {
            Some(Picker::Models | Picker::Files) => {
                let count = if self.picker == Some(Picker::Files) {
                    self.file_matches().len()
                } else {
                    self.model_matches().len()
                };
                self.picker_index = if up {
                    self.picker_index.saturating_sub(1)
                } else {
                    (self.picker_index + 1).min(count.saturating_sub(1))
                };
            }
            Some(Picker::Copy) => {
                self.picker_preview = if up {
                    self.picker_preview.saturating_sub(3)
                } else {
                    self.picker_preview.saturating_add(3)
                }
            }
            None => self.scroll_view(up, 3),
        }
    }
    fn handle(&mut self, key: Key) {
        if self.handle_approval(&key) {
            return;
        }
        if let Key::Mouse { kind, x, y } = key {
            self.mouse_event(kind, x, y);
            return;
        }
        if let Key::Scroll { up, x, y } = key {
            self.scroll_at(up, x, y);
            return;
        }
        if key == Key::Unknown {
            return;
        }
        if key == Key::Escape && self.clear_mouse_selection() {
            self.interrupt_armed = false;
            return;
        }
        if key != Key::Ctrl('y') {
            self.clear_mouse_selection();
            self.mouse_surface = None;
        }
        if key == Key::F2 && !self.key_mode && !self.confirm_quit {
            self.toggle_selection();
            return;
        }
        if self.selection_mode {
            match key {
                Key::Escape | Key::Enter => {
                    self.toggle_selection();
                    return;
                }
                Key::Ctrl('y') => {
                    let _ = self.copy("last").map_err(|e| self.status = e);
                    self.selection_painted = false;
                    return;
                }
                Key::Ctrl('c' | 'q') | Key::Eof => self.toggle_selection(),
                _ => return,
            }
        }
        if key == Key::Eof {
            self.input.clear();
            self.cancel_request();
            self.quit = true;
            return;
        }
        if self.welcome_fade.is_none() {
            self.welcome_fade = Some(Instant::now());
        }

        if self.key_mode {
            match key {
                Key::Escape | Key::Ctrl('c') => {
                    self.input.clear();
                    self.key_mode = false;
                    self.status = "Key entry canceled.".into();
                    if let Some(draft) = self.key_return.take() {
                        self.input.set(&draft);
                        self.status = "Key entry canceled; your message is still here.".into();
                    }
                }
                Key::Enter => {
                    if let Err(e) = self.save_key() {
                        self.status = e;
                    }
                }
                other => self.input.edit(&other),
            }
            return;
        }

        if self.confirm_quit {
            match key {
                Key::Char('y' | 'Y') => {
                    self.cancel_request();
                    self.quit = true;
                }
                Key::Enter | Key::Escape | Key::Char('n' | 'N') => {
                    self.confirm_quit = false;
                    self.interrupt_armed = false;
                    self.status = "Session kept open.".into();
                }
                _ => {}
            }
            return;
        }
        if key == Key::Ctrl('y') {
            if let Err(e) = self.copy_selected_or_last() {
                self.status = e;
            }
            return;
        }
        if self.picker.is_some() && !matches!(key, Key::Ctrl('c' | 'q')) {
            self.handle_picker(key);
            return;
        }
        if matches!(key, Key::Left | Key::Right | Key::Up | Key::Down) && self.peer_arrows() {
            let (dx, dy) = match key {
                Key::Left => (-1, 0),
                Key::Right => (1, 0),
                Key::Up => (0, -1),
                Key::Down => (0, 1),
                _ => (0, 0),
            };
            if let Err(error) = self.nudge_selected(dx, dy) {
                self.status = error;
            }
            return;
        }
        match key {
            Key::Ctrl('y') => {
                if let Err(e) = self.copy("last") {
                    self.status = e;
                }
            }
            Key::Ctrl('c') => {
                if self.interrupt_armed {
                    self.confirm_quit = true;
                } else {
                    let active = self.pending.is_some()
                        || self.models_rx.is_some()
                        || (self.folder_rx.is_some() && !self.folder_abandoned)
                        || self
                            .agents
                            .jobs
                            .iter()
                            .any(|job| job.state == crate::agents::JobState::Running);
                    self.cancel_request();
                    self.agents.cancel_all();
                    if self.picker.is_some() {
                        self.close_picker();
                    }
                    if !active {
                        self.input.clear();
                        self.attachments.clear();
                    }
                    self.interrupt_armed = true;
                    self.status = "Stopped. Press Ctrl+C again to close the session.".into();
                }
            }
            Key::Ctrl('q') => self.confirm_quit = true,
            Key::Eof => {
                self.cancel_request();
                self.quit = true;
            }
            Key::Escape => {
                if self.peer_art {
                    self.peer_art = false;
                    self.input.clear();
                    self.interrupt_armed = false;
                    self.status = "Art canceled.".into();
                } else {
                    if self.folder_rx.is_some() && !self.folder_abandoned {
                        self.folder_abandoned = true;
                        self.status = "Stopped reading files; nothing was staged.".into();
                    }
                    self.agents_view = false;
                    if self.panel.take().is_none() {
                        if self.peer_open {
                            self.peer_open = false;
                            self.input.clear();
                            self.status = "Room hidden. /peer opens it. Still connected.".into();
                        } else {
                            self.input.clear();
                        }
                    }
                    self.interrupt_armed = false;
                }
            }
            Key::Ctrl('n') => self.new_chat(),
            Key::PageUp | Key::PageDown => {
                let page = self
                    .viewport
                    .map_or(10, |(_, _, _, height)| height.saturating_sub(1).max(1));
                self.scroll_view(key == Key::PageUp, page);
            }
            Key::Tab => {
                let candidates = self.suggestions();
                if !candidates.is_empty() {
                    let name = candidates[self.command_index.min(candidates.len() - 1)].0;
                    self.input.set(&format!("/{name} "));
                    self.status = "Enter runs this command; type arguments to customize it.".into();
                    self.command_index = 0;
                } else if self.input.is_empty() {
                    let _ = self.command("chats", "");
                }
            }
            Key::Up | Key::Down if !self.suggestions().is_empty() => {
                let len = self.suggestions().len();
                if key == Key::Up {
                    self.command_index = self.command_index.saturating_sub(1);
                } else {
                    self.command_index = (self.command_index + 1).min(len - 1);
                }
            }
            Key::Up | Key::Down if self.panel.is_some() && self.input.is_empty() => {
                if key == Key::Up {
                    self.panel_scroll = self.panel_scroll.saturating_sub(1);
                } else {
                    self.panel_scroll = self.panel_scroll.saturating_add(1);
                }
            }
            Key::Up | Key::Down if self.input.is_empty() => {
                self.scroll_view(key == Key::Up, 1);
            }
            Key::Enter => {
                let candidates = self.suggestions();
                let exact = candidates
                    .iter()
                    .any(|c| self.input.value() == format!("/{}", c.0));
                if (!exact || self.command_index > 0) && !candidates.is_empty() {
                    let command = candidates[self.command_index.min(candidates.len() - 1)];
                    self.input.set(&format!(
                        "/{}{}",
                        command.0,
                        if command.1.is_empty() || command.1.starts_with('[') {
                            ""
                        } else {
                            " "
                        }
                    ));
                    if command.1.is_empty() || command.1.starts_with('[') {
                        self.send();
                    }
                } else {
                    self.send();
                }
            }
            Key::Paste(text) if self.pending.is_none() => {
                self.interrupt_armed = false;
                // A path-like paste is checked for dropped files in the
                // background. If that cannot start, keep the text as typed.
                let as_text = !crate::attachments::looks_like_drop(&text)
                    || self.drop_in_background(text.clone()).is_err();
                if as_text && !self.input.insert(&text) {
                    self.status =
                        "Paste would make the draft longer than 1 MB; nothing inserted.".into();
                }
            }
            Key::PasteRejected => self.status = "Paste exceeds 1 MB; nothing inserted.".into(),
            other => {
                self.interrupt_armed = false;
                self.input.edit(&other);
                self.command_index = 0;
            }
        }
    }
    fn draw_picker(
        &mut self,
        frame: &mut Frame,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        colors: &Colors,
    ) {
        if self.picker == Some(Picker::Files) {
            self.draw_files(frame, (x, y, w, h), colors);
        } else if self.picker == Some(Picker::Models) {
            let matches = self.model_matches();
            self.picker_index = self.picker_index.min(matches.len().saturating_sub(1));
            self.picker_rows = h.saturating_sub(2).checked_div(2).unwrap_or(1).max(1);
            frame.text(
                x,
                y,
                &format!(
                    "MODELS · {} · {} matches",
                    if self.free_only {
                        "free text pricing"
                    } else {
                        "chat compatible"
                    },
                    matches.len()
                ),
                colors.accent,
                w,
            );
            if matches.is_empty() {
                frame.text(
                    x,
                    y + 2,
                    if self.models_rx.is_some() {
                        "Loading…"
                    } else {
                        "No matching chat models. Edit search or Ctrl+R to refresh."
                    },
                    colors.muted,
                    w,
                );
            }
            let start = self.picker_index.saturating_sub(self.picker_rows - 1);
            for (row, &index) in matches
                .iter()
                .skip(start)
                .take(self.picker_rows)
                .enumerate()
            {
                let m = &self.models[index];
                let selected = start + row == self.picker_index;
                let style = if selected {
                    colors.selected
                } else {
                    colors.base
                };
                frame.fill(x, y + 2 + row * 2, w, 2, style);
                frame.text(
                    x,
                    y + 2 + row * 2,
                    &format!(
                        "{} {}  {}",
                        if selected { "›" } else { " " },
                        index + 1,
                        m.id
                    ),
                    style,
                    w,
                );
                frame.text(
                    x + 2,
                    y + 3 + row * 2,
                    &format!("{} · {} context", m.price(), m.context_length),
                    style,
                    w.saturating_sub(2),
                );
            }
        } else {
            let items = self.copy_matches();
            self.picker_index = self.picker_index.min(items.len().saturating_sub(1));
            frame.text(
                x,
                y,
                "COPY · choose a message or code block",
                colors.accent,
                w,
            );
            let list_rows = h.saturating_sub(4).min(5);
            let start = self
                .picker_index
                .saturating_sub(list_rows.saturating_sub(1));
            for (row, item) in items.iter().skip(start).take(list_rows).enumerate() {
                let style = if start + row == self.picker_index {
                    colors.selected
                } else {
                    colors.base
                };
                frame.fill(x, y + 2 + row, w, 1, style);
                frame.text(
                    x,
                    y + 2 + row,
                    &format!(
                        "{} {}",
                        if start + row == self.picker_index {
                            "›"
                        } else {
                            " "
                        },
                        item.label
                    ),
                    style,
                    w,
                );
            }
            let preview_y = y + list_rows + 3;
            if let Some(item) = items.get(self.picker_index) {
                let lines = wrap(&item.text, w.saturating_sub(2));
                let visible = h.saturating_sub(list_rows + 3);
                self.picker_preview = self.picker_preview.min(lines.len().saturating_sub(visible));
                frame.fill(x, preview_y, w, visible, colors.input);
                for (row, line) in lines
                    .iter()
                    .skip(self.picker_preview)
                    .take(visible)
                    .enumerate()
                {
                    frame.text(
                        x + 1,
                        preview_y + row,
                        line,
                        colors.input,
                        w.saturating_sub(2),
                    );
                }
            } else {
                frame.text(x, y + 2, "No matching elements.", colors.muted, w);
            }
        }
    }
    fn draw(&mut self, w: usize, h: usize) -> Frame {
        if !self.approvals.is_empty() && self.approval_size != Some((w, h)) {
            // A review must be painted in the current geometry before a yes
            // can authorize it, including after a resize hides the preview.
            self.approval_painted = None;
            self.approval_size = Some((w, h));
        }
        if let Some(frame) = self.draw_mouse_selection(w, h) {
            return frame;
        }
        self.mouse_surface = None;
        self.viewport = None;
        if self.chats[self.active].messages.is_empty() {
            self.scroll_max = 0;
        }
        let colors = self.colors();
        let mut frame = Frame::new(w, h, colors.base);
        if w < 20 || h < 8 {
            frame.text(0, 0, "Resize terminal to continue.", colors.base, w);
            return frame;
        }
        let margin = 2;
        let available = w.saturating_sub(4);
        let editor_width = available.saturating_sub(4).max(1);
        let value = self.input.value();
        let secret = self.key_mode
            || value
                .trim_start()
                .split_once(char::is_whitespace)
                .is_some_and(|(name, _)| name.eq_ignore_ascii_case("/key"));
        let (mut draft, mut cursor) = self.input.visual(editor_width);
        if secret {
            let masked = "•".repeat(if self.key_mode {
                value.chars().count()
            } else {
                value.chars().count().saturating_sub(5)
            });
            draft = vec![format!(
                "{}{}",
                if self.key_mode { "" } else { "/key " },
                masked
            )];
            cursor = (draft[0].chars().count().min(editor_width - 1), 0);
        }
        let input_rows = draft.len().clamp(1, if h < 24 { 3 } else { 6 });
        let footer_rows = if w < 76 { 2 } else { 1 };
        let status_rows = 2;
        let input_y = h.saturating_sub(footer_rows + status_rows + input_rows + 2);
        let candidates = if self.key_mode {
            vec![]
        } else {
            self.suggestions()
        };
        let menu_rows = if candidates.is_empty() {
            0
        } else {
            (candidates.len().min(5) + 1).min(input_y.saturating_sub(2))
        };
        let attachment_rows = usize::from(!self.attachments.is_empty());
        let content_end = input_y.saturating_sub(menu_rows + 1 + attachment_rows);
        // Each letter of PITTY is a rainbow color on a light ground of the same hue.
        for (index, (letter, style)) in wordmark(self.dark()).into_iter().enumerate() {
            frame.text(margin + index, 0, &letter.to_string(), style, 1);
        }
        let model = self
            .args
            .model
            .rsplit('/')
            .next()
            .unwrap_or(&self.args.model)
            .to_string();
        frame.text(
            margin + 7,
            0,
            &format!(
                "/ {model}{}",
                self.profile
                    .as_ref()
                    .ok()
                    .filter(|p| !p.team.is_empty())
                    .map(|p| format!(" · {}", p.team))
                    .unwrap_or_default()
            ),
            colors.muted,
            available.saturating_sub(7),
        );
        let cwd = self
            .working_dir
            .as_ref()
            .map(|p| home_relative(p))
            .unwrap_or_else(|e| format!("unavailable ({e})"));
        let suffix = format!(
            " · {} connected (/pwd)",
            self.folders.as_ref().map_or(0, |s| s.roots.len())
        );
        // One line: abbreviate the middle of a long path rather than wrapping it.
        let room = available.saturating_sub(width("Folder  ") + width(&suffix));
        let location = format!("Folder  {}{suffix}", shorten_middle(&cwd, room.max(12)));
        let header_lines = if h >= 18 { vec![location] } else { vec![] };
        for (row, line) in header_lines.iter().enumerate() {
            frame.text(margin, row + 1, line, colors.muted, available);
        }
        let content_y = 2 + header_lines.len();
        let content_h = content_end.saturating_sub(content_y);
        let mut x = margin;
        let mut content_w = available;
        if self.sidebar && w >= 100 {
            let side_w = 24;
            frame.text(margin, content_y, "CONVERSATIONS", colors.accent, side_w);
            let start = self.active.saturating_sub(content_h.saturating_sub(2));
            for (i, c) in self
                .chats
                .iter()
                .enumerate()
                .skip(start)
                .take(content_h.saturating_sub(1))
            {
                frame.text(
                    margin,
                    content_y + 1 + i - start,
                    &format!(
                        "{} {} {}",
                        if i == self.active { "›" } else { " " },
                        i + 1,
                        c.title
                    ),
                    if i == self.active {
                        colors.selected
                    } else {
                        colors.muted
                    },
                    side_w,
                );
            }
            x += side_w + 2;
            content_w = available.saturating_sub(side_w + 2);
        }
        content_w = content_w.saturating_sub(2);
        self.viewport = Some((x, content_y, content_w + 1, content_h));
        let mut selection_bounds = vec![Some((x, x + content_w)); content_h];
        let mut selection_code_rows = vec![None; content_h];
        if !self.approvals.is_empty() {
            self.draw_approval(&mut frame, x, content_y, content_w, content_h, &colors);
        } else if self.picker.is_some() {
            self.draw_picker(&mut frame, x, content_y, content_w, content_h, &colors);
        } else if let Some((title, entries)) = &self.panel {
            frame.text(x, content_y, title, colors.accent, content_w);
            let lines = entries
                .iter()
                .flat_map(|s| wrap(s, content_w))
                .collect::<Vec<_>>();
            let visible = content_h.saturating_sub(2);
            self.panel_scroll = self.panel_scroll.min(lines.len().saturating_sub(visible));
            for (i, line) in lines
                .iter()
                .skip(self.panel_scroll)
                .take(visible)
                .enumerate()
            {
                frame.text(x, content_y + 2 + i, line, colors.base, content_w);
            }
        } else if self.peer_open {
            self.draw_peer(&mut frame, x, content_y, content_w, content_h, &colors);
        } else if self.chats[self.active].messages.is_empty()
            && self
                .welcome_fade
                .is_none_or(|t| t.elapsed() < Duration::from_millis(600))
        {
            let logo = [
                " ____  ___ _____ _____ __   __",
                "|  _ \\|_ _|_   _|_   _|\\ \\ / /",
                "| |_) || |  | |   | |   \\ V / ",
                "|  __/ | |  | |   | |    | |  ",
                "|_|   |___| |_|   |_|    |_|  ",
            ];
            let rainbow = if self.dark() {
                [
                    (255, 117, 131),
                    (255, 181, 91),
                    (178, 215, 85),
                    (95, 190, 255),
                    (198, 149, 255),
                ]
            } else {
                [
                    (191, 41, 58),
                    (164, 82, 5),
                    (49, 125, 56),
                    (33, 100, 178),
                    (133, 57, 164),
                ]
            };
            if content_w >= 32 && content_h >= 8 {
                for (row, line) in logo.iter().enumerate() {
                    for (col, ch) in line.chars().enumerate() {
                        let mut style = if (11..23).contains(&col) {
                            Style::new(Some(rainbow[row]), colors.base.bg, true)
                        } else {
                            colors.accent
                        };
                        // Auto uses the terminal's own colors, which may be dark;
                        // blending toward a guessed background would flash, so
                        // the logo simply gives way to the hints.
                        if let Some(start) = self.welcome_fade.filter(|_| self.theme != Theme::Auto)
                        {
                            let amount = (start.elapsed().as_secs_f32() / 0.6).min(1.0);
                            let target = if self.theme == Theme::Dark {
                                (20, 25, 34)
                            } else {
                                (232, 237, 242)
                            };
                            let source = style.fg.unwrap_or(if self.theme == Theme::Dark {
                                (225, 232, 241)
                            } else {
                                (29, 37, 48)
                            });
                            let blend = |a: u8, b: u8| {
                                (f32::from(a) * (1.0 - amount) + f32::from(b) * amount) as u8
                            };
                            style.fg = Some((
                                blend(source.0, target.0),
                                blend(source.1, target.1),
                                blend(source.2, target.2),
                            ));
                        }
                        frame.text(x + col, content_y + row, &ch.to_string(), style, 1);
                    }
                }
                frame.text(
                    x,
                    content_y + 6,
                    "A little space to think, build, and explore.",
                    colors.base,
                    content_w,
                );
                for (row, hint) in self.start_hints().iter().enumerate() {
                    if 8 + row < content_h {
                        frame.text(x, content_y + 8 + row, hint, colors.muted, content_w);
                    }
                }
            } else {
                frame.text(
                    x,
                    content_y,
                    "PITTY · Ready when you are.",
                    colors.accent,
                    content_w,
                );
            }
        } else if self.chats[self.active].messages.is_empty() {
            // After the logo fades, an empty conversation keeps its next steps.
            for (row, hint) in self.start_hints().iter().enumerate().take(content_h) {
                frame.text(x, content_y + row, hint, colors.muted, content_w);
            }
        } else {
            let chat = &self.chats[self.active];
            let name = self
                .profile
                .as_ref()
                .map_or("You", |p| p.name.as_str())
                .to_string();
            let key = transcript_view::CacheKey {
                chat: chat.id.clone(),
                updated: chat.updated,
                count: chat.messages.len(),
                width: content_w,
                theme: self.theme,
                compact: self.compact,
                name,
            };
            if self.transcript_cache.as_ref().is_none_or(|c| c.key != key) {
                let (lines, code_rows) = transcript_view::render(
                    chat,
                    content_w,
                    &colors,
                    // Syntax colors follow the actual background.
                    if self.dark() { Theme::Dark } else { self.theme },
                    self.compact,
                    &key.name,
                );
                self.transcript_cache = Some(transcript_view::Cache {
                    key,
                    lines,
                    code_rows,
                });
            }
            let lines = &self.transcript_cache.as_ref().unwrap().lines;
            let thinking = self.pending.as_ref().map(|_| {
                vec![(
                    format!(
                        "● {}{} · {}",
                        if self.sent.is_empty() && self.pending_tools {
                            "Running"
                        } else {
                            "Working"
                        },
                        [".", "..", "..."][self.tick / 4 % 3],
                        elapsed(self.pending_started.map_or(Duration::ZERO, |t| t.elapsed()))
                    ),
                    colors.accent,
                )]
            });
            let line_count = lines.len() + usize::from(thinking.is_some());
            let max = line_count.saturating_sub(content_h);
            self.scroll_max = max;
            if let Some(top) = self.scroll_anchor.take() {
                self.scroll_back = max.saturating_sub(top);
            }
            self.scroll_back = self.scroll_back.min(max);
            for (i, spans) in lines
                .iter()
                .chain(thinking.iter())
                .skip(max - self.scroll_back)
                .take(content_h)
                .enumerate()
            {
                if let Some((text, style)) = spans.first()
                    && (text.starts_with("  ╭")
                        || text.starts_with("  │")
                        || text.starts_with("  ╰"))
                {
                    frame.fill(
                        x + 2,
                        content_y + i,
                        content_w.saturating_sub(2).min(100),
                        1,
                        *style,
                    );
                }
                let code_row = spans.first().is_some_and(|(text, _)| {
                    text.starts_with("  ╭") || text.starts_with("  │") || text.starts_with("  ╰")
                });
                // Copy the actual prose/code cells, excluding decorative code
                // borders and row padding while keeping source indentation.
                if spans
                    .first()
                    .is_some_and(|(text, _)| text.starts_with("  │ "))
                {
                    let end = x + spans
                        .iter()
                        .take(spans.len().saturating_sub(1))
                        .map(|(text, _)| width(text))
                        .sum::<usize>();
                    selection_bounds[i] =
                        Some(((x + 4).min(x + content_w), end.min(x + content_w)));
                } else if spans.first().is_some_and(|(text, _)| text == "  ") {
                    let end = x + spans.iter().map(|(text, _)| width(text)).sum::<usize>();
                    selection_bounds[i] =
                        Some(((x + 2).min(x + content_w), end.min(x + content_w)));
                }
                if spans
                    .first()
                    .is_some_and(|(text, _)| text.starts_with("  ╭") || text.starts_with("  ╰"))
                {
                    selection_bounds[i] = None;
                }
                selection_code_rows[i] = self
                    .transcript_cache
                    .as_ref()
                    .unwrap()
                    .code_rows
                    .get(max - self.scroll_back + i)
                    .cloned()
                    .flatten();
                let mut col = x;
                for (text, style) in spans {
                    frame.text(
                        col,
                        content_y + i,
                        text,
                        *style,
                        (x + content_w).saturating_sub(col),
                    );
                    col = col.saturating_add(width(text));
                }
                if code_row {
                    frame.fill(x, content_y + i, 2, 1, colors.base);
                }
            }
            if max > 0 && content_h > 0 {
                let thumb = (content_h * content_h / line_count).clamp(1, content_h);
                let offset = (max - self.scroll_back) * (content_h - thumb) / max;
                for row in 0..content_h {
                    frame.text(
                        x + content_w,
                        content_y + row,
                        if (offset..offset + thumb).contains(&row) {
                            "┃"
                        } else {
                            "│"
                        },
                        colors.muted,
                        1,
                    );
                }
            }
            if max > 0 && content_end > content_y {
                frame.text(
                    x,
                    content_end,
                    &format!(
                        "{} · PgUp/PgDn · {}",
                        if self.args.no_mouse {
                            "↑↓ scroll"
                        } else {
                            "Wheel/↑↓ scroll"
                        },
                        if self.scroll_back == 0 {
                            "latest"
                        } else {
                            "/bottom returns to latest"
                        }
                    ),
                    colors.muted,
                    content_w,
                );
            }
        }
        if !candidates.is_empty() && menu_rows > 1 {
            self.command_index = self.command_index.min(candidates.len() - 1);
            let visible_commands = menu_rows - 1;
            let start = self.command_index.saturating_sub(visible_commands - 1);
            let y = input_y.saturating_sub(menu_rows);
            for (i, command) in candidates
                .iter()
                .enumerate()
                .skip(start)
                .take(visible_commands)
            {
                let selected = i == self.command_index;
                frame.fill(
                    margin,
                    y + i - start,
                    available,
                    1,
                    if selected {
                        colors.selected
                    } else {
                        colors.base
                    },
                );
                frame.text(
                    margin,
                    y + i - start,
                    &format!(
                        "{} /{:<10} {}",
                        if selected { "›" } else { " " },
                        command.0,
                        command.2
                    ),
                    if selected {
                        colors.selected
                    } else {
                        colors.muted
                    },
                    available,
                );
            }
            frame.text(
                margin,
                input_y - 1,
                "↑↓ choose · Tab complete · Enter select · Esc dismiss",
                colors.muted,
                available,
            );
        }
        if !self.attachments.is_empty() && self.picker.is_none() {
            frame.text(
                margin,
                input_y.saturating_sub(menu_rows + 1),
                &format!(
                    "Files: {} · /attachments",
                    self.attachments
                        .iter()
                        .map(|f| f.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                colors.accent,
                available,
            );
        }
        frame.fill(
            margin + 1,
            input_y + 1,
            available - 2,
            input_rows,
            colors.input,
        );
        let rule = "─".repeat(available - 2);
        frame.text(
            margin,
            input_y,
            &format!("╭{rule}╮"),
            colors.muted,
            available,
        );
        for row in 0..input_rows {
            frame.text(margin, input_y + row + 1, "│", colors.muted, 1);
            frame.text(
                margin + available - 1,
                input_y + row + 1,
                "│",
                colors.muted,
                1,
            );
        }
        frame.text(
            margin,
            input_y + input_rows + 1,
            &format!("╰{rule}╯"),
            colors.muted,
            available,
        );
        let visible_start = cursor.1.saturating_sub(input_rows - 1);
        for (row, line) in draft
            .iter()
            .skip(visible_start)
            .take(input_rows)
            .enumerate()
        {
            frame.text(
                margin + 3,
                input_y + 1 + row,
                line,
                colors.input,
                editor_width,
            );
        }
        frame.text(
            margin + 1,
            input_y + 1,
            "›",
            Style::new(colors.input.fg, colors.input.bg, true),
            1,
        );
        if self.input.is_empty() {
            frame.text(
                margin + 3,
                input_y + 1,
                if self.key_mode {
                    "Paste your API key (hidden)"
                } else if self.peer_art {
                    "ASCII art · Alt+Enter newline · Enter places · Esc cancels"
                } else if self.peer_open && self.peer_connected() {
                    "Message the room…"
                } else if self.peer_open {
                    "/peer host, or /peer join ROOM"
                } else if self.picker.is_some() {
                    "Type to filter…"
                } else {
                    "Message, or / for commands…"
                },
                Style::new(colors.muted.fg.or(colors.input.fg), colors.input.bg, false),
                editor_width,
            );
        }
        if !self.confirm_quit && self.approvals.is_empty() {
            frame.cursor = Some((
                (margin + 3 + cursor.0).min(margin + available - 2),
                (input_y + 1 + cursor.1 - visible_start).min(h - 1),
            ));
        }
        let thinking_status = self
            .pending_started
            .map(|t| format!("Thinking · {} · Ctrl+C to stop.", elapsed(t.elapsed())));
        let status = if self.confirm_quit {
            "Close this session? [y/N]  Enter keeps it open."
        } else if !self.approvals.is_empty() {
            "Review action · Y run once · N / Enter decline · Ctrl+C stops"
        } else if self.pending_stopping {
            "Stopping local work… waiting for the process to exit."
        } else if self.selection_mode {
            "Drag to select; use your terminal's Copy shortcut. F2 or Esc returns · Ctrl+Y copies last reply."
        } else if self.status == "Thinking… Ctrl+C to stop." {
            thinking_status.as_deref().unwrap_or(&self.status)
        } else {
            &self.status
        };
        for (i, line) in wrap_words(status, available)
            .iter()
            .take(status_rows)
            .enumerate()
        {
            frame.text(
                margin,
                input_y + input_rows + 2 + i,
                line,
                if self.confirm_quit {
                    colors.error
                } else {
                    colors.muted
                },
                available,
            );
        }
        let picker_hint = if self.selection_mode {
            Some("F2 finish · Ctrl+Y copy reply")
        } else if self.peer_art {
            Some("Alt+Enter newline · Enter places · Esc cancels")
        } else if self.peer_open && self.peer_connected() {
            Some("Enter sends to the room · arrows move art · /peer leave")
        } else if self.peer_open {
            Some("/peer host · /peer join ROOM · Esc hides")
        } else {
            match self.picker {
                Some(Picker::Files) => Some("Enter open/attach · Esc back"),
                Some(Picker::Models) => Some("Enter select · Esc back"),
                Some(Picker::Copy) => Some("Enter copy · Esc back"),
                None => None,
            }
        };
        let info = format!(
            "{} tokens · web {} · tools {} · history {}",
            self.args.max_tokens,
            if self.args.web { "ON" } else { "off" },
            self.args.tools.label(),
            if self.history_dir.is_some() {
                "on"
            } else {
                "off"
            }
        );
        if footer_rows == 2 {
            frame.text(margin, h - 2, &info, colors.muted, available);
            frame.text(
                margin,
                h - 1,
                picker_hint.unwrap_or("Enter send · / commands · Ctrl+C stop / close"),
                colors.accent,
                available,
            );
        } else {
            let left = format!(
                "{} tok · web {} · tools {}",
                self.args.max_tokens,
                if self.args.web { "ON" } else { "off" },
                self.args.tools.label()
            );
            frame.text(margin, h - 1, &left, colors.muted, available);
            let hint = picker_hint.unwrap_or("/ commands · Enter send · Ctrl+C stop");
            let start = w.saturating_sub(width(hint) + margin);
            if start > margin + width(&left) + 2 {
                frame.text(start, h - 1, hint, colors.accent, width(hint));
            }
        }
        self.remember_mouse_surface(
            &frame,
            (x, content_y, content_w, content_h),
            (margin, input_y + input_rows + 2, available, status_rows),
            selection_bounds,
            selection_code_rows,
        );
        frame
    }
}
pub fn run(args: Args) -> Result<(), String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(
            "Interactive chat needs a terminal. Run pitty, or pitty \"your prompt\".".into(),
        );
    }
    let mut app = App::new(args);
    // A closed terminal tab (SIGHUP), `kill` (SIGTERM), or an external SIGINT
    // ends the session through the normal path: cancel requests, reap command
    // groups, restore the terminal. Declared before the terminal, so handling
    // stays installed until the terminal has been restored.
    let _signals = crate::signals::install(&[libc::SIGHUP, libc::SIGTERM, libc::SIGINT])?;
    let mut terminal = Terminal::open().map_err(|e| e.to_string())?;
    app.dark_terminal = terminal
        .background()
        .map(|(r, g, b)| {
            0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b) < 128.0
        })
        .unwrap_or_else(colorfgbg_dark);
    with_worker_cleanup(&mut app, |app| {
        let mut dirty = true;
        let mut size = (0, 0);
        let mut animated = Instant::now();
        let mut was_fading = false;
        while !app.quit {
            if crate::signals::received().is_some() {
                app.quit = true;
                break;
            }
            dirty |= app.receive();
            let next_size = terminal.size();
            dirty |= next_size != size;
            size = next_size;
            if app.welcome_fade.is_none() && app.welcome_started.elapsed() > Duration::from_secs(3)
            {
                app.welcome_fade = Some(Instant::now());
                dirty = true;
            }
            let fading = app
                .welcome_fade
                .is_some_and(|t| t.elapsed() < Duration::from_millis(700));
            if fading != was_fading {
                dirty = true;
                was_fading = fading;
            }
            if (app.pending.is_some() || app.agents.running() > 0 || fading)
                && animated.elapsed() > Duration::from_millis(250)
            {
                app.tick = app.tick.wrapping_add(1);
                dirty = true;
                animated = Instant::now();
            }
            if dirty && (!app.selection_mode || !app.selection_painted) {
                terminal
                    .set_mouse(!app.args.no_mouse)
                    .map_err(|e| e.to_string())?;
                terminal
                    .draw(app.draw(size.0, size.1))
                    .map_err(|e| e.to_string())?;
                dirty = false;
                app.selection_painted = app.selection_mode;
            }
            if let Some(key) = terminal
                .poll(Duration::from_millis(50))
                .map_err(|e| e.to_string())?
            {
                let mut next = Some(key);
                for _ in 0..256 {
                    let Some(key) = next.take() else {
                        break;
                    };
                    // Ctrl+L repaints every cell, as in shells and editors.
                    if key == Key::Ctrl('l') {
                        terminal.invalidate();
                    } else {
                        app.handle(key);
                    }
                    if app.quit {
                        break;
                    }
                    next = terminal.poll(Duration::ZERO).map_err(|e| e.to_string())?;
                }
                dirty = true;
            }
        }
        Ok(())
    })
}

fn peer_host_status(link: &crate::peer::Link) -> String {
    if link.room_number() > 0 {
        format!(
            "Hosting room {}. Nothing is stored on a server.",
            link.room_number()
        )
    } else {
        "Hosting. Nothing is stored on a server.".into()
    }
}
fn with_worker_cleanup<T>(
    app: &mut App,
    run: impl FnOnce(&mut App) -> Result<T, String>,
) -> Result<T, String> {
    // Terminal draw/poll errors must take the same shutdown path as /quit.
    let result = run(app);
    // Keep workers alive long enough to terminate and reap command groups.
    app.peer_shutdown();
    app.cancel_request();
    app.agents.cancel_all();
    let cleanup = Instant::now();
    while (app.pending.is_some() || app.agents.running() > 0 || !app.tool_streams.is_empty())
        && cleanup.elapsed() < Duration::from_secs(5)
    {
        app.receive();
        std::thread::sleep(Duration::from_millis(20));
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    fn app() -> App {
        App::new(Args::try_parse_from(["pitty", "--no-history"]).unwrap())
    }
    #[test]
    fn terminal_error_still_cancels_and_drains_the_local_worker() {
        let mut app = app();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (sender, receiver) = mpsc::channel();
        app.pending = Some(receiver);
        app.pending_tools = true;
        app.cancel = Some(cancel.clone());
        app.chats[0]
            .messages
            .push(Message::user("Run fixture".into()));
        let events = app.tool_channel("Command".into(), cancel.clone());
        let worker = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !worker_cancel.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(worker_cancel.load(Ordering::SeqCst));
            events
                .send(chat::ToolEvent::Completed {
                    summary: "Fixture command stopped".into(),
                    output: "Cleanup completed".into(),
                })
                .unwrap();
            sender.send(Err("Request canceled.".into())).unwrap();
        });
        let result: Result<(), String> =
            with_worker_cleanup(&mut app, |_| Err("Synthetic terminal I/O error".into()));
        assert_eq!(result.unwrap_err(), "Synthetic terminal I/O error");
        assert!(cancel.load(Ordering::SeqCst));
        assert!(app.pending.is_none());
        assert!(app.tool_streams.is_empty());
        assert_eq!(app.chats[0].actions.len(), 1);
        assert!(
            app.chats[0]
                .messages
                .last()
                .unwrap()
                .content
                .contains("Cleanup completed")
        );
        worker.join().unwrap();
    }

    #[test]
    fn responsive_layout_and_mark() {
        for (w, h) in [(80, 24), (120, 40), (40, 18)] {
            let mut a = app();
            let screen = a.draw(w, h).plain_text();
            assert!(screen.contains("PITTY"));
            assert!(screen.contains("Ctrl+C"));
            assert_eq!(screen.lines().count(), h);
        }
    }
    #[test]
    fn wordmark_uses_a_light_rainbow_ground_and_rainbow_tees() {
        let mark = wordmark(false);
        assert_eq!(mark.map(|(letter, _)| letter), ['P', 'I', 'T', 'T', 'Y']);
        let grounds = mark.map(|(_, style)| style.bg.unwrap());
        let inks = mark.map(|(_, style)| style.fg.unwrap());
        assert!(grounds.windows(2).all(|pair| pair[0] != pair[1]));
        assert!(inks.windows(2).all(|pair| pair[0] != pair[1]));
        assert!(
            grounds
                .iter()
                .all(|(red, green, blue)| *red > 180 && *green > 180 && *blue > 180)
        );
        assert!(inks.iter().any(|(red, _, blue)| *red > 160 && *blue < 80));
        assert!(inks.iter().any(|(red, _, blue)| *blue > 160 && *red < 80));
    }
    #[test]
    fn commands_do_not_send_prompts() {
        let mut a = app();
        a.input.set("/tokens 1234");
        a.send();
        assert_eq!(a.args.max_tokens, 1234);
        assert!(a.pending.is_none());
        assert!(a.chats[0].messages.is_empty());
        a.input.set("/bogus");
        a.send();
        assert!(a.status.contains("Unknown"));
    }
    #[test]
    fn control_c_requires_confirmation() {
        let mut a = app();
        a.handle(Key::Ctrl('c'));
        assert!(!a.quit);
        assert!(!a.confirm_quit);
        a.handle(Key::Ctrl('c'));
        assert!(a.confirm_quit);
        a.handle(Key::Enter);
        assert!(!a.quit);
        a.handle(Key::Ctrl('q'));
        a.handle(Key::Char('y'));
        assert!(a.quit);
    }
    #[test]
    fn cancellation_restores_draft() {
        let mut a = app();
        a.sent = "draft".into();
        a.chats[0].messages.push(Message::user("draft".into()));
        let (_tx, rx) = mpsc::channel();
        a.pending = Some(rx);
        let flag = Arc::new(AtomicBool::new(false));
        a.cancel = Some(flag.clone());
        a.handle(Key::Ctrl('c'));
        assert!(flag.load(Ordering::SeqCst));
        assert_eq!(a.input.value(), "draft");
        assert!(a.chats[0].messages.is_empty());
    }
    #[test]
    fn model_switch_starts_fresh_chat() {
        let mut a = app();
        a.chats[0].messages.push(Message::user("existing".into()));
        a.command("model", "openrouter/free").unwrap();
        assert_eq!(a.args.model, "openrouter/free");
        assert_eq!(a.chats.len(), 2);
        assert!(a.chats[1].messages.is_empty());
    }
    #[test]
    fn peer_room_draws_a_cat_and_a_message() {
        let mut app = app();
        app.peer_open = true;
        app.peer_room = Some(crate::peer::Room {
            name: "Ada".into(),
            role: crate::peer::RoleKind::Host,
            share: "192.168.0.103:59828".into(),
            local: "127.0.0.1:59828".into(),
            number: 4,
            peers: vec!["Ada".into(), "Bea".into()],
            arts: vec![crate::peer::Art {
                id: 1,
                name: "cat".into(),
                by: "Bea".into(),
                x: 0,
                y: 0,
                rows: crate::peer::builtin("cat").unwrap(),
            }],
            says: vec![crate::peer::Said {
                by: "Bea".into(),
                text: "hello from bea".into(),
            }],
            nearby: vec![],
            selected: Some(1),
            closed: false,
            discovery: false,
        });
        let text = app.draw(100, 36).plain_text();
        assert!(text.contains("nothing is stored"));
        assert!(text.contains("o.o"));
        assert!(text.contains("Bea · hello from bea"));
        assert!(text.contains("Room 4"));
        assert!(!text.contains("192.168"));
        assert!(!text.contains("127.0.0.1"));
        let hello = text.find("hello from bea").unwrap();
        let cat = text.find("o.o").unwrap();
        assert!(hello < cat, "the message stays above the drawing");
        app.peer_room.as_mut().unwrap().arts[0].y = 12;
        app.peer_room.as_mut().unwrap().arts[0].rows = vec!["Hello".into()];
        app.peer_room.as_mut().unwrap().says[0].text = "There".into();
        let short = app.draw(80, 24).plain_text();
        assert!(
            short.contains("Bea · There"),
            "a message stays visible when the drawing sits low on the stage: {short}"
        );
    }
    #[test]
    fn peer_art_and_unknown_verbs_need_a_room() {
        let mut app = app();
        let missing = app.command("peer", "move left").unwrap_err();
        assert!(missing.contains("room") || missing.contains("Host"));
        assert!(app.command("peer", "art cat").is_err());
        assert!(app.command("peer", "pick 1").is_err());
        let unknown = app.command("peer", "teleport").unwrap_err();
        assert!(unknown.contains("/peer"));
        assert!(app.peer.is_none());
    }
    #[test]
    fn slash_menu_and_completion() {
        let mut a = app();
        a.input.set("/");
        assert!(a.suggestions().len() > 20);
        a.input.set("/tok");
        a.handle(Key::Tab);
        assert_eq!(a.input.value(), "/tokens ");
    }
    #[test]
    fn key_entry_is_masked() {
        let mut a = app();
        a.key_mode = true;
        a.input.set("sk-or-secret-example");
        let text = a.draw(80, 24).plain_text();
        assert!(!text.contains("sk-or-secret"));
        assert!(text.contains('•'));
    }
    fn catalog() -> Vec<chat::Model> {
        serde_json::from_value(serde_json::json!([
            {"id":"example/free","name":"Free model","pricing":{"prompt":"0","completion":"0"},"architecture":{"input_modalities":["text"],"output_modalities":["text"]}},
            {"id":"example/paid","pricing":{"prompt":"0.01","completion":"0.02"}},
            {"id":"example/rerank","endpoints":["/api/v1/rerank"],"architecture":{"output_modalities":["rerank"]}}
        ])).unwrap()
    }
    #[test]
    fn exact_model_command_opens_picker_and_arrows_select_without_network() {
        let mut a = app();
        a.models = catalog();
        a.input.set("/model");
        a.handle(Key::Enter);
        assert!(a.picker == Some(Picker::Models));
        assert_eq!(a.model_matches(), vec![0, 1]);
        a.handle(Key::Tab);
        assert_eq!(a.model_matches(), vec![0]);
        a.handle(Key::Tab);
        a.handle(Key::Down);
        a.handle(Key::Enter);
        assert_eq!(a.args.model, "example/paid");
        assert!(a.pending.is_none());
        assert!(a.models_rx.is_none());
    }
    #[test]
    fn same_model_preserves_conversation_and_new_model_preserves_staged_draft() {
        let mut a = app();
        a.chats[0].messages.push(Message::user("existing".into()));
        a.command("model", MODEL).unwrap();
        assert_eq!(a.chats.len(), 1);
        a.input.set("draft");
        a.attachments.push(pitty_core::Attachment {
            name: "a.txt".into(),
            kind: "text".into(),
            mime: "text/plain".into(),
            data: "hello".into(),
        });
        a.command("model", "example/other").unwrap();
        assert_eq!(a.input.value(), "draft");
        assert_eq!(a.attachments.len(), 1);
    }
    #[test]
    fn ctrl_c_clears_a_megabyte_draft_before_requesting_exit() {
        let mut a = app();
        a.input.set(&"a".repeat(1_048_576));
        a.handle(Key::Ctrl('c'));
        assert!(a.input.is_empty());
        assert!(!a.confirm_quit);
        assert!(!a.quit);
        a.handle(Key::Ctrl('c'));
        assert!(a.confirm_quit);
        a.handle(Key::Char('n'));
        assert!(!a.quit);
    }
    #[test]
    fn eof_closes_modal_states() {
        for mode in 0..3 {
            let mut a = app();
            a.key_mode = mode == 0;
            a.confirm_quit = mode == 1;
            if mode == 2 {
                a.open_picker(Picker::Copy, "");
            }
            a.handle(Key::Eof);
            assert!(a.quit);
        }
    }
    #[test]
    fn arrow_scrolling_keeps_composer_anchored_and_code_is_copyable() {
        let mut a = app();
        a.chats[0].messages.push(Message::user(format!(
            "{}\n```rust\nfn main() {{}}\n```",
            "A paragraph that wraps at word boundaries.\n".repeat(80)
        )));
        let before = a.draw(90, 30);
        a.handle(Key::Up);
        let after = a.draw(90, 30);
        assert_eq!(before.cursor, after.cursor);
        assert_eq!(a.scroll_back, 1);
        a.command("copy", "").unwrap();
        a.input.set("code");
        let items = a.copy_matches();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "fn main() {}\n");
        a.handle(Key::Escape);
        assert!(a.picker.is_none());
    }
    #[test]
    fn web_toggle_is_local_and_visible() {
        let mut a = app();
        a.input.set("/web on");
        a.handle(Key::Enter);
        assert!(a.args.web);
        assert!(a.pending.is_none());
        assert!(a.draw(100, 30).plain_text().contains("web ON"));
        a.command("web", "off").unwrap();
        assert!(!a.args.web);
    }
    #[test]
    fn incompatible_attachment_error_names_file_and_preserves_draft() {
        let mut a = app();
        a.models = catalog();
        a.args.model = "example/free".into();
        a.attachments.push(pitty_core::Attachment {
            name: "photo.png".into(),
            kind: "image".into(),
            mime: "image/png".into(),
            data: "YWJj".into(),
        });
        a.input.set("Describe the photo");
        a.send();
        assert!(a.pending.is_none());
        assert!(a.status.contains("photo.png"));
        assert_eq!(a.attachments.len(), 1);
        assert_eq!(a.input.value(), "Describe the photo");
    }
    #[test]
    fn narrow_layouts_and_picker_states_do_not_overlap_composer() {
        for (w, h) in [(20, 8), (40, 18), (80, 24), (140, 45)] {
            let mut a = app();
            a.models = catalog();
            a.open_picker(Picker::Models, "");
            let f = a.draw(w, h);
            assert_eq!(f.plain_text().lines().count(), h);
            if let Some((x, y)) = f.cursor {
                assert!(x < w && y < h);
            }
        }
    }
    #[test]
    fn inline_key_arguments_are_always_masked() {
        for text in [
            "/key secret-value",
            "/KEY secret-value",
            " /KeY\tsecret-value",
        ] {
            let mut a = app();
            a.input.set(text);
            assert!(!a.draw(80, 24).plain_text().contains("secret-value"));
        }
    }
    fn scrolling_app() -> App {
        let mut a = app();
        let text = (0..120)
            .map(|i| format!("Transcript line {i:03}.\n"))
            .collect::<String>();
        a.chats[0].messages.push(Message::user(text));
        a
    }
    fn bottom(frame: &Frame) -> Vec<String> {
        let text = frame.plain_text();
        let start = text
            .lines()
            .position(|line| line.trim_start().starts_with("╭─"))
            .unwrap();
        text.lines().skip(start).map(str::to_owned).collect()
    }
    #[test]
    fn wheel_scroll_keeps_composer_status_footer_and_draft_fixed() {
        for (w, h) in [(80, 24), (120, 40), (40, 18)] {
            for draft in [
                "",
                "Draft stays here",
                "First draft line\nSecond draft line",
            ] {
                let mut a = scrolling_app();
                a.input.set(draft);
                let before = a.draw(w, h);
                let (x, y, _, _) = a.viewport.unwrap();
                a.interrupt_armed = true;
                a.handle(Key::Scroll { up: true, x, y });
                let after = a.draw(w, h);
                assert_eq!(a.scroll_back, 3);
                assert_ne!(before.plain_text(), after.plain_text());
                assert_eq!(bottom(&before), bottom(&after));
                assert_eq!(before.cursor, after.cursor);
                assert_eq!(a.input.value(), draft);
                assert!(a.interrupt_armed);
                a.handle(Key::Scroll { up: false, x, y });
                a.draw(w, h);
                assert_eq!(a.scroll_back, 0);
            }
        }
    }
    #[test]
    fn wheel_ignores_footer_sidebar_modals_and_disabled_mouse() {
        let mut a = scrolling_app();
        a.sidebar = true;
        a.draw(120, 40);
        let (x, y, _, height) = a.viewport.unwrap();
        for (col, row) in [(x, y + height), (x, 39), (0, y), (x, 0)] {
            a.handle(Key::Scroll {
                up: true,
                x: col,
                y: row,
            });
            assert_eq!(a.scroll_back, 0);
        }
        for mode in 0..3 {
            a.key_mode = mode == 0;
            a.confirm_quit = mode == 1;
            a.args.no_mouse = mode == 2;
            a.handle(Key::Scroll { up: true, x, y });
            assert_eq!(a.scroll_back, 0);
        }
    }
    #[test]
    fn page_scrolling_uses_viewport_height_and_preserves_draft() {
        let mut a = scrolling_app();
        a.input.set("draft");
        let before = a.draw(90, 30);
        let page = a.viewport.unwrap().3 - 1;
        a.handle(Key::PageUp);
        let after = a.draw(90, 30);
        assert_eq!(a.scroll_back, page);
        assert_eq!(bottom(&before), bottom(&after));
        for _ in 0..200 {
            a.handle(Key::PageUp);
        }
        assert_eq!(a.scroll_back, a.scroll_max);
        a.command("bottom", "").unwrap();
        a.draw(90, 30);
        assert_eq!(a.scroll_back, 0);
    }
    #[test]
    fn incoming_reply_preserves_scrolled_reading_position() {
        let mut a = scrolling_app();
        a.draw(90, 30);
        a.handle(Key::PageUp);
        let before = a.draw(90, 30).plain_text();
        let old_top = a.scroll_max - a.scroll_back;
        let (tx, rx) = mpsc::channel();
        a.pending = Some(rx);
        let mut message = Message::user("A new answer.\n".repeat(30));
        message.role = "assistant".into();
        tx.send(Ok(ChatReply {
            message,
            truncated: false,
        }))
        .unwrap();
        assert!(a.receive());
        let after = a.draw(90, 30).plain_text();
        assert_eq!(a.scroll_max - a.scroll_back, old_top);
        assert_eq!(
            before.lines().nth(a.viewport.unwrap().1),
            after.lines().nth(a.viewport.unwrap().1)
        );
    }
    #[test]
    fn mouse_command_roundtrips_through_wasm() {
        let mut a = app();
        a.input.set("/mouse off");
        a.send();
        assert!(a.args.no_mouse);
        a.input.set("/mouse on");
        a.send();
        assert!(!a.args.no_mouse);
        assert!(a.pending.is_none());
        assert!(
            Args::try_parse_from(["pitty", "--no-mouse"])
                .unwrap()
                .no_mouse
        );
    }
    #[test]
    fn header_is_the_only_model_label_and_scroll_commands_discard_stale_anchor() {
        for (w, h) in [(40, 18), (120, 40)] {
            let mut a = scrolling_app();
            assert_eq!(a.draw(w, h).plain_text().matches("nemotron").count(), 1);
            a.scroll_anchor = Some(5);
            a.command("bottom", "").unwrap();
            a.draw(w, h);
            assert_eq!(a.scroll_back, 0);
            a.scroll_anchor = Some(5);
            a.new_chat();
            assert!(a.scroll_anchor.is_none());
        }
    }
    #[test]
    #[ignore = "local rendering benchmark; run with --ignored --nocapture"]
    fn transcript_redraw_benchmark() {
        let mut a = app();
        for i in 0..80 {
            a.chats[0].messages.push(Message::user(format!(
                "Message {i}\n{}\n```rust\nfn main() {{ println!(\"hello\"); }}\n```",
                "A paragraph with **bold words** and `inline code` to format.\n".repeat(50)
            )));
        }
        a.draw(100, 35);
        let started = Instant::now();
        for i in 0..40 {
            a.input.set(&format!("draft {i}"));
            std::hint::black_box(a.draw(100, 35));
        }
        println!(
            "40 redraws of a 232KB formatted transcript: {:?}",
            started.elapsed()
        );
    }
    #[test]
    fn cancel_invalidates_cached_prompt_without_replacing_a_new_draft() {
        let mut a = app();
        a.chats[0].messages.push(Message::user("Old prompt".into()));
        a.draw(90, 30);
        let (_tx, rx) = mpsc::channel();
        a.pending = Some(rx);
        a.sent = "Old prompt".into();
        a.input.set("New draft");
        a.cancel_request();
        assert!(a.transcript_cache.is_none());
        assert_eq!(a.input.value(), "New draft");
        a.chats[0]
            .messages
            .push(Message::user("Replacement prompt".into()));
        let screen = a.draw(90, 30).plain_text();
        assert!(screen.contains("Replacement prompt"));
        assert!(!screen.contains("Old prompt"));
    }
    #[test]
    fn escape_abandons_pending_staging_and_elapsed_time_is_visible() {
        let mut a = app();
        let (_tx, rx) = mpsc::channel();
        a.folder_rx = Some(rx);
        a.input.set("some path");
        a.handle(Key::Escape);
        assert!(a.folder_abandoned);
        assert!(a.input.is_empty());
        let (_tx, rx) = mpsc::channel();
        a.pending = Some(rx);
        a.pending_started = Some(Instant::now() - Duration::from_secs(5));
        a.chats[0].messages.push(Message::user("Question".into()));
        assert!(a.draw(100, 35).plain_text().contains("5s"));
    }
    #[cfg(unix)]
    #[test]
    fn history_rejects_pipes_directories_and_symlinks() {
        let path = env::temp_dir().join(format!(
            "pitty-history-check-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        assert!(history_file(&path).is_err());
        let fifo = path.join("blocked.json");
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        assert!(history_file(&fifo).is_err());
        fs::write(path.join("good.json"), "{}").unwrap();
        std::os::unix::fs::symlink(path.join("good.json"), path.join("link.json")).unwrap();
        assert!(history_file(&path.join("link.json")).is_err());
        assert!(history_file(&path.join("good.json")).is_ok());
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn failed_tool_request_without_actions_restores_draft_and_files() {
        let mut a = app();
        let mut message = Message::user("hello".into());
        message.attachments.push(pitty_core::Attachment {
            name: "notes.txt".into(),
            kind: "text".into(),
            mime: "text/plain".into(),
            data: "x".into(),
        });
        a.chats[0].messages.push(message);
        a.sent = "hello".into();
        let (tx, rx) = mpsc::channel();
        a.pending = Some(rx);
        a.pending_tools = true;
        tx.send(Err("Model does not support tools".into())).unwrap();
        a.receive();
        assert!(a.chats[0].messages.is_empty(), "no fake assistant turn");
        assert_eq!(a.input.value(), "hello");
        assert_eq!(a.attachments.len(), 1);
        assert!(a.status.contains("does not support tools"));
    }
    #[test]
    fn first_message_without_a_key_opens_masked_entry_and_keeps_the_message() {
        let mut a = app();
        a.args.key_file = Some(std::env::temp_dir().join("pitty-test-missing-key.env"));
        a.input.set("Hello there");
        a.send();
        assert!(a.key_mode);
        assert!(a.pending.is_none());
        assert!(a.chats[0].messages.is_empty());
        assert!(a.status.contains("openrouter.ai/keys"));
        a.handle(Key::Escape);
        assert!(!a.key_mode);
        assert_eq!(a.input.value(), "Hello there");
    }
    #[test]
    fn key_updates_keep_other_dotenv_entries() {
        let updated = key_file_contents(
            "# mine\nDATABASE_URL=x\nexport OPENROUTER_API_KEY=old\nOPENROUTER_API_KEY = older\n",
            "OPENROUTER_API_KEY",
            "sk-or-new",
        );
        assert_eq!(
            updated,
            "# mine\nDATABASE_URL=x\nOPENROUTER_API_KEY=sk-or-new\n"
        );
        assert_eq!(
            key_file_contents("", "OPENROUTER_API_KEY", "k"),
            "OPENROUTER_API_KEY=k\n"
        );
        let kept = key_file_contents(
            "OPENROUTER_API_KEY=sk-or-kept\n",
            "OPENAI_API_KEY",
            "sk-openai",
        );
        assert_eq!(
            kept,
            "OPENROUTER_API_KEY=sk-or-kept\nOPENAI_API_KEY=sk-openai\n"
        );
    }
    #[test]
    fn long_folder_paths_shorten_in_the_middle_to_one_line() {
        let path = "/Volumes/Shared Drive/Projects/2026/client-work/very-long-name/src";
        let short = shorten_middle(path, 30);
        assert!(width(&short) <= 30, "{short}");
        assert!(short.starts_with("/Volumes"));
        assert!(short.ends_with("name/src"));
        assert!(short.contains('…'));
        assert_eq!(shorten_middle("~/code", 30), "~/code");
    }
    #[test]
    fn actions_command_is_available_while_a_request_runs() {
        let mut a = app();
        let (_tx, rx) = mpsc::channel::<Result<ChatReply, String>>();
        a.pending = Some(rx);
        a.input.set("/actions");
        a.send();
        assert!(
            a.panel
                .as_ref()
                .is_some_and(|(title, _)| title.starts_with("Local actions"))
        );
    }
    #[test]
    fn new_conversation_keeps_an_unsent_draft_and_files() {
        let mut a = app();
        a.chats[0].messages.push(Message::user("earlier".into()));
        a.input.set("unsent idea");
        a.attachments.push(pitty_core::Attachment {
            name: "a.txt".into(),
            kind: "text".into(),
            mime: "text/plain".into(),
            data: "x".into(),
        });
        a.handle(Key::Ctrl('n'));
        assert!(a.chats[a.active].messages.is_empty());
        assert_eq!(a.input.value(), "unsent idea");
        assert_eq!(a.attachments.len(), 1);
    }
    #[test]
    fn direct_command_output_is_labeled_as_a_command() {
        let messages = vec![
            Message::user("/run ls".into()),
            tools_view::assistant("total 0".into()),
            Message::user("What changed?".into()),
            tools_view::assistant("Nothing.".into()),
        ];
        assert!(tools_view::is_command_result(&messages, 1));
        assert!(!tools_view::is_command_result(&messages, 3));
    }
    #[test]
    fn interface_switch_keeps_the_review_and_asks_for_a_model() {
        let mut a = app();
        a.chats[0].messages.push(Message::user("earlier".into()));
        a.input.set("/interface openai");
        a.send();
        assert_eq!(a.args.interface, crate::interface::InterfaceKind::OpenAI);
        assert!(a.chats[a.active].messages.is_empty());
        assert!(a.status.contains("New conversation"));
        a.input.set("hello");
        a.send();
        assert!(a.pending.is_none());
        assert!(a.chats[a.active].messages.is_empty());
        assert!(a.status.contains("/model"));
        a.input.set("/interface compatible");
        a.send();
        assert!(a.status.contains("BASE_URL"));
        a.input
            .set("/interface compatible http://127.0.0.1:11434/v1");
        a.send();
        assert_eq!(
            a.args.interface,
            crate::interface::InterfaceKind::Compatible
        );
        assert_eq!(
            a.args.chat_endpoint().unwrap(),
            "http://127.0.0.1:11434/v1/chat/completions"
        );
        a.input.set("/web on");
        a.send();
        assert!(!a.args.web);
        assert!(a.status.contains("OpenRouter"));
    }
}
