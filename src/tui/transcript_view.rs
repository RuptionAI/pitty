use super::*;
use std::{ops::Range, sync::Arc};
pub(super) type Lines = Vec<Vec<(String, Style)>>;
#[derive(PartialEq, Eq)]
pub(super) struct CacheKey {
    pub chat: String,
    pub updated: u128,
    pub count: usize,
    pub width: usize,
    pub theme: Theme,
    pub compact: bool,
    pub name: String,
}
#[derive(Clone)]
pub(super) struct CodeRow {
    /// One shared source allocation per logical line, before display tab expansion.
    pub source: Arc<str>,
    /// UTF-8 source bytes touched by this display row. Split tabs overlap rows.
    pub bytes: Range<usize>,
    /// Leading tab cells already displayed in earlier wrapped rows.
    pub skip_cells: usize,
}
pub(super) struct Cache {
    pub key: CacheKey,
    pub lines: Lines,
    pub code_rows: Vec<Option<CodeRow>>,
}

/// Wrap one sanitized logical code line without allocating per-character maps.
/// Tabs render as four cells, matching the terminal renderer, while metadata
/// retains the original tab and the consumed prefix when it crosses a wrap.
fn wrap_code(source: &str, columns: usize) -> Vec<(String, CodeRow)> {
    fn push_row(
        rows: &mut Vec<(String, CodeRow)>,
        display: &mut String,
        source: &Arc<str>,
        bytes: Range<usize>,
        skip_cells: usize,
    ) {
        rows.push((
            std::mem::take(display),
            CodeRow {
                source: source.clone(),
                bytes,
                skip_cells,
            },
        ));
    }
    let source: Arc<str> = Arc::from(source);
    let columns = columns.max(1);
    let mut rows = Vec::new();
    let mut display = String::new();
    let mut cells = 0;
    let mut start = 0;
    let mut end = 0;
    let mut skip_cells = 0;
    for (byte, character) in source.char_indices() {
        let next = byte + character.len_utf8();
        let pieces = if character == '\t' { 4 } else { 1 };
        for piece in 0..pieces {
            let rendered = if character == '\t' { ' ' } else { character };
            let width = pitty_ui::char_width(rendered);
            if cells + width > columns && !display.is_empty() {
                push_row(&mut rows, &mut display, &source, start..end, skip_cells);
                cells = 0;
            }
            if display.is_empty() {
                start = byte;
                skip_cells = if character == '\t' { piece } else { 0 };
            }
            display.push(rendered);
            cells += width;
            end = next;
        }
    }
    push_row(&mut rows, &mut display, &source, start..end, skip_cells);
    rows
}
pub(super) fn render(
    chat: &Conversation,
    content_w: usize,
    colors: &Colors,
    theme: Theme,
    compact: bool,
    user_name: &str,
) -> (Lines, Vec<Option<CodeRow>>) {
    let mut lines: Vec<Vec<(String, Style)>> = vec![];
    let mut code_rows = Vec::new();
    let prose_width = content_w.saturating_sub(3).clamp(1, 100);
    let mut code_number = 0;
    for (index, message) in chat.messages.iter().enumerate() {
        lines.push(vec![(
            format!(
                "{} {}  · /copy message {}",
                if message.role == "user" { "›" } else { "●" },
                if message.role == "user" {
                    user_name
                } else if super::tools_view::is_command_result(&chat.messages, index) {
                    "Command"
                } else if index + 1 == chat.messages.len() {
                    chat.agent_name.as_deref().unwrap_or("Assistant")
                } else {
                    "Assistant"
                },
                index + 1
            ),
            colors.accent,
        )]);
        for file in &message.attachments {
            lines.push(vec![(
                format!("  Attached: {} ({})", file.name, file.kind),
                colors.muted,
            )]);
        }
        for block in crate::transcript::blocks(&pitty_ui::sanitize_copy(&message.content)) {
            match block {
                crate::transcript::Block::Prose(text) => {
                    for spans in crate::transcript::prose_lines(&clean(&text), prose_width) {
                        let mut row = vec![("  ".into(), colors.base)];
                        for (text, kind) in spans {
                            row.push((
                                text,
                                match kind {
                                    1 => colors.accent,
                                    2 => colors.selected,
                                    _ => colors.base,
                                },
                            ));
                        }
                        lines.push(row);
                    }
                }
                crate::transcript::Block::Code { language, text } => {
                    code_number += 1;
                    let block_width = content_w.saturating_sub(2).clamp(4, 100);
                    let label = format!(
                        " {} · /copy code {code_number} ",
                        if language.is_empty() {
                            "code"
                        } else {
                            &language
                        }
                    );
                    let mut label_width = 0;
                    let label: String = label
                        .chars()
                        .take_while(|c| {
                            label_width += pitty_ui::char_width(*c);
                            label_width <= block_width.saturating_sub(2)
                        })
                        .collect();
                    let heading = format!(
                        "  ╭{label}{}╮",
                        "─".repeat(block_width.saturating_sub(width(&label) + 2))
                    );
                    lines.push(vec![(
                        heading,
                        Style::new(colors.input.fg, colors.input.bg, true),
                    )]);
                    for source in text.lines() {
                        for (line, metadata) in
                            wrap_code(source, block_width.saturating_sub(4).max(1))
                        {
                            let mut row = vec![("  │ ".into(), colors.input)];
                            for (text, kind) in if matches!(
                                language.to_lowercase().as_str(),
                                "rust"
                                    | "rs"
                                    | "python"
                                    | "py"
                                    | "javascript"
                                    | "js"
                                    | "typescript"
                                    | "ts"
                                    | "c"
                                    | "cpp"
                                    | "java"
                                    | "go"
                                    | "sh"
                                    | "bash"
                                    | "json"
                                    | "toml"
                            ) {
                                crate::transcript::code_spans(&line)
                            } else {
                                vec![(line.clone(), 0)]
                            } {
                                let fg = match (theme == Theme::Dark, kind) {
                                    (true, 1) => (182, 159, 255),
                                    (true, 2) => (144, 215, 155),
                                    (true, 3) => (167, 183, 202),
                                    (_, 1) => (115, 48, 153),
                                    (_, 2) => (31, 108, 67),
                                    (_, 3) => (83, 99, 119),
                                    _ => colors.input.fg.unwrap_or((29, 37, 48)),
                                };
                                row.push((text, Style::new(Some(fg), colors.input.bg, kind == 1)));
                            }
                            row.push((
                                format!(
                                    "{} │",
                                    " ".repeat(block_width.saturating_sub(width(&line) + 4))
                                ),
                                colors.input,
                            ));
                            code_rows.resize(lines.len(), None);
                            code_rows.push(Some(metadata));
                            lines.push(row);
                        }
                    }
                    lines.push(vec![(
                        format!("  ╰{}╯", "─".repeat(block_width.saturating_sub(2))),
                        colors.input,
                    )]);
                }
            }
        }
        if !compact {
            lines.push(vec![]);
        }
    }

    if lines.is_empty() {
        lines.push(vec![(
            "Start a conversation, or /model to choose a model.".into(),
            colors.muted,
        )]);
    }
    code_rows.resize(lines.len(), None);
    (lines, code_rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_wrapping_matches_rendered_tabs_unicode_and_combining_marks() {
        for source in [
            "",
            "abc",
            "\tvalue",
            "A\tB",
            "\t\t",
            "界e\u{301}🙂",
            "a\t界e\u{301}\tend",
        ] {
            for columns in 1..=12 {
                let rows = wrap_code(source, columns);
                assert_eq!(
                    rows.iter()
                        .map(|(text, _)| text.clone())
                        .collect::<Vec<_>>(),
                    wrap(source, columns),
                    "source={source:?}, columns={columns}"
                );
                for (_, row) in &rows {
                    assert_eq!(&*row.source, source);
                    assert!(row.source.is_char_boundary(row.bytes.start));
                    assert!(row.source.is_char_boundary(row.bytes.end));
                    assert!(row.bytes.start <= row.bytes.end && row.bytes.end <= source.len());
                    assert!(Arc::ptr_eq(&rows[0].1.source, &row.source));
                }
            }
        }
    }
    #[test]
    fn split_tabs_record_source_ranges_and_consumed_cells() {
        let rows = wrap_code("A\tB", 3);
        assert_eq!(
            rows.iter()
                .map(|(text, _)| text.as_str())
                .collect::<Vec<_>>(),
            ["A  ", "  B"]
        );
        assert_eq!(rows[0].1.bytes, 0..2);
        assert_eq!(rows[0].1.skip_cells, 0);
        assert_eq!(rows[1].1.bytes, 1..3);
        assert_eq!(rows[1].1.skip_cells, 2);
        let rows = wrap_code("\t", 1);
        for (index, (_, row)) in rows.iter().enumerate() {
            assert_eq!(row.bytes, 0..1);
            assert_eq!(row.skip_cells, index);
        }
        let first = wrap_code("same", 2);
        let second = wrap_code("same", 2);
        assert!(!Arc::ptr_eq(&first[0].1.source, &second[0].1.source));
    }
    #[test]
    fn render_metadata_aligns_only_with_code_body_rows() {
        let colors = Theme::Light.colors();
        let mut chat = Conversation::new("example/model");
        chat.messages.push(Message::user("Prose before\n\n```rust\n\tlet wide = \"界\";\n\n\tlet wide = \"界\";\n```\n\nProse after".into()));
        let (lines, metadata) = render(&chat, 20, &colors, Theme::Light, false, "You");
        assert_eq!(lines.len(), metadata.len());
        let mut distinct_sources: Vec<Arc<str>> = Vec::new();
        for (line, row) in lines.iter().zip(&metadata) {
            let body = line.first().is_some_and(|(text, _)| text == "  │ ");
            assert_eq!(row.is_some(), body);
            if let Some(row) = row
                && distinct_sources
                    .last()
                    .is_none_or(|source| !Arc::ptr_eq(source, &row.source))
            {
                distinct_sources.push(row.source.clone());
            }
        }
        assert_eq!(distinct_sources.len(), 3);
        assert_eq!(&*distinct_sources[0], "\tlet wide = \"界\";");
        assert_eq!(&*distinct_sources[1], "");
        assert_eq!(&*distinct_sources[2], "\tlet wide = \"界\";");
        assert!(!Arc::ptr_eq(&distinct_sources[0], &distinct_sources[2]));
    }
}
