//! Pitty: a terminal renderer, editor, and input decoder with no dependencies.
use std::{
    fs::File,
    io::{self, Read, Write},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    Down,
    Drag,
    Up,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    AltEnter,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Tab,
    Escape,
    PageUp,
    PageDown,
    F2,
    /// Mouse wheel position in zero-based terminal cells.
    Scroll {
        up: bool,
        x: usize,
        y: usize,
    },
    /// Left-button events in zero-based terminal cells.
    Mouse {
        kind: MouseKind,
        x: usize,
        y: usize,
    },
    Ctrl(char),
    Paste(String),
    PasteRejected,
    Eof,
    Unknown,
}

/// Strip terminal commands and invisible directional overrides from untrusted text.
pub fn sanitize(text: &str) -> String {
    sanitize_inner(text, false)
}
/// Preserve code tabs on the clipboard while removing terminal control sequences.
pub fn sanitize_copy(text: &str) -> String {
    sanitize_inner(text, true)
}
fn sanitize_inner(text: &str, preserve_tabs: bool) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.next() {
                Some('[') => {
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']' | 'P' | '_' | '^') => {
                    while let Some(next) = chars.next() {
                        if next == '\x07' || (next == '\x1b' && chars.peek() == Some(&'\\')) {
                            if next == '\x1b' {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if c == '\n' {
            out.push(c);
        } else if c == '\t' {
            if preserve_tabs {
                out.push('\t');
            } else {
                out.push_str("    ");
            }
        } else if !c.is_control() && !matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}') {
            out.push(c);
        }
    }
    out
}
pub fn char_width(c: char) -> usize {
    let n = c as u32;
    if c.is_control()
        || matches!(n,0x0300..=0x036f|0x1ab0..=0x1aff|0x1dc0..=0x1dff|0x200b..=0x200f|0x20d0..=0x20ff|0xfe00..=0xfe0f|0xfe20..=0xfe2f|0x1f3fb..=0x1f3ff|0x2060..=0x2064|0xfeff|0xe0000..=0xe0fff)
    {
        0
    } else if matches!(n,0x1100..=0x115f|0x2329..=0x232a|0x2e80..=0xa4cf|0xac00..=0xd7a3|0xf900..=0xfaff|0xfe10..=0xfe19|0xfe30..=0xfe6f|0xff01..=0xff60|0xffe0..=0xffe6|0x1f300..=0x1faff|0x20000..=0x3fffd)
        || wide_emoji(n)
    {
        2
    } else {
        1
    }
}
/// Emoji outside the main emoji blocks that terminals draw two columns wide
/// (Unicode East Asian Width "W" with default emoji presentation).
fn wide_emoji(n: u32) -> bool {
    matches!(
        n,
        0x231a..=0x231b
            | 0x23e9..=0x23ec
            | 0x23f0
            | 0x23f3
            | 0x25fd..=0x25fe
            | 0x2614..=0x2615
            | 0x2648..=0x2653
            | 0x267f
            | 0x2693
            | 0x26a1
            | 0x26aa..=0x26ab
            | 0x26bd..=0x26be
            | 0x26c4..=0x26c5
            | 0x26ce
            | 0x26d4
            | 0x26ea
            | 0x26f2..=0x26f3
            | 0x26f5
            | 0x26fa
            | 0x26fd
            | 0x2705
            | 0x270a..=0x270b
            | 0x2728
            | 0x274c
            | 0x274e
            | 0x2753..=0x2755
            | 0x2757
            | 0x2795..=0x2797
            | 0x27b0
            | 0x27bf
            | 0x2b1b..=0x2b1c
            | 0x2b50
            | 0x2b55
            | 0x1f004
            | 0x1f0cf
            | 0x1f18e
            | 0x1f191..=0x1f19a
            | 0x1f200..=0x1f2ff
    )
}
pub fn width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}
pub fn wrap(text: &str, columns: usize) -> Vec<String> {
    let columns = columns.max(1);
    let mut result = vec![];
    for source in sanitize(text).split('\n') {
        let mut line = String::new();
        let mut count = 0;
        for c in source.chars() {
            let w = char_width(c);
            if count + w > columns && !line.is_empty() {
                result.push(std::mem::take(&mut line));
                count = 0;
            }
            line.push(c);
            count += w;
        }
        result.push(line);
    }
    result
}
/// Wrap prose at whitespace while retaining the existing character wrapper for code.
pub fn wrap_words(text: &str, columns: usize) -> Vec<String> {
    let columns = columns.max(1);
    let mut lines = Vec::new();
    for source in sanitize(text).split('\n') {
        let mut line = String::new();
        for word in source.split_whitespace() {
            if !line.is_empty() && width(&line) + 1 + width(word) > columns {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            let pieces = wrap(word, columns);
            for (i, piece) in pieces.iter().enumerate() {
                if i > 0 {
                    lines.push(std::mem::take(&mut line));
                }
                line.push_str(piece);
            }
        }
        lines.push(line);
    }
    lines
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub fg: Option<(u8, u8, u8)>,
    pub bg: Option<(u8, u8, u8)>,
    pub bold: bool,
}
impl Style {
    pub const fn new(fg: Option<(u8, u8, u8)>, bg: Option<(u8, u8, u8)>, bold: bool) -> Self {
        Self { fg, bg, bold }
    }
    fn ansi(self) -> String {
        let mut out = "\x1b[0m".to_string();
        if let Some((r, g, b)) = self.fg {
            out += &format!("\x1b[38;2;{r};{g};{b}m");
        }
        if let Some((r, g, b)) = self.bg {
            out += &format!("\x1b[48;2;{r};{g};{b}m");
        }
        if self.bold {
            out += "\x1b[1m";
        }
        out
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Cell {
    text: String,
    style: Style,
    continuation: bool,
}
#[derive(Clone, Debug)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    cells: Vec<Cell>,
    pub cursor: Option<(usize, usize)>,
}
impl Frame {
    pub fn new(width: usize, height: usize, style: Style) -> Self {
        Self {
            width,
            height,
            cells: vec![
                Cell {
                    text: " ".into(),
                    style,
                    continuation: false
                };
                width * height
            ],
            cursor: None,
        }
    }
    pub fn text(&mut self, x: usize, y: usize, text: &str, style: Style, max_width: usize) {
        if y >= self.height {
            return;
        }
        let mut x = x;
        let end = (x + max_width).min(self.width);
        for c in sanitize(text).chars().take_while(|c| *c != '\n') {
            let w = char_width(c);
            if w == 0 {
                if x > 0 && x <= self.width {
                    let mut prev = x - 1;
                    if self.cells[y * self.width + prev].continuation && prev > 0 {
                        prev -= 1;
                    }
                    self.cells[y * self.width + prev].text.push(c);
                }
                continue;
            }
            if x + w > end {
                break;
            }
            self.cells[y * self.width + x] = Cell {
                text: c.to_string(),
                style,
                continuation: false,
            };
            if w == 2 {
                self.cells[y * self.width + x + 1] = Cell {
                    text: String::new(),
                    style,
                    continuation: true,
                };
            }
            x += w;
        }
    }
    pub fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, style: Style) {
        for row in y..(y + h).min(self.height) {
            for col in x..(x + w).min(self.width) {
                self.cells[row * self.width + col] = Cell {
                    text: " ".into(),
                    style,
                    continuation: false,
                };
            }
        }
    }
    fn cell_range(&self, y: usize, start: usize, end: usize) -> Option<(usize, usize)> {
        let mut start = start.min(self.width);
        let mut end = end.min(self.width);
        if y >= self.height || start >= end {
            return None;
        }
        let row = y * self.width;
        // A selection touching either half of a wide glyph includes the whole
        // glyph. Combining characters already live in its leading cell.
        while start > 0 && self.cells[row + start].continuation {
            start -= 1;
        }
        while end < self.width && self.cells[row + end].continuation {
            end += 1;
        }
        Some((row + start, row + end))
    }
    /// Extract a half-open range of terminal cells without splitting wide glyphs
    /// or losing their combining marks. Blank cells are preserved.
    pub fn text_range(&self, y: usize, start: usize, end: usize) -> String {
        self.cell_range(y, start, end)
            .map_or_else(String::new, |(start, end)| {
                self.cells[start..end]
                    .iter()
                    .filter(|cell| !cell.continuation)
                    .map(|cell| cell.text.as_str())
                    .collect()
            })
    }
    /// Apply a selection style to the same complete glyphs returned by text_range.
    pub fn highlight_range(&mut self, y: usize, start: usize, end: usize, style: Style) {
        if let Some((start, end)) = self.cell_range(y, start, end) {
            for cell in &mut self.cells[start..end] {
                cell.style = style;
            }
        }
    }
    pub fn plain_text(&self) -> String {
        self.cells
            .chunks(self.width.max(1))
            .map(|row| {
                row.iter()
                    .map(|c| c.text.as_str())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Default, Debug)]
pub struct Editor {
    text: Vec<char>,
    cursor: usize,
}
impl Editor {
    pub fn value(&self) -> String {
        self.text.iter().collect()
    }
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
    pub fn set(&mut self, text: &str) {
        self.text = sanitize(text).chars().collect();
        self.cursor = self.text.len();
    }
    /// Insert sanitized text at the cursor. Returns false, leaving the draft
    /// unchanged, when the result would exceed the 1,048,576-character limit.
    pub fn insert(&mut self, text: &str) -> bool {
        let chars: Vec<char> = sanitize(text).chars().collect();
        let count = chars.len();
        if self.text.len() + count > 1_048_576 {
            return false;
        }
        self.text.splice(self.cursor..self.cursor, chars);
        self.cursor += count;
        true
    }
    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }
    pub fn edit(&mut self, key: &Key) {
        match key {
            Key::Char(c) => {
                self.insert(&c.to_string());
            }
            Key::Paste(s) => {
                self.insert(s);
            }
            Key::AltEnter => {
                self.insert("\n");
            }
            Key::Left => self.cursor = self.cursor.saturating_sub(1),
            Key::Right => self.cursor = (self.cursor + 1).min(self.text.len()),
            Key::Home | Key::Ctrl('a') => {
                while self.cursor > 0 && self.text[self.cursor - 1] != '\n' {
                    self.cursor -= 1;
                }
            }
            Key::End | Key::Ctrl('e') => {
                while self.cursor < self.text.len() && self.text[self.cursor] != '\n' {
                    self.cursor += 1;
                }
            }
            Key::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.text.remove(self.cursor);
                }
            }
            Key::Delete => {
                if self.cursor < self.text.len() {
                    self.text.remove(self.cursor);
                }
            }
            Key::Ctrl('u') => self.clear(),
            // Kill to the end of the line, as readline does; at a line's end,
            // join the next line instead of discarding the rest of the draft.
            Key::Ctrl('k') => {
                let end = self.text[self.cursor..]
                    .iter()
                    .position(|c| *c == '\n')
                    .map_or(self.text.len(), |offset| self.cursor + offset);
                if end == self.cursor && end < self.text.len() {
                    self.text.remove(self.cursor);
                } else {
                    self.text.drain(self.cursor..end);
                }
            }
            Key::Up | Key::Down => {
                let start = self.text[..self.cursor]
                    .iter()
                    .rposition(|c| *c == '\n')
                    .map_or(0, |i| i + 1);
                let col = self.cursor - start;
                if *key == Key::Up && start > 0 {
                    let end = start - 1;
                    let previous = self.text[..end]
                        .iter()
                        .rposition(|c| *c == '\n')
                        .map_or(0, |i| i + 1);
                    self.cursor = (previous + col).min(end);
                } else if *key == Key::Down
                    && let Some(offset) = self.text[self.cursor..].iter().position(|c| *c == '\n')
                {
                    let start = self.cursor + offset + 1;
                    let end = self.text[start..]
                        .iter()
                        .position(|c| *c == '\n')
                        .map_or(self.text.len(), |n| start + n);
                    self.cursor = (start + col).min(end);
                }
            }
            _ => {}
        }
    }
    pub fn visual(&self, columns: usize) -> (Vec<String>, (usize, usize)) {
        let columns = columns.max(1);
        let mut lines = vec![String::new()];
        let (mut x, mut y) = (0, 0);
        let mut cursor = (0, 0);
        for (i, c) in self.text.iter().enumerate() {
            let w = char_width(*c);
            if *c != '\n' && x + w > columns {
                lines.push(String::new());
                y += 1;
                x = 0;
            }
            if i == self.cursor {
                cursor = (x, y);
            }
            if *c == '\n' {
                lines.push(String::new());
                y += 1;
                x = 0;
            } else {
                lines[y].push(*c);
                x += w;
            }
        }
        if self.cursor == self.text.len() {
            if x >= columns {
                lines.push(String::new());
                y += 1;
                x = 0;
            }
            cursor = (x, y);
        }
        (lines, cursor)
    }
}
#[derive(Default)]
pub struct Decoder {
    bytes: Vec<u8>,
    overflow: bool,
    mouse_overflow: bool,
    mouse_left_down: bool,
    paste_scan: usize,
}
impl Decoder {
    pub fn feed(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }
    fn track_mouse(&mut self, key: &Key) {
        if let Key::Mouse { kind, .. } = key {
            match kind {
                MouseKind::Down => self.mouse_left_down = true,
                MouseKind::Up => self.mouse_left_down = false,
                MouseKind::Drag => {}
            }
        }
    }
    pub fn next(&mut self, escape_timeout: bool) -> Option<Key> {
        let b = &self.bytes;
        if b.is_empty() {
            return None;
        }
        if b.starts_with(b"\x1b[200~") {
            // Retain only the five-byte overlap needed to recognize a marker
            // split across reads. Rescanning the complete paste on every read
            // makes a large paste quadratic in its length.
            let start = self.paste_scan.max(6).min(b.len());
            if let Some(end) = b[start..].windows(6).position(|w| w == b"\x1b[201~") {
                let end = end + start;
                let key = if self.overflow || end > 1_048_582 {
                    Key::PasteRejected
                } else {
                    Key::Paste(pasted(&b[6..end]))
                };
                self.bytes.drain(..end + 6);
                self.overflow = false;
                self.paste_scan = 0;
                return Some(key);
            }
            if b.len() > 1_048_588 {
                let suffix = b[b.len() - 6..].to_vec();
                self.bytes.truncate(6);
                self.bytes.extend(suffix);
                self.overflow = true;
                self.paste_scan = 6;
            } else {
                self.paste_scan = b.len().saturating_sub(5).max(6);
            }
            return None;
        }
        // SGR reports can arrive across arbitrary read boundaries. Consume the
        // whole report, including ignored clicks, so coordinates never become text.
        if b.starts_with(b"\x1b[<") {
            let end = b[3..]
                .iter()
                .position(|c| matches!(c, b'M' | b'm'))
                .map(|i| i + 3);
            let interrupted = b[3..]
                .iter()
                .position(|c| c.is_ascii_control())
                .map(|i| i + 3);
            if let Some(i) = interrupted.filter(|i| end.is_none_or(|end| *i < end)) {
                self.bytes.drain(..i);
                self.mouse_overflow = false;
                return Some(Key::Unknown);
            }
            if let Some(end) = end {
                let key = if !self.mouse_overflow && end <= 64 {
                    sgr_mouse(&b[3..end], b[end] == b'm')
                } else {
                    Key::Unknown
                };
                self.bytes.drain(..=end);
                self.mouse_overflow = false;
                self.track_mouse(&key);
                return Some(key);
            }
            if b.len() > 64 {
                // Keep a sentinel until the final marker, without buffering an
                // unbounded malformed report or leaking its remaining digits.
                self.bytes.truncate(3);
                self.mouse_overflow = true;
            }
            return None;
        }
        // Some terminals ignore SGR mode and still send six-byte X10 reports.
        // Consume each complete report; legacy release packets identify no
        // button, so only emit a release after an observed left-button press.
        if b.starts_with(b"\x1b[M") {
            // All three X10 payload bytes are offset by 32. An earlier control
            // byte therefore interrupts a malformed report; leave it queued so
            // Ctrl+C, Enter or a fresh escape sequence can still be handled.
            if let Some(offset) = b[3..b.len().min(6)].iter().position(|byte| *byte < 32) {
                self.bytes.drain(..3 + offset);
                return Some(Key::Unknown);
            }
            if b.len() < 6 {
                return None;
            }
            let key = match (
                b[3].checked_sub(32),
                b[4].checked_sub(32),
                b[5].checked_sub(32),
            ) {
                (Some(button), Some(x), Some(y)) => {
                    let button = u16::from(button);
                    if button & !(4 | 8 | 16) == 3 {
                        if self.mouse_left_down {
                            mouse(0, u16::from(x), u16::from(y), true)
                        } else {
                            Key::Unknown
                        }
                    } else {
                        mouse(button, u16::from(x), u16::from(y), false)
                    }
                }
                _ => Key::Unknown,
            };
            self.bytes.drain(..6);
            self.track_mouse(&key);
            return Some(key);
        }
        let (key, len) = match b[0] {
            27 => {
                let known: &[(&[u8], Key)] = &[
                    (b"\x1bOA", Key::Up),
                    (b"\x1bOB", Key::Down),
                    (b"\x1bOC", Key::Right),
                    (b"\x1bOD", Key::Left),
                    (b"\x1bOH", Key::Home),
                    (b"\x1bOF", Key::End),
                    (b"\x1bOQ", Key::F2),
                    (b"\x1b[A", Key::Up),
                    (b"\x1b[B", Key::Down),
                    (b"\x1b[C", Key::Right),
                    (b"\x1b[D", Key::Left),
                    (b"\x1b[H", Key::Home),
                    (b"\x1b[F", Key::End),
                    (b"\x1b[1~", Key::Home),
                    (b"\x1b[7~", Key::Home),
                    (b"\x1b[4~", Key::End),
                    (b"\x1b[8~", Key::End),
                    (b"\x1b[3~", Key::Delete),
                    (b"\x1b[5~", Key::PageUp),
                    (b"\x1b[6~", Key::PageDown),
                    (b"\x1b[12~", Key::F2),
                    (b"\x1b[[B", Key::F2),
                    (b"\x1b\r", Key::AltEnter),
                    (b"\x1b\n", Key::AltEnter),
                    // Shift+Enter and Alt+Enter from terminals that report
                    // modified keys (kitty protocol, xterm modifyOtherKeys).
                    (b"\x1b[13;2u", Key::AltEnter),
                    (b"\x1b[13;3u", Key::AltEnter),
                    (b"\x1b[27;2;13~", Key::AltEnter),
                    (b"\x1b[27;3;13~", Key::AltEnter),
                ];
                if let Some((seq, key)) = known.iter().find(|(seq, _)| b.starts_with(seq)) {
                    (key.clone(), seq.len())
                } else if b.len() == 1 {
                    if !escape_timeout {
                        return None;
                    }
                    (Key::Escape, 1)
                } else if b[1] == b']' {
                    // OSC strings, such as the reply to the background-color
                    // query, are consumed whole so they never become text.
                    match osc_end(b) {
                        Some(end) => (Key::Unknown, end),
                        None if b.len() > 512 => (Key::Unknown, b.len()),
                        None if !escape_timeout => return None,
                        None => (Key::Unknown, 2),
                    }
                } else if b[1] == b'O' || b.starts_with(b"\x1b[[") {
                    // SS3 keys (F1, F3, F4, keypad) and Linux console function
                    // keys are three and four bytes long; consume them whole so
                    // their final letter never reaches the draft.
                    let length = if b[1] == b'O' { 3 } else { 4 };
                    if b.len() < length && !escape_timeout {
                        return None;
                    }
                    (Key::Unknown, length.min(b.len()))
                } else if b[1] >= 0x80 {
                    // Alt plus a non-ASCII character: discard the whole character.
                    let length = 1 + utf8_length(b[1]);
                    if b.len() < length && !escape_timeout {
                        return None;
                    }
                    (Key::Unknown, length.min(b.len()))
                } else if b[1] == b'[' {
                    let end = b[2..].iter().position(|c| (0x40..=0x7e).contains(c));
                    let interrupted = b[2..].iter().position(|c| c.is_ascii_control());
                    if let Some(offset) = interrupted.filter(|i| end.is_none_or(|end| *i < end)) {
                        // Preserve Ctrl+C and new escapes after a malformed or
                        // incomplete CSI rather than waiting for more bytes.
                        (Key::Unknown, offset + 2)
                    } else if let Some(end) = end {
                        (Key::Unknown, end + 3)
                    } else if b.len() > 64 {
                        (Key::Unknown, b.len())
                    } else {
                        return None;
                    }
                } else if b[1].is_ascii_control() {
                    (Key::Unknown, 1)
                } else {
                    (Key::Unknown, 2)
                }
            }
            // Raw-mode Enter is CR. Ctrl+J sends LF, the reliable newline key on
            // terminals where Option/Alt+Return is not reported (Terminal.app's
            // default). A CR LF pair from a newline-mode terminal is one Enter.
            b'\r' => (Key::Enter, if b.get(1) == Some(&b'\n') { 2 } else { 1 }),
            b'\n' => (Key::AltEnter, 1),
            9 => (Key::Tab, 1),
            8 | 127 => (Key::Backspace, 1),
            1..=26 => (Key::Ctrl((b[0] + b'a' - 1) as char), 1),
            byte => {
                let len = utf8_length(byte);
                if b.len() < len && b[1..].iter().all(|v| v & 0xc0 == 0x80) {
                    return None;
                }
                let len = len.min(b.len());
                let c = std::str::from_utf8(&b[..len])
                    .ok()
                    .and_then(|s| s.chars().next())
                    .unwrap_or('\u{fffd}');
                (
                    Key::Char(c),
                    if c == '\u{fffd}' && std::str::from_utf8(&b[..len]).is_err() {
                        1
                    } else {
                        len
                    },
                )
            }
        };
        self.bytes.drain(..len);
        Some(key)
    }
}

