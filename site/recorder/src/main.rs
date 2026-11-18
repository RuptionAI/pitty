//! Records real Pitty sessions in a pseudo-terminal and renders them into the
//! captioned 1080p demo video on pitty.tech/demo/. See README.md.
//!
//!   record SCENE...  run each scene against the real model, save takes/SCENE.json
//!   render SCENE...  draw every frame with headless Chrome, encode render/pitty-demo.mp4
//!
//! Environment: DEMO_ROOT (working folder), DEMO_HOME (the HOME Pitty runs
//! in; its paths appear on screen), PITTY_BIN (the pitty binary), FONTS_DIR
//! (defaults to site/static/fonts), and OPENROUTER_API_KEY, which is passed to
//! Pitty and nowhere else.
//!
//! Scenes run in their own HOME, so Pitty's settings, chats, and decision log
//! are the demo's own. Waits for the model are recorded in real time and
//! compressed when rendering; those frames carry a "Sped up" badge, so the
//! video never hides that time was shortened.
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, String>;

const COLS: u16 = 88;
const ROWS: u16 = 25;
const INK: &str = "#1d2530";
const PAPER: &str = "#f8f9fb";
const REVIEW: &str = "Y run once · A approve all";
const CHECK_IN: &str = "Y continue · A continue";
const READY: &str = "Ctrl+Y copies the last reply";
const PROMPT: &str = "Message, or / for commands";
const FPS: f64 = 30.0;
/// The longest a sped-up wait lasts in the video, in seconds.
const FAST_TARGET: f64 = 1.4;
const FRAMES_PER_PAGE: usize = 8;
const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

#[derive(Serialize, Deserialize, Clone, PartialEq)]
struct Frame {
    t: u64,
    html: String,
    caption: String,
    fast: bool,
}

#[derive(Serialize, Deserialize)]
struct Take {
    scene: String,
    frames: Vec<Frame>,
    end: u64,
}

struct State {
    parser: vt100::Parser,
    caption: String,
    fast: bool,
    frames: Vec<Frame>,
    started: Instant,
}

fn env_path(name: &str) -> Result<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{name} is not set"))
}

fn io<T>(result: std::io::Result<T>, what: &str) -> Result<T> {
    result.map_err(|error| format!("{what}: {error}"))
}

// ---------------------------------------------------------------- screen

fn palette(index: u8) -> String {
    const BASE: [&str; 16] = [
        "#1d2530", "#ba242a", "#12803a", "#b05c08", "#184eba", "#7a2ab0", "#0e7490", "#4b5666",
        "#6b7686", "#d63a3a", "#1a9a4a", "#c47a10", "#2a66d6", "#9440c8", "#1590a8", "#f8f9fb",
    ];
    match index {
        0..=15 => BASE[index as usize].into(),
        16..=231 => {
            let n = index - 16;
            let step = |v: u8| [0u8, 95, 135, 175, 215, 255][v as usize];
            format!(
                "#{:02x}{:02x}{:02x}",
                step(n / 36),
                step(n / 6 % 6),
                step(n % 6)
            )
        }
        _ => {
            let v = 8 + (index - 232) * 10;
            format!("#{v:02x}{v:02x}{v:02x}")
        }
    }
}

