//! Message elements keep original text for copying; rendering is a separate step.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Prose(String),
    Code { language: String, text: String },
}

pub fn blocks(text: &str) -> Vec<Block> {
    let mut result = Vec::new();
    let mut prose = String::new();
    let mut code: Option<(char, usize, String, String)> = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if let Some((mark, count, language, body)) = &mut code {
            let closing = trimmed.chars().take_while(|c| c == mark).count();
            if closing >= *count && trimmed.chars().all(|c| c == *mark) {
                result.push(Block::Code {
                    language: language.clone(),
                    text: std::mem::take(body),
                });
                code = None;
            } else {
                body.push_str(line);
            }
        } else {
            let mark = trimmed.chars().next().unwrap_or(' ');
            let count = trimmed.chars().take_while(|c| *c == mark).count();
            if (mark == '`' || mark == '~') && count >= 3 {
                if !prose.is_empty() {
                    result.push(Block::Prose(std::mem::take(&mut prose)));
                }
                code = Some((
                    mark,
                    count,
                    trimmed[count..].trim().to_string(),
                    String::new(),
                ));
            } else {
                prose.push_str(line);
            }
        }
    }
    if let Some((_, _, language, text)) = code {
        result.push(Block::Code { language, text });
    }
    if !prose.is_empty() {
        result.push(Block::Prose(prose));
    }
    result
}

#[derive(Clone)]
pub struct CopyItem {
    pub label: String,
    pub text: String,
    pub command: String,
}
pub fn copy_items(messages: &[pitty_core::Message]) -> Vec<CopyItem> {
    let mut items = Vec::new();
    let mut code_number = 0;
    for (i, message) in messages.iter().enumerate() {
        items.push(CopyItem {
            label: format!("Message {} · {}", i + 1, message.role),
            text: message.content.clone(),
            command: format!("message {}", i + 1),
        });
        // Number blocks from the same sanitized text the transcript displays, so
        // "/copy code N" always matches the N shown on screen.
        for block in blocks(&pitty_ui::sanitize_copy(&message.content)) {
            if let Block::Code { language, text } = block {
                code_number += 1;
                items.push(CopyItem {
                    label: format!(
                        "Code {code_number} · {} · message {}",
                        if language.is_empty() {
                            "plain text"
                        } else {
                            &language
                        },
                        i + 1
                    ),
                    text,
                    command: format!("code {code_number}"),
                });
            }
        }
    }
    items
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn code_numbers_match_the_display_despite_control_characters() {
        let text = "\u{202e}```sh\necho hello\n```\n```sh\nmake deploy\n```";
        let items = copy_items(&[pitty_core::Message::user(text.into())]);
        let code: Vec<_> = items
            .iter()
            .filter(|item| item.command.starts_with("code"))
            .collect();
        assert_eq!(code.len(), 2);
        assert_eq!(code[0].text, "echo hello\n");
        assert_eq!(code[1].text, "make deploy\n");
    }
    #[test]
    fn copies_exact_code_without_fences_or_display_indentation() {
        let text = "Example:\n```rust\n\tfn main() {\n    println!(\"hi\");\n}\n```\nAfter";
        let items = copy_items(&[pitty_core::Message::user(text.into())]);
        assert_eq!(items[1].text, "\tfn main() {\n    println!(\"hi\");\n}\n");
        assert_eq!(items[1].command, "code 1");
    }
    #[test]
    fn handles_longer_fences_tildes_and_truncated_output() {
        assert_eq!(
            blocks("````md\n```\n````\n~~~rs\nlet x = 1;"),
            vec![
                Block::Code {
                    language: "md".into(),
                    text: "```\n".into()
                },
                Block::Code {
                    language: "rs".into(),
                    text: "let x = 1;".into()
                }
            ]
        );
    }
}