/// Length through the BEL or ST that ends an OSC string starting at `b[0]`.
fn osc_end(b: &[u8]) -> Option<usize> {
    let mut index = 2;
    while index < b.len() {
        match b[index] {
            0x07 => return Some(index + 1),
            0x1b if b.get(index + 1) == Some(&b'\\') => return Some(index + 2),
            _ => index += 1,
        }
    }
    None
}

/// Parse `rgb:RRRR/GGGG/BBBB` (one to four hex digits per channel).
fn parse_rgb(text: &str) -> Option<(u8, u8, u8)> {
    let channels = text
        .strip_prefix("rgb:")
        .or_else(|| text.strip_prefix("rgba:"))?;
    let mut values = channels.split('/').map(|part| {
        let digits = part.len();
        let value = u32::from_str_radix(part, 16).ok()?;
        (1..=4)
            .contains(&digits)
            .then(|| (value * 255 / ((1 << (4 * digits)) - 1)) as u8)
    });
    Some((values.next()??, values.next()??, values.next()??))
}

impl Decoder {
    /// Replies to startup queries still in the input buffer: the terminal's
    /// background color (OSC 11) and whether device attributes (DA1) arrived.
    /// Scanning does not consume input, so keys typed meanwhile are kept.
    pub fn reports(&self) -> (Option<(u8, u8, u8)>, bool) {
        let b = &self.bytes;
        let mut background = None;
        let mut attributes = false;
        let mut index = 0;
        while index + 2 < b.len() {
            if b[index] == 0x1b && b[index + 1] == b']' {
                if let Some(end) = osc_end(&b[index..]) {
                    let body = String::from_utf8_lossy(&b[index + 2..index + end]);
                    let body = body.trim_end_matches(['\x07', '\x1b', '\\']);
                    if let Some(color) = body.strip_prefix("11;").and_then(parse_rgb) {
                        background = Some(color);
                    }
                    index += end;
                    continue;
                }
            } else if b[index] == 0x1b
                && b[index + 1] == b'['
                && b[index + 2] == b'?'
                && let Some(end) = b[index + 3..]
                    .iter()
                    .position(|c| !matches!(c, b'0'..=b'9' | b';'))
                && b[index + 3 + end] == b'c'
            {
                attributes = true;
            }
            index += 1;
        }
        (background, attributes)
    }
}