fn color(value: vt100::Color) -> Option<String> {
    match value {
        vt100::Color::Default => None,
        vt100::Color::Idx(index) => Some(palette(index)),
        vt100::Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// One `<div class="r">` per row, with a `<span>` per run of equal style.
fn screen_html(screen: &vt100::Screen) -> String {
    let (rows, cols) = screen.size();
    let (cursor_row, cursor_col) = screen.cursor_position();
    let show_cursor = !screen.hide_cursor();
    let mut out = String::new();
    for row in 0..rows {
        out.push_str("<div class=\"r\">");
        let mut style = String::new();
        let mut text = String::new();
        let flush = |out: &mut String, style: &str, text: &mut String| {
            if text.is_empty() {
                return;
            }
            if style.is_empty() {
                out.push_str(&escape(text));
            } else {
                out.push_str(&format!("<span style=\"{style}\">{}</span>", escape(text)));
            }
            text.clear();
        };
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let mut fg = color(cell.fgcolor());
            let mut bg = color(cell.bgcolor());
            if cell.inverse() {
                let (f, b) = (fg.take(), bg.take());
                fg = Some(b.unwrap_or_else(|| PAPER.into()));
                bg = Some(f.unwrap_or_else(|| INK.into()));
            }
            if show_cursor && row == cursor_row && col == cursor_col {
                fg = Some(PAPER.into());
                bg = Some(INK.into());
            }
            let mut cell_style = String::new();
            if let Some(fg) = fg {
                cell_style.push_str(&format!("color:{fg};"));
            }
            if let Some(bg) = bg {
                cell_style.push_str(&format!("background:{bg};"));
            }
            if cell.bold() {
                cell_style.push_str("font-weight:700;");
            }
            if cell.italic() {
                cell_style.push_str("font-style:italic;");
            }
            if cell.underline() {
                cell_style.push_str("text-decoration:underline;");
            }
            if cell.is_wide() {
                cell_style.push_str("display:inline-block;width:2ch;");
            }
            let contents = cell.contents();
            let glyph = if contents.is_empty() {
                " ".to_string()
            } else {
                contents.to_string()
            };
            if cell_style != style || cell.is_wide() {
                flush(&mut out, &style, &mut text);
                style = cell_style;
            }
            text.push_str(&glyph);
        }
        flush(&mut out, &style, &mut text);
        out.push_str("</div>");
    }
    out
}

fn snapshot(state: &Mutex<State>) {
    let mut state = state.lock().unwrap();
    let frame = Frame {
        t: state.started.elapsed().as_millis() as u64,
        html: screen_html(state.parser.screen()),
        caption: state.caption.clone(),
        fast: state.fast,
    };
    let changed = state.frames.last().is_none_or(|last| {
        last.html != frame.html || last.caption != frame.caption || last.fast != frame.fast
    });
    if changed {
        state.frames.push(frame);
    }
}

/// End offsets of every occurrence of `needle` in `hay`.
fn ends(hay: &[u8], needle: &[u8]) -> Vec<usize> {
    hay.windows(needle.len())
        .enumerate()
        .filter(|(_, window)| *window == needle)
        .map(|(start, _)| start + needle.len())
        .collect()
}

// --------------------------------------------------------------- session

struct Session {
    state: Arc<Mutex<State>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    stop: Arc<AtomicBool>,
    ticker: Option<thread::JoinHandle<()>>,
    reader: Option<thread::JoinHandle<()>>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

impl Session {
    fn spawn(home: &Path, project: &Path) -> Result<Session> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| format!("Cannot open a pseudo-terminal: {error}"))?;
        let mut command = CommandBuilder::new(env_path("PITTY_BIN")?);
        command.arg("--directory");
        command.arg(project);
        command.cwd(project);
        command.env_clear();
        let real_home = env_path("HOME")?;
        command.env("HOME", home);
        command.env("PATH", std::env::var("PATH").unwrap_or_default());
        command.env("CARGO_HOME", real_home.join(".cargo"));
        command.env("RUSTUP_HOME", real_home.join(".rustup"));
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("LANG", "en_US.UTF-8");
        if let Ok(key) = std::env::var("OPENROUTER_API_KEY") {
            command.env("OPENROUTER_API_KEY", key);
        }
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| format!("Cannot start Pitty: {error}"))?;
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| error.to_string())?;
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(
            pair.master
                .take_writer()
                .map_err(|error| error.to_string())?,
        ));
        let state = Arc::new(Mutex::new(State {
            parser: vt100::Parser::new(ROWS, COLS, 0),
            caption: String::new(),
            fast: false,
            frames: Vec::new(),
            started: Instant::now(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let reader_thread = {
            let state = state.clone();
            let writer = writer.clone();
            thread::spawn(move || {
                let mut buffer = [0u8; 16384];
                let mut tail: Vec<u8> = Vec::new();
                loop {
                    let count = match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => count,
                    };
                    let chunk = &buffer[..count];
                    let (row, col) = {
                        let mut state = state.lock().unwrap();
                        state.parser.process(chunk);
                        state.parser.screen().cursor_position()
                    };
                    // Answer the terminal queries Pitty sends: background
                    // colour (light, like Pitty's light theme), device
                    // attributes, and cursor position.
                    let mut hay = tail.clone();
                    hay.extend_from_slice(chunk);
                    let queries: [(&[u8], Vec<u8>); 3] = [
                        (b"\x1b]11;?", b"\x1b]11;rgb:f8f8/f9f9/fbfb\x1b\\".to_vec()),
                        (b"\x1b[c", b"\x1b[?62;22c".to_vec()),
                        (
                            b"\x1b[6n",
                            format!("\x1b[{};{}R", row + 1, col + 1).into_bytes(),
                        ),
                    ];
                    let mut replies = Vec::new();
                    for (needle, reply) in &queries {
                        for end in ends(&hay, needle) {
                            if end > tail.len() {
                                replies.push(reply.clone());
                            }
                        }
                    }
                    if !replies.is_empty() {
                        let mut writer = writer.lock().unwrap();
                        for reply in replies {
                            let _ = writer.write_all(&reply);
                        }
                        let _ = writer.flush();
                    }
                    let keep = hay.len().saturating_sub(8);
                    tail = hay[keep..].to_vec();
                }
            })
        };
        let ticker = {
            let state = state.clone();
            let stop = stop.clone();
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    snapshot(&state);
                    thread::sleep(Duration::from_millis(33));
                }
            })
        };
        Ok(Session {
            state,
            writer,
            child,
            stop,
            ticker: Some(ticker),
            reader: Some(reader_thread),
            _master: pair.master,
        })
    }

    fn screen(&self) -> String {
        self.state.lock().unwrap().parser.screen().contents()
    }

    fn send(&self, bytes: &[u8]) {
        let mut writer = self.writer.lock().unwrap();
        let _ = writer.write_all(bytes);
        let _ = writer.flush();
    }

    /// Type like a person: one character at a time, 32–62 ms apart.
    fn type_text(&self, text: &str) {
        let mut seed: u32 = 2_463_534_242;
        for ch in text.chars() {
            let mut bytes = [0u8; 4];
            self.send(ch.encode_utf8(&mut bytes).as_bytes());
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            thread::sleep(Duration::from_millis(32 + u64::from(seed % 31)));
        }
    }

    fn hold(&self, millis: u64) {
        thread::sleep(Duration::from_millis(millis));
    }

    fn caption(&self, text: &str) {
        self.state.lock().unwrap().caption = text.into();
    }

    fn fast(&self, on: bool) {
        snapshot(&self.state);
        self.state.lock().unwrap().fast = on;
    }

    fn wait_for(&self, needle: &str, seconds: u64) -> Result<()> {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(seconds) {
            if self.screen().contains(needle) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }
        Err(format!(
            "Timed out waiting for {needle:?}. Screen:\n{}",
            self.screen()
        ))
    }

    /// Wait out one request: show each review long enough to read, answer Y,
    /// and stop when Pitty reports the reply is ready.
    fn turn(&self, seen: &mut HashSet<&'static str>) -> Result<()> {
        self.fast(true);
        let _ = self.wait_for("Thinking", 15);
        let started = Instant::now();
        let mut quiet_since: Option<Instant> = None;
        let live = env_path("DEMO_ROOT")?.join("live-screen.txt");
        let mut written = Instant::now() - Duration::from_secs(2);
        loop {
            let screen = self.screen();
            if written.elapsed() >= Duration::from_secs(1) {
                let _ = fs::write(&live, &screen);
                written = Instant::now();
            }
            let busy = ["Thinking…", "Working", "Model turn", "Running", "Stopping"]
                .iter()
                .any(|signal| screen.contains(signal));
            if busy {
                quiet_since = None;
            } else if !screen.contains(REVIEW) && !screen.contains(CHECK_IN) {
                let since = *quiet_since.get_or_insert_with(Instant::now);
                if since.elapsed() > Duration::from_secs(4) && !screen.contains(READY) {
                    self.fast(false);
                    return Err(format!("Pitty stopped without a reply:\n{screen}"));
                }
            }
            if screen.contains(REVIEW) || screen.contains(CHECK_IN) {
                let kind = classify(&screen);
                self.fast(false);
                let first = seen.insert(kind);
                if first {
                    self.caption(caption_for(kind));
                }
                self.hold(if first { 4200 } else { 1600 });
                self.send(b"y");
                self.hold(300);
                self.fast(true);
                continue;
            }
            if screen.contains(READY) && !screen.contains("Thinking…") {
                self.fast(false);
                return Ok(());
            }
            if screen.contains("shows completed work") || screen.contains("Token limit reached") {
                self.fast(false);
                return Err(format!("The request ended early:\n{screen}"));
            }
            if started.elapsed() > Duration::from_secs(900) {
                self.fast(false);
                return Err(format!("The request took too long:\n{screen}"));
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    fn finish(mut self, scene: &str) -> Take {
        snapshot(&self.state);
        let end = self.state.lock().unwrap().started.elapsed().as_millis() as u64;
        self.stop.store(true, Ordering::Relaxed);
        if let Some(ticker) = self.ticker.take() {
            let _ = ticker.join();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        let frames = std::mem::take(&mut self.state.lock().unwrap().frames);
        Take {
            scene: scene.into(),
            frames,
            end,
        }
    }
}

fn classify(screen: &str) -> &'static str {
    if screen.contains(CHECK_IN) {
        return "continue";
    }
    let header = screen
        .lines()
        .find(|line| line.trim_start().starts_with("Review ·"))
        .unwrap_or("");
    let action = header.rsplit(" · ").next().unwrap_or("").trim();
    let starts = |prefixes: &[&str]| prefixes.iter().any(|prefix| action.starts_with(prefix));
    if starts(&["Run"]) {
        "run"
    } else if starts(&["Search the web", "Fetch"]) {
        "web"
    } else if starts(&["Read"]) {
        "read"
    } else if starts(&[
        "Edit", "Write", "Create", "Replace", "Delete", "Move", "Rename",
    ]) {
        "file"
    } else {
        "other"
    }
}

fn caption_for(kind: &str) -> &'static str {
    match kind {
        "file" => {
            "Every file change appears exactly as it will be written. Nothing lands until you press Y."
        }
        "run" => "Commands wait too: the exact command and folder, shown before anything runs.",
        "web" => "Searches and page fetches wait for a yes, like everything else.",
        "read" => "Files that may hold credentials need their own approval.",
        "continue" => "Long tasks check in every 16 turns.",
        _ => "Every action waits for your answer.",
    }
}

// ----------------------------------------------------------------- scenes

const CARGO_TOML: &str =
    "[package]\nname = \"hello\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\n";
const MAIN_RS: &str = "fn main() {\n    println!(\"Hello\");\n}\n";

fn reset_home(home: &Path) -> Result<()> {
    if home.exists() {
        io(fs::remove_dir_all(home), "Cannot reset the demo home")?;
    }
    let project = home.join("src/hello");
    io(
        fs::create_dir_all(project.join("src")),
        "Cannot create the demo project",
    )?;
    io(
        fs::write(project.join("Cargo.toml"), CARGO_TOML),
        "Cannot write Cargo.toml",
    )?;
    io(
        fs::write(project.join("src/main.rs"), MAIN_RS),
        "Cannot write main.rs",
    )?;
    Ok(())
}

fn record(name: &str) -> Result<Take> {
    let root = env_path("DEMO_ROOT")?;
    // A short, neutral HOME: Pitty prints absolute paths in its reviews.
    let home = env_path("DEMO_HOME").unwrap_or_else(|_| root.join("home"));
    if name == "edit" || !home.exists() {
        reset_home(&home)?;
    }
    let project = home.join("src/hello");
    let session = Session::spawn(&home, &project)?;
    session.wait_for(PROMPT, 30)?;
    session.hold(900);
    let mut seen = HashSet::new();
    match name {
        "edit" => {
            session.caption(
                "Pitty is an AI agent for your terminal. Ask for a change in plain words.",
            );
            session.hold(2200);
            session.type_text(
                "Add a --name option to src/main.rs, using only the standard library, then run it with --name Ada.",
            );
            session.hold(900);
            session.send(b"\r");
            session.turn(&mut seen)?;
            session.caption(
                "Each action ran only after a yes, and its result went back to the model.",
            );
            session.hold(5200);
        }
        "web" => {
            session.caption("Every model can search the web. Here Pitty searches DuckDuckGo itself, with no key.");
            session.hold(1800);
            session.type_text("/web duckduckgo");
            session.hold(600);
            session.send(b"\r");
            session.hold(2600);
            session.type_text(
                "What are the three ownership rules in the Rust Book? Quote them and cite the page you used.",
            );
            session.hold(900);
            session.send(b"\r");
            session.turn(&mut seen)?;
            session.caption("The answer lists the sources it used.");
            session.hold(5500);
        }
        "auto" => {
            session.caption("Trust the task? Shift+Tab switches from Ask to Auto.");
            session.hold(2200);
            session.send(b"\x1b[Z");
            session.hold(2000);
            session.caption("In Auto, the work keeps going without stopping, and every action is still recorded.");
            session.type_text("Add a unit test for the greeting, then run cargo test.");
            session.hold(900);
            session.send(b"\r");
            session.turn(&mut seen)?;
            session.hold(4200);
        }
        "audit" => {
            session.caption(
                "Every decision is kept in a hash-chained log on your computer. /audit checks it.",
            );
            session.hold(2000);
            session.type_text("/audit");
            session.hold(600);
            session.send(b"\r");
            session.wait_for("Chain intact", 20)?;
            session.hold(5000);
        }
        _ => {
            return Err(format!(
                "Unknown scene {name}; use edit, web, auto, or audit."
            ));
        }
    }
    Ok(session.finish(name))
}

// ----------------------------------------------------------------- render

const CSS: &str = r#"
@font-face { font-family: Next; src: url(atkinson-hyperlegible-next.woff2); font-weight: 200 800; }
@font-face { font-family: Mono; src: url(atkinson-hyperlegible-mono.woff2); font-weight: 200 800; }
* { margin: 0; box-sizing: border-box; }
body { width: 1920px; background: #eef1f5; }
.frame { position: relative; width: 1920px; height: 1080px; overflow: hidden; background: #eef1f5; font-family: Next, sans-serif; color: #1d2530; }
.window { position: absolute; top: 26px; left: 50%; transform: translateX(-50%); background: #f8f9fb; border: 2px solid #c3cdd8; border-radius: 18px; box-shadow: 0 20px 60px rgba(29, 37, 48, 0.16); overflow: hidden; }
.bar { display: flex; align-items: center; gap: 16px; height: 54px; padding: 0 22px; background: #e7edf5; border-bottom: 2px solid #c3cdd8; }
.wm { display: inline-flex; border-radius: 7px; overflow: hidden; }
.wm b { font: 700 20px/1 Mono, monospace; padding: 5px 7px; }
.p { background: #ffd6d2; color: #ba242a; }
.i { background: #ffe8c4; color: #b05c08; }
.t1 { background: #d6f2dc; color: #12803a; }
.t2 { background: #d2e6ff; color: #184eba; }
.y { background: #ecd6ff; color: #7a2ab0; }
.where { font: 400 20px Mono, monospace; color: #4b5666; }
.badge { margin-left: auto; font: 700 24px Next, sans-serif; color: #7a2ab0; background: #ecd6ff; padding: 4px 16px; border-radius: 999px; }
.term { padding: 16px 24px 18px; font: 400 25px/30px Menlo, monospace; white-space: pre; width: calc(88ch + 48px); color: #1d2530; }
.r { height: 30px; }
.caption { position: absolute; top: 900px; left: 50%; transform: translateX(-50%); width: 1560px; text-align: center; font: 700 34px/1.3 Next, sans-serif; color: #1d2530; }
.band { position: absolute; left: 0; bottom: 0; width: 1920px; height: 12px; display: flex; }
.band i { flex: 1; }
.card { position: absolute; inset: 0; padding: 140px 180px 160px; display: flex; flex-direction: column; justify-content: center; gap: 44px; }
.card .wm { align-self: flex-start; border-radius: 12px; }
.card .wm b { font-size: 48px; padding: 14px 16px; }
.card h1 { font: 800 120px/1.04 Next, sans-serif; letter-spacing: -2px; }
.card p { font: 400 38px/1.4 Next, sans-serif; color: #4b5666; max-width: 1400px; }
.card .links { font: 700 44px/1.5 Mono, monospace; color: #195697; }
"#;

const WORDMARK: &str = r#"<span class="wm"><b class="p">P</b><b class="i">I</b><b class="t1">T</b><b class="t2">T</b><b class="y">Y</b></span>"#;
const BAND: &str = r#"<div class="band"><i class="p"></i><i class="i"></i><i class="t1"></i><i class="t2"></i><i class="y"></i></div>"#;

fn frame_body(frame: &Frame, speed: f64) -> String {
    let badge = if frame.fast && speed >= 1.5 {
        format!("<span class=\"badge\">Sped up {speed:.0}×</span>")
    } else {
        String::new()
    };
    let caption = if frame.caption.is_empty() {
        String::new()
    } else {
        format!("<p class=\"caption\">{}</p>", escape(&frame.caption))
    };
    format!(
        "<div class=\"window\"><div class=\"bar\">{WORDMARK}<span class=\"where\">~/src/hello</span>{badge}</div><div class=\"term\">{}</div></div>{caption}{BAND}",
        frame.html
    )
}

fn title_card() -> String {
    format!(
        "<div class=\"card\">{WORDMARK}<h1>The model proposes.<br>You press Y.</h1><p>A real session with Pitty 0.8.0 and a free cloud model. Waits for the model are sped up, and marked.</p></div>{BAND}"
    )
}

fn end_card() -> String {
    format!(
        "<div class=\"card\">{WORDMARK}<h1>Free and open source,<br>for macOS and Linux.</h1><p class=\"links\">pitty.tech<br>github.com/RuptionAI/pitty</p><p>Ruption AI, Inc.</p></div>{BAND}"
    )
}

/// Frame bodies and how long each shows, resampled to 30 fps, with each
/// sped-up stretch compressed to at most FAST_TARGET seconds.
fn scene_items(take: &Take) -> Vec<(String, f64)> {
    let frames = &take.frames;
    let count = frames.len();
    let mut durations: Vec<f64> = (0..count)
        .map(|i| {
            let next = frames.get(i + 1).map_or(take.end, |frame| frame.t);
            next.saturating_sub(frames[i].t) as f64 / 1000.0
        })
        .collect();
    let mut speeds = vec![1.0f64; count];
    let mut i = 0;
    while i < count {
        if frames[i].fast {
            let j = (i..count).find(|&k| !frames[k].fast).unwrap_or(count);
            let total: f64 = durations[i..j].iter().sum();
            if total > FAST_TARGET {
                let factor = total / FAST_TARGET;
                for k in i..j {
                    durations[k] /= factor;
                    speeds[k] = factor;
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    let mut starts = Vec::with_capacity(count);
    let mut clock = 0.0;
    for duration in &durations {
        starts.push(clock);
        clock += duration;
    }
    let ticks = (clock * FPS).round() as usize;
    let mut items: Vec<(String, f64)> = Vec::new();
    let mut k = 0;
    for tick in 0..ticks {
        let time = tick as f64 / FPS;
        while k + 1 < count && starts[k + 1] <= time {
            k += 1;
        }
        let body = frame_body(&frames[k], speeds[k]);
        match items.last_mut() {
            Some((last, seconds)) if *last == body => *seconds += 1.0 / FPS,
            _ => items.push((body, 1.0 / FPS)),
        }
    }
    items
}

fn run(command: &mut Command, what: &str) -> Result<()> {
    let status = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("{what}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{what} failed ({status})"))
    }
}

fn draw_page(dir: &Path, page: usize, bodies: &[&String], first: usize) -> Result<()> {
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>frames</title><style>{CSS}</style></head><body>{}</body></html>",
        bodies
            .iter()
            .map(|body| format!("<div class=\"frame\">{body}</div>"))
            .collect::<String>()
    );
    let html_path = dir.join(format!("page{page:04}.html"));
    let png_path = dir.join(format!("page{page:04}.png"));
    io(fs::write(&html_path, html), "Cannot write a frame page")?;
    run(
        Command::new(CHROME)
            .args([
                "--headless=new",
                "--disable-gpu",
                "--hide-scrollbars",
                "--allow-file-access-from-files",
                "--host-resolver-rules=MAP * ~NOTFOUND",
                "--force-device-scale-factor=1",
                "--virtual-time-budget=1500",
            ])
            .arg(format!("--window-size=1920,{}", 1080 * bodies.len()))
            .arg(format!(
                "--user-data-dir={}",
                dir.join(format!("chrome{}", page % 4)).display()
            ))
            .arg(format!("--screenshot={}", png_path.display()))
            .arg(format!("file://{}", html_path.display())),
        "Chrome",
    )?;
    if !png_path.is_file() {
        return Err(format!("Chrome drew nothing for page {page}"));
    }
    let mut filter = format!("[0]split={}", bodies.len());
    for k in 0..bodies.len() {
        filter.push_str(&format!("[s{k}]"));
    }
    for k in 0..bodies.len() {
        filter.push_str(&format!(";[s{k}]crop=1920:1080:0:{}[o{k}]", k * 1080));
    }
    let mut ffmpeg = Command::new("ffmpeg");
    ffmpeg
        .args(["-loglevel", "error", "-y", "-i"])
        .arg(&png_path)
        .arg("-filter_complex")
        .arg(filter);
    for k in 0..bodies.len() {
        ffmpeg
            .arg("-map")
            .arg(format!("[o{k}]"))
            .arg(dir.join(format!("f{:05}.png", first + k)));
    }
    run(&mut ffmpeg, "ffmpeg crop")?;
    let _ = fs::remove_file(png_path);
    let _ = fs::remove_file(html_path);
    Ok(())
}

fn render(scenes: &[String]) -> Result<()> {
    let root = env_path("DEMO_ROOT")?;
    let fonts = env_path("FONTS_DIR")
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../static/fonts"));
    let dir = root.join("render");
    if dir.exists() {
        io(fs::remove_dir_all(&dir), "Cannot clear render/")?;
    }
    io(fs::create_dir_all(&dir), "Cannot create render/")?;
    for font in [
        "atkinson-hyperlegible-next.woff2",
        "atkinson-hyperlegible-mono.woff2",
    ] {
        io(
            fs::copy(fonts.join(font), dir.join(font)),
            "Cannot copy a font",
        )?;
    }
    let mut items: Vec<(String, f64)> = vec![(title_card(), 3.6)];
    for scene in scenes {
        let path = root.join("takes").join(format!("{scene}.json"));
        let take: Take = serde_json::from_slice(&io(fs::read(&path), "Cannot read a take")?)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        items.extend(scene_items(&take));
    }
    items.push((end_card(), 5.0));
    // Draw each distinct frame once.
    let mut index: HashMap<&String, usize> = HashMap::new();
    let mut unique: Vec<&String> = Vec::new();
    for (body, _) in &items {
        if !index.contains_key(body) {
            index.insert(body, unique.len());
            unique.push(body);
        }
    }
    let pages: Vec<(usize, &[&String])> = unique.chunks(FRAMES_PER_PAGE).enumerate().collect();
    let errors: Mutex<Vec<String>> = Mutex::new(Vec::new());
    thread::scope(|scope| {
        for worker in 0..4 {
            let pages = &pages;
            let dir = &dir;
            let errors = &errors;
            scope.spawn(move || {
                for (page, chunk) in pages.iter().filter(|(page, _)| page % 4 == worker) {
                    if let Err(error) = draw_page(dir, *page, chunk, page * FRAMES_PER_PAGE) {
                        errors.lock().unwrap().push(error);
                    }
                }
            });
        }
    });
    let errors = errors.into_inner().unwrap();
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    let mut list = String::from("ffconcat version 1.0\n");
    let mut total = 0.0;
    for (body, seconds) in &items {
        list.push_str(&format!(
            "file 'f{:05}.png'\nduration {seconds:.4}\n",
            index[body]
        ));
        total += seconds;
    }
    let last = index[&items.last().unwrap().0];
    list.push_str(&format!("file 'f{last:05}.png'\n"));
    io(
        fs::write(dir.join("list.ffconcat"), list),
        "Cannot write the frame list",
    )?;
    let video = dir.join("pitty-demo.mp4");
    run(
        Command::new("ffmpeg")
            .args([
                "-loglevel",
                "error",
                "-y",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
            ])
            .arg(dir.join("list.ffconcat"))
            .args([
                "-vf",
                "fps=30,format=yuv420p",
                "-c:v",
                "libx264",
                "-preset",
                "slow",
                "-crf",
                "20",
                "-movflags",
                "+faststart",
            ])
            // The concat demuxer holds the repeated last frame too long; cut
            // the video at the planned length.
            .arg("-t")
            .arg(format!("{total:.3}"))
            .arg(&video),
        "ffmpeg encode",
    )?;
    io(
        fs::copy(dir.join("f00000.png"), dir.join("poster.png")),
        "Cannot write the poster",
    )?;
    println!(
        "{} distinct frames, {total:.1} seconds: {}",
        unique.len(),
        video.display()
    );
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("record") => args[1..].iter().try_for_each(|scene| {
            let take = record(scene)?;
            let dir = env_path("DEMO_ROOT")?.join("takes");
            io(fs::create_dir_all(&dir), "Cannot create takes/")?;
            let json = serde_json::to_vec(&take).map_err(|error| error.to_string())?;
            io(
                fs::write(dir.join(format!("{scene}.json")), json),
                "Cannot save the take",
            )?;
            println!(
                "{scene}: {} frames over {:.1} s",
                take.frames.len(),
                take.end as f64 / 1000.0
            );
            Ok(())
        }),
        Some("render") => render(&args[1..]),
        _ => Err("Usage: pitty-demo-recorder record SCENE... | render SCENE...".into()),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_terminal_queries_pitty_sends() {
        assert_eq!(ends(b"x\x1b]11;?\x1b\\", b"\x1b]11;?"), vec![7]);
        assert!(ends(b"plain text", b"\x1b[c").is_empty());
    }

    #[test]
    fn names_the_kind_of_review() {
        let review = |action: &str| format!("  PITTY\n  Review · Pitty · {action}\n");
        assert_eq!(classify(&review("Edit /tmp/x/src/main.rs")), "file");
        assert_eq!(classify(&review("Run command in /tmp/x")), "run");
        assert_eq!(classify(&review("Search the web: rust ownership")), "web");
        assert_eq!(classify(&review("Fetch https://example.com/")), "web");
        assert_eq!(
            classify(&format!("{CHECK_IN} · N / Enter stop")),
            "continue"
        );
    }

    #[test]
    fn long_waits_are_shortened_and_marked() {
        let frame = |t: u64, html: &str, fast: bool| Frame {
            t,
            html: html.into(),
            caption: "c".into(),
            fast,
        };
        let take = Take {
            scene: "t".into(),
            frames: vec![
                frame(0, "typing", false),
                frame(1_000, "waiting", true),
                frame(31_000, "answer", false),
            ],
            end: 33_000,
        };
        let items = scene_items(&take);
        let total: f64 = items.iter().map(|(_, seconds)| seconds).sum();
        // One second of typing, thirty of waiting shown in FAST_TARGET, two of answer.
        assert!((total - (3.0 + FAST_TARGET)).abs() < 0.1, "{total}");
        let waiting = items
            .iter()
            .find(|(body, _)| body.contains("waiting"))
            .unwrap();
        assert!(waiting.0.contains("Sped up 21×"), "{}", waiting.0);
        assert!(!items[0].0.contains("Sped up"));
    }

    #[test]
    fn screen_text_is_escaped_and_coloured() {
        let mut parser = vt100::Parser::new(2, 10, 0);
        parser.process(b"\x1b[?25la<b \x1b[31mred");
        let html = screen_html(parser.screen());
        assert!(html.contains("a&lt;b"), "{html}");
        assert!(html.contains("color:#ba242a;\">red"), "{html}");
    }
}
