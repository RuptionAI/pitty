//! Escaping and the small set of repeated page components.

/// Escape text for HTML element content and double-quoted attributes.
pub fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(ch),
        }
    }
    out
}

fn status(id: &str) -> String {
    format!(
        r#"<p class="copy-status" data-copy-status="{id}" role="status" aria-live="polite"></p>"#
    )
}

/// A dark code panel with a label and a copy button. The displayed code is the
/// only source of copied text; the button appears once WebAssembly is ready.
pub fn code_box(id: &str, label: &str, what: &str, button: &str, code: &str) -> String {
    format!(
        r#"<div class="command-box"><div class="command-toolbar"><span>{label}</span><button type="button" class="copy-button" data-copy="{id}" aria-label="Copy {what}" disabled>{button}</button></div><pre tabindex="0" role="group" aria-label="{what}"><code id="{id}">{code}</code></pre>{status}</div>"#,
        label = esc(label),
        what = esc(what),
        button = esc(button),
        code = esc(code),
        status = status(id),
    )
}

/// One-line commands, each with its own copy button.
pub fn command_list(id: &str, label: &str, commands: &[&str]) -> String {
    let mut html = format!(
        r#"<div class="command-list"><p class="command-list-label">{}</p>"#,
        esc(label)
    );
    for (index, command) in commands.iter().enumerate() {
        let id = format!("{id}-{}", index + 1);
        html.push_str(&format!(
            r#"<div class="command-line"><code id="{id}">{code}</code><button type="button" class="copy-button" data-copy="{id}" aria-label="Copy {code}" disabled>Copy</button>{status}</div>"#,
            code = esc(command),
            status = status(&id),
        ));
    }
    html.push_str("</div>");
    html
}

/// Keyboard keys joined with plus signs.
pub fn keys(keys: &[&str]) -> String {
    keys.iter()
        .map(|key| format!("<kbd>{}</kbd>", esc(key)))
        .collect::<Vec<_>>()
        .join(" + ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escaping_covers_markup_and_both_quotes() {
        assert_eq!(
            esc(r#"<a href="x">'&'</a>"#),
            "&lt;a href=&quot;x&quot;&gt;&#x27;&amp;&#x27;&lt;/a&gt;"
        );
    }
    #[test]
    fn code_boxes_pair_buttons_with_targets() {
        let html = code_box(
            "demo",
            "Terminal",
            "demo commands",
            "Copy",
            "echo \"hi\" && ls <dir>",
        );
        assert!(html.contains(r#"data-copy="demo""#));
        assert!(
            html.contains(
                r#"<code id="demo">echo &quot;hi&quot; &amp;&amp; ls &lt;dir&gt;</code>"#
            )
        );
        assert!(html.contains(r#"data-copy-status="demo""#));
        assert!(html.contains("disabled"));
    }
    #[test]
    fn command_lists_number_each_command() {
        let html = command_list("cmd", "Inside Pitty", &["/run cargo test", "/actions"]);
        assert!(html.contains(r#"<code id="cmd-1">/run cargo test</code>"#));
        assert!(html.contains(r#"data-copy="cmd-2""#));
    }
}