/// Terminals separate pasted lines with CR (xterm, VTE, Terminal.app, iTerm2,
/// tmux) or LF. Normalize both to LF before removing control characters, so a
/// multi-line paste keeps its lines.
fn pasted(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    sanitize(&text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// Length of the UTF-8 sequence a leading byte announces (1 for invalid bytes).
fn utf8_length(byte: u8) -> usize {
    if byte < 0x80 {
        1
    } else if byte & 0xe0 == 0xc0 {
        2
    } else if byte & 0xf0 == 0xe0 {
        3
    } else if byte & 0xf8 == 0xf0 {
        4
    } else {
        1
    }
}

fn mouse(button: u16, x: u16, y: u16, release: bool) -> Key {
    let Some(x) = x.checked_sub(1) else {
        return Key::Unknown;
    };
    let Some(y) = y.checked_sub(1) else {
        return Key::Unknown;
    };
    let x = usize::from(x);
    let y = usize::from(y);
    // Ignore modifiers, but reject other buttons and unsupported motion modes.
    let button = button & !(4 | 8 | 16);
    if release {
        return if button == 0 {
            Key::Mouse {
                kind: MouseKind::Up,
                x,
                y,
            }
        } else {
            Key::Unknown
        };
    }
    match button {
        0 => Key::Mouse {
            kind: MouseKind::Down,
            x,
            y,
        },
        32 => Key::Mouse {
            kind: MouseKind::Drag,
            x,
            y,
        },
        64 => Key::Scroll { up: true, x, y },
        65 => Key::Scroll { up: false, x, y },
        _ => Key::Unknown,
    }
}
fn sgr_mouse(report: &[u8], release: bool) -> Key {
    let Ok(report) = std::str::from_utf8(report) else {
        return Key::Unknown;
    };
    if report.bytes().any(|c| !c.is_ascii_digit() && c != b';') {
        return Key::Unknown;
    }
    let mut fields = report.split(';');
    let mut number = || fields.next().and_then(|s| s.parse::<u16>().ok());
    let values = (number(), number(), number());
    if fields.next().is_some() {
        return Key::Unknown;
    }
    match values {
        (Some(button), Some(x), Some(y)) => mouse(button, x, y, release),
        _ => Key::Unknown,
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn terminal_size(tty: &File) -> Option<(usize, usize)> {
    use std::os::fd::AsRawFd;
    #[repr(C)]
    #[derive(Default)]
    struct WindowSize {
        rows: u16,
        columns: u16,
        x_pixels: u16,
        y_pixels: u16,
    }
    unsafe extern "C" {
        fn ioctl(fd: std::ffi::c_int, request: std::ffi::c_ulong, ...) -> std::ffi::c_int;
    }
    #[cfg(target_os = "macos")]
    const TIOCGWINSZ: std::ffi::c_ulong = 0x40087468;
    #[cfg(target_os = "linux")]
    const TIOCGWINSZ: std::ffi::c_ulong = 0x5413;
    let mut size = WindowSize::default();
    // SAFETY: tty owns a live descriptor. TIOCGWINSZ writes one initialized,
    // repr(C) winsize structure, whose four unsigned-short fields match the OS ABI.
    let result = unsafe { ioctl(tty.as_raw_fd(), TIOCGWINSZ, &mut size as *mut WindowSize) };
    (result == 0 && size.rows > 0 && size.columns > 0)
        .then_some((usize::from(size.columns), usize::from(size.rows)))
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn terminal_size(_: &File) -> Option<(usize, usize)> {
    None
}

pub struct Terminal {
    saved: String,
    tty: File,
    rx: Receiver<Vec<u8>>,
    decoder: Decoder,
    previous: Option<Frame>,
    size: (usize, usize),
    sized: Instant,
    mouse: bool,
    background: Option<(u8, u8, u8)>,
}
impl Terminal {
    pub fn open() -> io::Result<Self> {
        let tty = File::open("/dev/tty")?;
        let saved = Command::new("stty")
            .arg("-g")
            .stdin(Stdio::from(tty.try_clone()?))
            .output()?;
        if !saved.status.success() {
            return Err(io::Error::other("Cannot read terminal settings"));
        }
        let saved = String::from_utf8_lossy(&saved.stdout).trim().to_string();
        let status = Command::new("stty")
            .args(["raw", "-echo"])
            .stdin(Stdio::from(tty.try_clone()?))
            .status()?;
        if !status.success() {
            return Err(io::Error::other("Cannot enter terminal input mode"));
        }
        let (tx, rx) = mpsc::sync_channel(16);
        std::thread::spawn(move || {
            let mut input = io::stdin().lock();
            let mut bytes = [0; 1024];
            loop {
                match input.read(&mut bytes) {
                    // A signal can interrupt a read; that is not end of input.
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(bytes[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let mut terminal = Self {
            saved,
            tty,
            rx,
            decoder: Decoder::default(),
            previous: None,
            size: (100, 30),
            sized: Instant::now() - Duration::from_secs(2),
            mouse: false,
            background: None,
        };
        write!(
            io::stdout(),
            "\x1b[?1049h\x1b[r\x1b[?6l\x1b[2J\x1b[H\x1b[?2004h\x1b[?25l"
        )?;
        io::stdout().flush()?;
        terminal.size();
        terminal.background = terminal.query_background();
        // A panic on this thread would print onto the alternate screen and then
        // vanish when unwinding restores the terminal. Restore first, so the
        // message lands on the normal screen. Other threads' panics leave the
        // running interface alone.
        let owner = std::thread::current().id();
        let saved = terminal.saved.clone();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if std::thread::current().id() == owner {
                restore(&saved);
            }
            previous(info);
        }));
        Ok(terminal)
    }
    /// Forget the previous frame so the next draw repaints every cell, for
    /// example after another program wrote to the terminal.
    pub fn invalidate(&mut self) {
        self.previous = None;
    }
    /// The terminal's background color, when it answered the startup query.
    pub fn background(&self) -> Option<(u8, u8, u8)> {
        self.background
    }
    /// Ask for the background color, then for device attributes, which every
    /// terminal answers in order: once that second reply arrives, a missing
    /// first reply means the terminal does not report its colors. Waits at
    /// most 200 ms; typed keys stay queued for the editor.
    fn query_background(&mut self) -> Option<(u8, u8, u8)> {
        let mut stdout = io::stdout().lock();
        stdout.write_all(b"\x1b]11;?\x1b\\\x1b[c").ok()?;
        stdout.flush().ok()?;
        drop(stdout);
        let deadline = Instant::now() + Duration::from_millis(200);
        loop {
            let (background, attributes) = self.decoder.reports();
            if background.is_some() || attributes {
                return background;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            match self.rx.recv_timeout(left) {
                Ok(bytes) => self.decoder.feed(&bytes),
                Err(_) => return None,
            }
        }
    }
    /// Enable wheel, button, and button-held motion reports. No all-motion
    /// tracking is enabled. Turning this off restores native terminal selection.
    pub fn set_mouse(&mut self, enabled: bool) -> io::Result<()> {
        if self.mouse == enabled {
            return Ok(());
        }
        let mut stdout = io::stdout().lock();
        stdout.write_all(if enabled {
            b"\x1b[?1000l\x1b[?1002h\x1b[?1006h"
        } else {
            b"\x1b[?1000l\x1b[?1002l\x1b[?1006l"
        })?;
        stdout.flush()?;
        self.mouse = enabled;
        Ok(())
    }
    pub fn size(&mut self) -> (usize, usize) {
        if self.sized.elapsed() > Duration::from_millis(200) {
            if let Some((w, h)) = terminal_size(&self.tty) {
                self.size = (w.min(500), h.min(200));
            } else if let Ok(tty) = self.tty.try_clone()
                && let Ok(out) = Command::new("stty")
                    .arg("size")
                    .stdin(Stdio::from(tty))
                    .output()
            {
                let size: Vec<usize> = String::from_utf8_lossy(&out.stdout)
                    .split_whitespace()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                if size.len() == 2 && size[0] > 0 && size[1] > 0 {
                    self.size = (size[1].min(500), size[0].min(200));
                }
            }
            self.sized = Instant::now();
        }
        self.size
    }
    pub fn poll(&mut self, timeout: Duration) -> io::Result<Option<Key>> {
        if let Some(key) = self.decoder.next(false) {
            return Ok(Some(key));
        }
        match self.rx.recv_timeout(timeout) {
            Ok(bytes) => {
                self.decoder.feed(&bytes);
                Ok(self.decoder.next(false))
            }
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(self.decoder.next(!timeout.is_zero())),
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(Some(Key::Eof)),
        }
    }
    pub fn draw(&mut self, frame: Frame) -> io::Result<()> {
        let same_size = self
            .previous
            .as_ref()
            .is_some_and(|p| p.width == frame.width && p.height == frame.height);
        let mut out = String::new();
        if !same_size {
            out.push_str("\x1b[r\x1b[?6l\x1b[2J\x1b[H");
        }
        for y in 0..frame.height {
            let row = &frame.cells[y * frame.width..(y + 1) * frame.width];
            if same_size
                && self.previous.as_ref().unwrap().cells[y * frame.width..(y + 1) * frame.width]
                    == *row
            {
                continue;
            }
            out += &format!("\x1b[{};1H", y + 1);
            let mut style = None;
            for cell in row {
                if cell.continuation {
                    continue;
                }
                if style != Some(cell.style) {
                    out += &cell.style.ansi();
                    style = Some(cell.style);
                }
                out += &cell.text;
            }
        }
        let cursor_changed = self
            .previous
            .as_ref()
            .is_none_or(|p| p.cursor != frame.cursor);
        if !out.is_empty() || cursor_changed {
            if let Some((x, y)) = frame.cursor {
                out += &format!("\x1b[{};{}H\x1b[?25h", y + 1, x + 1);
            } else {
                out += "\x1b[?25l";
            }
            let mut stdout = io::stdout().lock();
            stdout.write_all(b"\x1b[?7l")?;
            stdout.write_all(out.as_bytes())?;
            stdout.write_all(b"\x1b[?7h")?;
            stdout.flush()?;
        }
        self.previous = Some(frame);
        Ok(())
    }
}
/// Leave the alternate screen, disable mouse and paste reporting, show the
/// cursor, and restore the saved terminal settings. Safe to repeat.
fn restore(saved: &str) {
    let _ = write!(
        io::stdout(),
        "\x1b[0m\x1b[?1000l\x1b[?1002l\x1b[?1006l\x1b[?7h\x1b[r\x1b[?2004l\x1b[?25h\x1b[?1049l"
    );
    let _ = io::stdout().flush();
    if let Ok(tty) = File::open("/dev/tty") {
        let _ = Command::new("stty")
            .arg(saved)
            .stdin(Stdio::from(tty))
            .status();
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        restore(&self.saved);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paste_does_not_become_enter_events() {
        let mut d = Decoder::default();
        d.feed(b"\x1b[200~hello\nthere\x1b[201~\r");
        assert_eq!(d.next(false), Some(Key::Paste("hello\nthere".into())));
        assert_eq!(d.next(false), Some(Key::Enter));
    }
    #[test]
    fn split_utf8_and_escape() {
        let mut d = Decoder::default();
        d.feed(&[0xc3]);
        assert!(d.next(false).is_none());
        d.feed(&[0xa9]);
        assert_eq!(d.next(false), Some(Key::Char('é')));
        d.feed(b"\x1b");
        assert!(d.next(false).is_none());
        d.feed(b"[A");
        assert_eq!(d.next(false), Some(Key::Up));
    }
    #[test]
    fn removes_ansi_and_clipboard_sequences() {
        assert_eq!(sanitize("hi\x1b[31m red\x1b]52;c;secret\x07!"), "hi red!");
    }
    #[test]
    fn unicode_editor() {
        let mut e = Editor::default();
        e.insert("Hi界");
        e.edit(&Key::Backspace);
        assert_eq!(e.value(), "Hi");
        e.insert("é");
        let (_, cursor) = e.visual(10);
        assert_eq!(cursor, (3, 0));
    }
    #[test]
    fn canvas_clips_without_escape_injection() {
        let mut f = Frame::new(5, 2, Style::default());
        f.text(0, 0, "界abcDEF\x1b[2J", Style::default(), 5);
        assert_eq!(f.plain_text(), "界abc\n");
    }
    #[test]
    fn oversized_paste_is_rejected() {
        let mut d = Decoder::default();
        d.feed(b"\x1b[200~");
        d.feed(&vec![b'x'; 1_048_600]);
        assert!(d.next(false).is_none());
        d.feed(b"\x1b[201~");
        assert_eq!(d.next(false), Some(Key::PasteRejected));
    }
    #[test]
    fn malformed_utf8_preserves_following_keys_and_ss3_arrows() {
        let mut d = Decoder::default();
        d.feed(&[0xe2, b'a', 3]);
        assert_eq!(d.next(false), Some(Key::Char('\u{fffd}')));
        assert_eq!(d.next(false), Some(Key::Char('a')));
        assert_eq!(d.next(false), Some(Key::Ctrl('c')));
        d.feed(b"\x1bO");
        assert!(d.next(false).is_none());
        d.feed(b"A");
        assert_eq!(d.next(false), Some(Key::Up));
    }
    #[test]
    fn prose_wrap_preserves_whole_words_and_code_wrap_preserves_spaces() {
        assert_eq!(
            wrap_words("Hello Crouton Alpha", 14),
            vec!["Hello Crouton", "Alpha"]
        );
        assert_eq!(wrap("    fn main()", 80), vec!["    fn main()"]);
    }
    #[test]
    fn safe_copy_preserves_tabs_without_terminal_commands() {
        assert_eq!(
            sanitize_copy("\tfn main() {}\n\x1b]52;c;secret\x07"),
            "\tfn main() {}\n"
        );
    }
    #[test]
    fn sgr_wheel_accepts_all_chunk_boundaries_and_modifiers() {
        let report = b"\x1b[<80;300;12M";
        for split in 1..report.len() {
            let mut d = Decoder::default();
            d.feed(&report[..split]);
            assert_eq!(d.next(false), None, "split {split}");
            d.feed(&report[split..]);
            assert_eq!(
                d.next(false),
                Some(Key::Scroll {
                    up: true,
                    x: 299,
                    y: 11
                })
            );
        }
        let mut d = Decoder::default();
        d.feed(b"\x1b[<93;1;1M");
        assert_eq!(
            d.next(false),
            Some(Key::Scroll {
                up: false,
                x: 0,
                y: 0
            })
        );
    }
    #[test]
    fn sgr_left_button_events_accept_every_fragment_boundary_and_modifiers() {
        for (report, kind) in [
            (b"\x1b[<0;300;12M".as_slice(), MouseKind::Down),
            (b"\x1b[<32;300;12M".as_slice(), MouseKind::Drag),
            (b"\x1b[<0;300;12m".as_slice(), MouseKind::Up),
            (b"\x1b[<28;300;12M".as_slice(), MouseKind::Down),
            (b"\x1b[<60;300;12M".as_slice(), MouseKind::Drag),
            (b"\x1b[<28;300;12m".as_slice(), MouseKind::Up),
        ] {
            for split in 1..report.len() {
                let mut decoder = Decoder::default();
                decoder.feed(&report[..split]);
                assert_eq!(decoder.next(false), None, "split {split}");
                decoder.feed(&report[split..]);
                decoder.feed(b"x");
                assert_eq!(
                    decoder.next(false),
                    Some(Key::Mouse {
                        kind,
                        x: 299,
                        y: 11
                    })
                );
                assert_eq!(decoder.next(false), Some(Key::Char('x')));
                assert_eq!(decoder.next(false), None);
            }
        }
        for report in [
            b"\x1b[<0;0;1M".as_slice(),
            b"\x1b[<32;1;65536M",
            b"\x1b[<0;1;0m",
        ] {
            let mut decoder = Decoder::default();
            decoder.feed(report);
            assert_eq!(decoder.next(false), Some(Key::Unknown));
        }
    }
    #[test]
    fn legacy_left_release_requires_a_prior_left_press() {
        let down = [27, b'[', b'M', 32, 40, 45];
        let drag = [27, b'[', b'M', 64, 41, 46];
        let up = [27, b'[', b'M', 35, 42, 47];
        for split in 1..up.len() {
            let mut decoder = Decoder::default();
            decoder.feed(&down);
            assert_eq!(
                decoder.next(false),
                Some(Key::Mouse {
                    kind: MouseKind::Down,
                    x: 7,
                    y: 12
                })
            );
            decoder.feed(&drag);
            assert_eq!(
                decoder.next(false),
                Some(Key::Mouse {
                    kind: MouseKind::Drag,
                    x: 8,
                    y: 13
                })
            );
            decoder.feed(&up[..split]);
            assert_eq!(decoder.next(false), None);
            decoder.feed(&up[split..]);
            assert_eq!(
                decoder.next(false),
                Some(Key::Mouse {
                    kind: MouseKind::Up,
                    x: 9,
                    y: 14
                })
            );
            decoder.feed(&up);
            assert_eq!(decoder.next(false), Some(Key::Unknown));
        }
    }
    #[test]
    fn frame_ranges_keep_wide_glyphs_combining_marks_and_bounds() {
        let base = Style::default();
        let selected = Style::new(Some((255, 255, 255)), Some((25, 60, 90)), true);
        let mut frame = Frame::new(10, 2, base);
        frame.text(1, 0, "A界e\u{301}🙂Z", base, 9);
        assert_eq!(frame.text_range(0, 2, 3), "界");
        assert_eq!(frame.text_range(0, 3, 4), "界");
        assert_eq!(frame.text_range(0, 4, 5), "e\u{301}");
        assert_eq!(frame.text_range(0, 5, 6), "🙂");
        assert_eq!(frame.text_range(0, 6, 7), "🙂");
        assert_eq!(frame.text_range(0, 1, 8), "A界e\u{301}🙂Z");
        assert_eq!(frame.text_range(0, 8, usize::MAX), "  ");
        assert!(frame.text_range(2, 0, 10).is_empty());
        assert!(frame.text_range(0, 3, 3).is_empty());
        assert!(frame.text_range(0, 7, 2).is_empty());
        assert!(frame.text_range(0, usize::MAX, usize::MAX).is_empty());
        frame.highlight_range(0, 3, 4, selected);
        assert_eq!(frame.cells[2].style, selected);
        assert_eq!(frame.cells[3].style, selected);
        assert_eq!(frame.cells[1].style, base);
        assert_eq!(frame.cells[4].style, base);
        frame.highlight_range(1, usize::MAX, usize::MAX, selected);
        frame.highlight_range(usize::MAX, 0, usize::MAX, selected);
        assert_eq!(frame.text_range(0, 1, 8), "A界e\u{301}🙂Z");
    }
    #[test]
    fn ignored_sgr_events_never_leak_coordinates_into_editor() {
        for report in [
            "\x1b[<1;9;2M",
            "\x1b[<2;9;2m",
            "\x1b[<33;9;2M",
            "\x1b[<35;9;2M",
            "\x1b[<64;9;2m",
            "\x1b[<66;9;2M",
            "\x1b[<67;9;2M",
            "\x1b[<96;9;2M",
            "\x1b[<64;0;2M",
            "\x1b[<64;2;0M",
            "\x1b[<64;65536;2M",
            "\x1b[<64;abc;2M",
            "\x1b[<64;1;2;3M",
        ] {
            let mut d = Decoder::default();
            d.feed(report.as_bytes());
            d.feed(b"x");
            assert_eq!(d.next(false), Some(Key::Unknown), "{report:?}");
            assert_eq!(d.next(false), Some(Key::Char('x')), "{report:?}");
            assert_eq!(d.next(false), None);
        }
    }
    #[test]
    fn oversized_mouse_report_is_bounded_and_recovers() {
        let mut d = Decoder::default();
        d.feed(b"\x1b[<64;");
        d.feed(&[b'9'; 1024]);
        assert_eq!(d.next(false), None);
        assert_eq!(d.bytes.len(), 3);
        d.feed(b";3Mx");
        assert_eq!(d.next(false), Some(Key::Unknown));
        assert_eq!(d.next(false), Some(Key::Char('x')));
        d.feed(b"\x1b[<64;1;\x03");
        assert_eq!(d.next(false), Some(Key::Unknown));
        assert_eq!(d.next(false), Some(Key::Ctrl('c')));
    }
    #[test]
    fn legacy_mouse_fallback_consumes_clicks_and_partial_reports() {
        for (button, expected) in [
            (
                64,
                Key::Scroll {
                    up: true,
                    x: 4,
                    y: 5,
                },
            ),
            (
                65,
                Key::Scroll {
                    up: false,
                    x: 4,
                    y: 5,
                },
            ),
            (
                0,
                Key::Mouse {
                    kind: MouseKind::Down,
                    x: 4,
                    y: 5,
                },
            ),
            (
                32,
                Key::Mouse {
                    kind: MouseKind::Drag,
                    x: 4,
                    y: 5,
                },
            ),
            (1, Key::Unknown),
            (2, Key::Unknown),
            (3, Key::Unknown),
            (66, Key::Unknown),
        ] {
            let bytes = [27, b'[', b'M', button + 32, 5 + 32, 6 + 32];
            for split in 1..bytes.len() {
                let mut d = Decoder::default();
                d.feed(&bytes[..split]);
                assert_eq!(d.next(false), None);
                d.feed(&bytes[split..]);
                d.feed(b"z");
                assert_eq!(d.next(false), Some(expected.clone()));
                assert_eq!(d.next(false), Some(Key::Char('z')));
            }
        }
    }
    #[test]
    fn mouse_reports_inside_paste_are_never_events() {
        let mut d = Decoder::default();
        d.feed(b"\x1b[200~text\x1b[<64;2;3M\x1b[201~");
        assert_eq!(d.next(false), Some(Key::Paste("text".into())));
        assert_eq!(d.next(false), None);
    }
    #[test]
    fn truncated_legacy_mouse_report_preserves_interrupting_controls() {
        for prefix in [
            b"\x1b[M".as_slice(),
            b"\x1b[M ".as_slice(),
            b"\x1b[M *".as_slice(),
        ] {
            let mut d = Decoder::default();
            d.feed(prefix);
            d.feed(b"\x03");
            assert_eq!(d.next(false), Some(Key::Unknown));
            assert_eq!(d.next(false), Some(Key::Ctrl('c')));
            assert_eq!(d.next(false), None);
        }
        let mut d = Decoder::default();
        d.feed(b"\x1b[M\x1b[A");
        assert_eq!(d.next(false), Some(Key::Unknown));
        assert_eq!(d.next(false), Some(Key::Up));
    }
    #[test]
    fn incomplete_csi_preserves_interrupting_controls_and_new_escapes() {
        for prefix in [b"\x1b[".as_slice(), b"\x1b[1;", b"\x1b[?123;", b"\x1b"] {
            for (suffix, expected) in [
                (b"\x03".as_slice(), Key::Ctrl('c')),
                (b"\x04", Key::Ctrl('d')),
                (b"\x1b[A", Key::Up),
            ] {
                let mut d = Decoder::default();
                d.feed(prefix);
                assert!(d.next(false).is_none());
                d.feed(suffix);
                assert_eq!(d.next(false), Some(Key::Unknown));
                assert_eq!(d.next(false), Some(expected));
                assert!(d.next(true).is_none());
            }
        }
    }
    #[test]
    fn fragmented_paste_finds_marker_across_every_boundary_and_resets_scan() {
        for split in 0..=6 {
            let mut d = Decoder::default();
            d.feed(b"\x1b[200~");
            for _ in 0..80 {
                d.feed(b"some pasted words\n");
                assert!(d.next(false).is_none());
                assert_eq!(d.paste_scan, d.bytes.len() - 5);
            }
            d.feed(&b"\x1b[201~"[..split]);
            if split < 6 {
                assert!(d.next(false).is_none());
            }
            d.feed(&b"\x1b[201~"[split..]);
            assert_eq!(
                d.next(false),
                Some(Key::Paste("some pasted words\n".repeat(80)))
            );
            assert_eq!(d.paste_scan, 0);
            d.feed(b"\x1b[200~second\x1b[201~\r");
            assert_eq!(d.next(false), Some(Key::Paste("second".into())));
            assert_eq!(d.next(false), Some(Key::Enter));
        }
    }
    #[test]
    fn oversized_fragmented_paste_stays_bounded_and_keeps_following_keys() {
        let mut d = Decoder::default();
        d.feed(b"\x1b[200~");
        let chunk = vec![b'a'; 1024];
        for _ in 0..2050 {
            d.feed(&chunk);
            assert!(d.next(false).is_none());
            assert!(d.bytes.len() <= 1_048_588);
        }
        d.feed(b"\x1b[20");
        assert!(d.next(false).is_none());
        d.feed(b"1~\x03");
        assert_eq!(d.next(false), Some(Key::PasteRejected));
        assert_eq!(d.next(false), Some(Key::Ctrl('c')));
    }
    #[test]
    fn f2_decodes_ss3_and_csi_across_read_boundaries() {
        for sequence in [b"\x1bOQ".as_slice(), b"\x1b[12~"] {
            for split in 1..sequence.len() {
                let mut decoder = Decoder::default();
                decoder.feed(&sequence[..split]);
                assert_eq!(decoder.next(false), None);
                decoder.feed(&sequence[split..]);
                assert_eq!(decoder.next(false), Some(Key::F2));
                assert_eq!(decoder.next(false), None);
            }
        }
    }
    #[test]
    fn pasted_lines_survive_carriage_return_separators() {
        for separator in ["\r", "\n", "\r\n"] {
            let mut d = Decoder::default();
            let paste = format!("\x1b[200~fn main() {{{separator}    run();{separator}}}\x1b[201~");
            d.feed(paste.as_bytes());
            assert_eq!(
                d.next(false),
                Some(Key::Paste("fn main() {\n    run();\n}".into()))
            );
        }
    }
    #[test]
    fn ctrl_j_and_modified_enter_insert_newlines() {
        let mut d = Decoder::default();
        d.feed(b"\n\r\n\r\x1b[13;2u\x1b[27;2;13~");
        assert_eq!(d.next(false), Some(Key::AltEnter));
        assert_eq!(d.next(false), Some(Key::Enter));
        assert_eq!(d.next(false), Some(Key::Enter));
        assert_eq!(d.next(false), Some(Key::AltEnter));
        assert_eq!(d.next(false), Some(Key::AltEnter));
        assert_eq!(d.next(false), None);
    }
    #[test]
    fn function_keys_and_alt_characters_never_type_letters() {
        let mut d = Decoder::default();
        d.feed("\x1bOP\x1bOR\x1bOS\x1b[[A\x1b\u{e4}x".as_bytes());
        for _ in 0..5 {
            assert_eq!(d.next(false), Some(Key::Unknown));
        }
        assert_eq!(d.next(false), Some(Key::Char('x')));
        let mut d = Decoder::default();
        d.feed(b"\x1b[1~\x1b[4~\x1b[7~\x1b[8~\x1b[[B");
        for key in [Key::Home, Key::End, Key::Home, Key::End, Key::F2] {
            assert_eq!(d.next(false), Some(key));
        }
    }
    #[test]
    fn ctrl_k_kills_to_line_end_and_joins_at_line_end() {
        let mut editor = Editor::default();
        editor.set("one\ntwo\nthree");
        editor.edit(&Key::Up);
        editor.edit(&Key::Up);
        editor.edit(&Key::Home);
        editor.edit(&Key::Right);
        editor.edit(&Key::Ctrl('k'));
        assert_eq!(editor.value(), "o\ntwo\nthree");
        editor.edit(&Key::Ctrl('k'));
        assert_eq!(editor.value(), "otwo\nthree");
    }
    #[test]
    fn oversized_insert_is_reported_and_leaves_the_draft() {
        let mut editor = Editor::default();
        assert!(editor.insert("keep"));
        assert!(!editor.insert(&"x".repeat(1_048_576)));
        assert_eq!(editor.value(), "keep");
    }
    #[test]
    fn background_replies_are_reported_and_never_typed() {
        let mut d = Decoder::default();
        d.feed(b"x\x1b]11;rgb:1e1e/2020/2a2a\x1b\\\x1b[?62;22cy");
        assert_eq!(d.reports(), (Some((30, 32, 42)), true));
        assert_eq!(d.next(false), Some(Key::Char('x')));
        assert_eq!(d.next(false), Some(Key::Unknown));
        assert_eq!(d.next(false), Some(Key::Unknown));
        assert_eq!(d.next(false), Some(Key::Char('y')));
        let mut bel = Decoder::default();
        bel.feed(b"\x1b]11;rgb:ff/ff/ff\x07");
        assert_eq!(bel.reports(), (Some((255, 255, 255)), false));
        let mut none = Decoder::default();
        none.feed(b"\x1b[?1;2c");
        assert_eq!(none.reports(), (None, true));
        assert_eq!(parse_rgb("rgb:zz/00/00"), None);
    }
    #[test]
    fn common_emoji_are_two_columns_and_tags_are_invisible() {
        for emoji in ['\u{2705}', '\u{274c}', '\u{2b50}', '\u{26a1}', '\u{1f600}'] {
            assert_eq!(char_width(emoji), 2, "{emoji}");
        }
        assert_eq!(char_width('\u{e0067}'), 0);
        assert_eq!(char_width('\u{feff}'), 0);
        assert_eq!(width("\u{2705} Tests pass"), 13);
    }
}
