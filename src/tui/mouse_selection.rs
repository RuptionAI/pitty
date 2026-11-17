//! Mouse selection uses a snapshot of the visible transcript. Background replies
//! can finish normally without moving the text beneath a drag or changing a copy.
use super::*;
use pitty_ui::MouseKind;

const COPY_LABEL: &str = "[Copy selection]";
type Point = (usize, usize);
type Area = (usize, usize, usize, usize);

pub(super) struct Surface {
    frame: Frame,
    area: Area,
    status: Area,
    text_bounds: Vec<Option<(usize, usize)>>,
    code_rows: Vec<Option<transcript_view::CodeRow>>,
}
pub(super) struct Selection {
    surface: Surface,
    anchor: Point,
    focus: Point,
    dragging: bool,
    moved: bool,
    feedback: Option<String>,
}
fn contains((left, top, width, height): Area, (x, y): Point) -> bool {
    x >= left && x < left.saturating_add(width) && y >= top && y < top.saturating_add(height)
}
impl Selection {
    fn endpoints(&self) -> (Point, Point) {
        if (self.anchor.1, self.anchor.0) <= (self.focus.1, self.focus.0) {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }
    fn range(&self, row: usize) -> Option<(usize, usize)> {
        let (first, last) = self.endpoints();
        let (left, top, width, _) = self.surface.area;
        let (text_start, text_end) = self
            .surface
            .text_bounds
            .get(row - top)
            .copied()
            .unwrap_or(Some((left, left + width)))?;
        let start = if row == first.1 { first.0 } else { left };
        let end = if row == last.1 {
            last.0 + 1
        } else {
            left + width
        };
        Some((
            start.clamp(text_start, text_end),
            end.clamp(text_start, text_end),
        ))
    }
    fn text(&self) -> String {
        if !self.moved {
            return String::new();
        }
        let (first, last) = self.endpoints();
        let (left, top, width, _) = self.surface.area;
        let mut text = String::new();
        let mut written = false;
        let mut previous: Option<(std::sync::Arc<str>, usize)> = None;
        for row in first.1..=last.1 {
            let Some((start, end)) = self.range(row) else {
                continue;
            };
            if let Some(Some(source)) = self.surface.code_rows.get(row - top) {
                let (text_start, _) = self.surface.text_bounds[row - top].unwrap();
                let Some(mut range) = source_range(source, start - text_start, end - text_start)
                    .or_else(|| source.source.is_empty().then_some(0..0))
                else {
                    continue;
                };
                let continuation = previous
                    .as_ref()
                    .is_some_and(|(prior, _)| std::sync::Arc::ptr_eq(prior, &source.source));
                if continuation {
                    range.start = range.start.max(previous.as_ref().unwrap().1).min(range.end);
                } else if written {
                    text.push('\n');
                }
                text.push_str(&source.source[range.clone()]);
                previous = Some((source.source.clone(), range.end));
            } else {
                let line = self.surface.frame.text_range(row, start, end);
                if written {
                    text.push('\n');
                }
                text.push_str(if end == left + width {
                    line.trim_end_matches(' ')
                } else {
                    &line
                });
                previous = None;
            }
            written = true;
        }
        text
    }
    fn move_to(&mut self, (x, y): Point) {
        let (left, top, width, height) = self.surface.area;
        self.focus = (
            x.clamp(left, left + width - 1),
            y.clamp(top, top + height - 1),
        );
        self.moved |= self.focus != self.anchor;
    }
    fn copy_button(&self) -> Area {
        let (x, y, width, _) = self.surface.status;
        (x, y, width.min(COPY_LABEL.len()), 1)
    }
}
/// Map rendered cells back to original code bytes. Tabs touched by a selection
/// remain literal tabs; repeated fragments of a wrapped tab are merged by text().
fn source_range(
    source: &transcript_view::CodeRow,
    start: usize,
    end: usize,
) -> Option<std::ops::Range<usize>> {
    if start >= end {
        return None;
    }
    let start = start + source.skip_cells;
    let end = end + source.skip_cells;
    let mut column = 0;
    let mut first = None;
    let mut last = source.bytes.start;
    let mut selected_glyph = false;
    for (offset, ch) in source.source[source.bytes.clone()].char_indices() {
        let byte = source.bytes.start + offset;
        let cells = if ch == '\t' {
            4
        } else {
            pitty_ui::char_width(ch)
        };
        if cells == 0 {
            if selected_glyph {
                last = byte + ch.len_utf8();
            }
        } else {
            selected_glyph = column < end && column + cells > start;
            if selected_glyph {
                first.get_or_insert(byte);
                last = byte + ch.len_utf8();
            }
        }
        column += cells;
    }
    first.map(|first| first..last)
}

impl App {
    pub(super) fn remember_mouse_surface(
        &mut self,
        frame: &Frame,
        area: Area,
        status: Area,
        text_bounds: Vec<Option<(usize, usize)>>,
        code_rows: Vec<Option<transcript_view::CodeRow>>,
    ) {
        self.mouse_surface = if !self.args.no_mouse
            && !self.selection_mode
            && !self.key_mode
            && !self.confirm_quit
            && self.panel.is_none()
            && self.picker.is_none()
            && !self.chats[self.active].messages.is_empty()
            && area.2 > 0
            && area.3 > 0
        {
            Some(Surface {
                frame: frame.clone(),
                area,
                status,
                text_bounds,
                code_rows,
            })
        } else {
            None
        };
    }
    pub(super) fn clear_mouse_selection(&mut self) -> bool {
        let existed = self.mouse_selection.take().is_some();
        if existed {
            self.mouse_surface = None;
        }
        existed
    }
    pub(super) fn selected_text(&self) -> Option<String> {
        self.mouse_selection.as_ref().map(Selection::text)
    }
    pub(super) fn copy_selected_or_last(&mut self) -> Result<(), String> {
        if let Some(text) = self.selected_text() {
            let result = if text.trim().is_empty() {
                Err("Selection contains no text. Drag across the text you want to copy.".into())
            } else {
                self.copy_text(&text)
            };
            if let Some(selection) = &mut self.mouse_selection {
                selection.feedback = Some(match &result {
                    Ok(()) => self.status.clone(),
                    Err(error) => error.clone(),
                });
            }
            result
        } else {
            self.copy("last")
        }
    }
    pub(super) fn mouse_event(&mut self, kind: MouseKind, x: usize, y: usize) {
        if self.args.no_mouse || self.selection_mode || self.key_mode || self.confirm_quit {
            return;
        }
        if kind == MouseKind::Down
            && self
                .mouse_selection
                .as_ref()
                .is_some_and(|s| s.moved && contains(s.copy_button(), (x, y)))
        {
            if let Err(error) = self.copy_selected_or_last() {
                self.status = error;
            }
            return;
        }
        match kind {
            MouseKind::Down => {
                let surface = self
                    .mouse_selection
                    .take()
                    .map(|s| s.surface)
                    .or_else(|| self.mouse_surface.take());
                if let Some(surface) = surface {
                    if self.picker.is_none()
                        && self.panel.is_none()
                        && contains(surface.area, (x, y))
                    {
                        self.mouse_selection = Some(Selection {
                            surface,
                            anchor: (x, y),
                            focus: (x, y),
                            dragging: true,
                            moved: false,
                            feedback: None,
                        });
                    } else {
                        self.mouse_surface = Some(surface);
                    }
                }
            }
            MouseKind::Drag | MouseKind::Up => {
                let mut clear = false;
                if let Some(selection) = &mut self.mouse_selection
                    && selection.dragging
                {
                    selection.move_to((x, y));
                    if kind == MouseKind::Up {
                        selection.dragging = false;
                        clear = !selection.moved || selection.text().trim().is_empty();
                    }
                }
                if clear {
                    self.mouse_surface = self.mouse_selection.take().map(|s| s.surface);
                }
            }
        }
    }
    pub(super) fn draw_mouse_selection(&mut self, w: usize, h: usize) -> Option<Frame> {
        let selection = self.mouse_selection.as_ref()?;
        if selection.surface.frame.width != w || selection.surface.frame.height != h {
            self.clear_mouse_selection();
            return None;
        }
        let colors = self.colors();
        let mut frame = selection.surface.frame.clone();
        if selection.moved {
            let (first, last) = selection.endpoints();
            for row in first.1..=last.1 {
                if let Some((start, end)) = selection.range(row) {
                    frame.highlight_range(row, start, end, colors.selected);
                }
            }
        }
        let (x, y, width, height) = selection.surface.status;
        frame.fill(x, y, width, height, colors.base);
        if selection.moved {
            frame.text(x, y, COPY_LABEL, colors.selected, width);
            if width > COPY_LABEL.len() + 2 {
                frame.text(
                    x + COPY_LABEL.len() + 2,
                    y,
                    "Ctrl+Y copy · Esc clear",
                    colors.accent,
                    width - COPY_LABEL.len() - 2,
                );
            }
        } else {
            frame.text(
                x,
                y,
                "Drag across text to select · Esc cancels",
                colors.muted,
                width,
            );
        }
        if height > 1 {
            frame.text(x, y + 1,
                selection.feedback.as_deref().unwrap_or(if width < 40 {
                    "Ctrl+Y · Esc"
                } else {
                    "Selection stays still · click Copy selection or press Ctrl+Y · F2 for native selection"
                }), colors.muted, width);
        }
        Some(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    fn app() -> App {
        let mut a = App::new(Args::try_parse_from(["pitty", "--no-history"]).unwrap());
        a.welcome_fade = Some(Instant::now() - Duration::from_secs(1));
        a.chats[a.active]
            .messages
            .push(Message::user("Hello 界e\u{301} world".into()));
        a
    }
    fn position(frame: &Frame, text: &str) -> Point {
        frame
            .plain_text()
            .lines()
            .enumerate()
            .find_map(|(y, line)| line.find(text).map(|byte| (width(&line[..byte]), y)))
            .unwrap()
    }
    fn mouse(a: &mut App, kind: MouseKind, point: Point) {
        a.handle(Key::Mouse {
            kind,
            x: point.0,
            y: point.1,
        });
    }
    #[test]
    fn drag_both_directions_preserves_wide_and_combining_glyphs() {
        for reverse in [false, true] {
            let mut a = app();
            let frame = a.draw(80, 24);
            let start = position(&frame, "界");
            let end = (start.0 + 2, start.1);
            let (anchor, focus) = if reverse { (end, start) } else { (start, end) };
            mouse(&mut a, MouseKind::Down, anchor);
            mouse(&mut a, MouseKind::Drag, focus);
            mouse(&mut a, MouseKind::Up, focus);
            assert_eq!(a.selected_text().as_deref(), Some("界e\u{301}"));
            assert!(a.draw(80, 24).plain_text().contains(COPY_LABEL));
        }
    }
    #[test]
    fn multiline_code_selection_omits_borders_and_retains_indentation() {
        let mut a = app();
        a.chats[a.active].messages[0].content =
            "```rust\nfn main() {\n    println!(\"Hi\");\n}\n```".into();
        let frame = a.draw(100, 32);
        let start = position(&frame, "fn main()");
        let end = position(&frame, "}");
        mouse(&mut a, MouseKind::Down, start);
        mouse(&mut a, MouseKind::Drag, end);
        mouse(&mut a, MouseKind::Up, end);
        assert_eq!(
            a.selected_text().as_deref(),
            Some("fn main() {\n    println!(\"Hi\");\n}")
        );
        mouse(&mut a, MouseKind::Down, (0, 0));
        assert!(a.mouse_selection.is_none());
    }
    #[test]
    fn partial_wide_code_cells_include_their_combining_marks() {
        let source: std::sync::Arc<str> = std::sync::Arc::from("界\u{301}x");
        let row = transcript_view::CodeRow {
            bytes: 0..source.len(),
            source,
            skip_cells: 0,
        };
        let range = source_range(&row, 0, 1).unwrap();
        assert_eq!(&row.source[range], "界\u{301}");
    }
    #[test]
    fn wrapped_code_keeps_real_lines_tabs_and_reverse_drag() {
        let code = "all:\n\t@echo \"a long string that must not gain any synthetic newline while copied\"\n// abcdefghijklmnop\tXYZ";
        for reverse in [false, true] {
            let mut a = app();
            a.chats[a.active].messages[0].content = format!("```makefile\n{code}\n```");
            let frame = a.draw(32, 40);
            let start = position(&frame, "╭");
            let end = position(&frame, "╰");
            let (anchor, focus) = if reverse { (end, start) } else { (start, end) };
            mouse(&mut a, MouseKind::Down, anchor);
            mouse(&mut a, MouseKind::Drag, focus);
            mouse(&mut a, MouseKind::Up, focus);
            assert_eq!(a.selected_text().as_deref(), Some(code));
        }
    }
    #[test]
    fn empty_selection_cannot_copy_reply_and_fast_clicks_keep_the_surface() {
        let mut a = app();
        let frame = a.draw(80, 24);
        let start = position(&frame, "Hello");
        mouse(&mut a, MouseKind::Down, (70, start.1));
        assert!(a.copy_selected_or_last().unwrap_err().contains("no text"));
        mouse(&mut a, MouseKind::Up, (70, start.1));
        // No intervening draw: the next drag in the same input batch still works.
        mouse(&mut a, MouseKind::Down, start);
        mouse(&mut a, MouseKind::Drag, (start.0 + 4, start.1));
        mouse(&mut a, MouseKind::Up, (start.0 + 4, start.1));
        assert_eq!(a.selected_text().as_deref(), Some("Hello"));
        a.interrupt_armed = true;
        a.handle(Key::Escape);
        assert!(!a.interrupt_armed);
    }
    #[test]
    fn viewport_bounds_exclude_header_composer_sidebar_and_footer() {
        let mut a = app();
        a.sidebar = true;
        let frame = a.draw(120, 32);
        let start = position(&frame, "Hello");
        let area = a.mouse_surface.as_ref().unwrap().area;
        for point in [(0, 0), (2, start.1), (3, 30)] {
            mouse(&mut a, MouseKind::Down, point);
            assert!(a.mouse_selection.is_none());
            a.draw(120, 32);
        }
        mouse(&mut a, MouseKind::Down, start);
        mouse(&mut a, MouseKind::Drag, (usize::MAX, usize::MAX));
        mouse(&mut a, MouseKind::Up, (usize::MAX, usize::MAX));
        let selection = a.mouse_selection.as_ref().unwrap();
        assert_eq!(selection.focus, (area.0 + area.2 - 1, area.1 + area.3 - 1));
        let text = a.selected_text().unwrap();
        assert!(!text.contains("Message, or"));
        assert!(!text.contains("Ctrl+C"));
    }
    #[test]
    fn selection_freezes_visible_text_and_escape_preserves_draft() {
        let mut a = app();
        a.input.set("draft to preserve");
        let frame = a.draw(90, 28);
        let start = position(&frame, "Hello");
        mouse(&mut a, MouseKind::Down, start);
        mouse(&mut a, MouseKind::Drag, (start.0 + 4, start.1));
        mouse(&mut a, MouseKind::Up, (start.0 + 4, start.1));
        a.chats[a.active].messages[0].content = "New reply has arrived".into();
        a.transcript_cache = None;
        assert!(a.draw(90, 28).plain_text().contains("Hello"));
        assert_eq!(a.selected_text().as_deref(), Some("Hello"));
        a.handle(Key::Escape);
        assert!(a.mouse_selection.is_none());
        assert_eq!(a.input.value(), "draft to preserve");
        assert!(
            a.draw(90, 28)
                .plain_text()
                .contains("New reply has arrived")
        );
    }
    #[test]
    fn editing_wheel_resize_and_native_selection_clear_the_snapshot() {
        for action in 0..4 {
            let mut a = app();
            let frame = a.draw(80, 24);
            let start = position(&frame, "Hello");
            mouse(&mut a, MouseKind::Down, start);
            mouse(&mut a, MouseKind::Drag, (start.0 + 4, start.1));
            mouse(&mut a, MouseKind::Up, (start.0 + 4, start.1));
            match action {
                0 => a.handle(Key::Char('x')),
                1 => a.handle(Key::Scroll {
                    up: true,
                    x: start.0,
                    y: start.1,
                }),
                2 => {
                    a.draw(81, 24);
                }
                _ => a.handle(Key::F2),
            }
            assert!(a.mouse_selection.is_none());
        }
    }
}