/// Small, deliberately bounded Markdown presentation: emphasis, inline code,
/// headings, lists and code fences. Original source remains available to copy.
pub fn prose_lines(text: &str, columns: usize) -> Vec<Vec<(String, u8)>> {
    let mut rows = Vec::new();
    for source in text.lines() {
        let heading = source.starts_with('#');
        let source = if heading {
            source.trim_start_matches('#').trim_start()
        } else {
            source
        };
        let mut chars = source.chars().peekable();
        let (mut bold, mut code, mut italic) = (heading, false, false);
        let mut cells = Vec::new();
        while let Some(c) = chars.next() {
            if c == '`' {
                code = !code;
                continue;
            }
            if !code && c == '*' && chars.peek() == Some(&'*') {
                chars.next();
                bold = !bold;
                continue;
            }
            if !code && c == '*' && (italic || chars.peek().is_some_and(|c| !c.is_whitespace())) {
                italic = !italic;
                continue;
            }
            cells.push((
                c,
                if code {
                    2
                } else if bold || italic {
                    1
                } else {
                    0
                },
            ));
        }
        if cells.is_empty() {
            rows.push(Vec::new());
            continue;
        }
        let mut start = 0;
        while start < cells.len() {
            let (mut end, mut width, mut space) = (start, 0, None);
            while end < cells.len() {
                let next = pitty_ui::char_width(cells[end].0);
                if width + next > columns.max(1) && end > start {
                    break;
                }
                width += next;
                if cells[end].0.is_whitespace() {
                    space = Some(end);
                }
                end += 1;
            }
            let cut = if end < cells.len() {
                space.filter(|v| *v > start).unwrap_or(end)
            } else {
                end
            };
            let mut spans: Vec<(String, u8)> = Vec::new();
            for &(c, style) in &cells[start..cut] {
                if let Some(last) = spans.last_mut().filter(|last| last.1 == style) {
                    last.0.push(c);
                } else {
                    spans.push((c.to_string(), style));
                }
            }
            rows.push(spans);
            start = cut;
            while start < cells.len() && cells[start].0.is_whitespace() {
                start += 1;
            }
        }
    }
    rows
}

pub fn code_spans(line: &str) -> Vec<(String, u8)> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if (c == '/' && chars.peek() == Some(&'/')) || c == '#' {
            out.push((format!("{c}{}", chars.collect::<String>()), 3));
            break;
        }
        if c == '"' || c == '\'' {
            let mut value = c.to_string();
            let mut escape = false;
            for next in chars.by_ref() {
                value.push(next);
                if next == c && !escape {
                    break;
                }
                escape = next == '\\' && !escape;
            }
            out.push((value, 2));
        } else if c.is_alphanumeric() || c == '_' {
            let mut word = c.to_string();
            while chars
                .peek()
                .is_some_and(|c| c.is_alphanumeric() || *c == '_')
            {
                word.push(chars.next().unwrap());
            }
            let kind = if [
                "fn", "let", "mut", "pub", "use", "impl", "struct", "enum", "match", "if", "else",
                "for", "while", "return", "async", "await", "const", "def", "class", "import",
                "from", "function", "true", "false", "None", "Some", "Ok", "Err",
            ]
            .contains(&word.as_str())
            {
                1
            } else {
                0
            };
            out.push((word, kind));
        } else {
            out.push((c.to_string(), 0));
        }
    }
    out
}

#[cfg(test)]
mod render_tests {
    #[test]
    fn markdown_is_styled_without_literal_markers_or_broken_words() {
        let lines = super::prose_lines("**Cargo** is a useful `tool`.", 18);
        let plain = lines
            .iter()
            .map(|spans| spans.iter().map(|s| s.0.as_str()).collect::<String>())
            .collect::<Vec<_>>();
        assert_eq!(plain, vec!["Cargo is a useful", "tool."]);
        assert_eq!(lines[0][0], ("Cargo".into(), 1));
        assert_eq!(lines[1][0], ("tool".into(), 2));
    }
}
