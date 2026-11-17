//! The document shell shared by every page.
use crate::{ORIGIN, RELEASE, SOURCE, assets::Site, html::esc};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Product,
    Download,
    Guide,
    Mcp,
}

pub struct Page {
    /// Output path inside website/.
    pub file: &'static str,
    /// Public URL path; `None` for pages that should not be indexed (404).
    pub url: Option<&'static str>,
    pub current: Option<Nav>,
    pub title: &'static str,
    pub description: &'static str,
    /// Extra elements for <head>, such as structured data.
    pub head: String,
    /// The complete <main> element.
    pub main: String,
}

/// Pitty's wordmark as the terminal draws it: each letter a saturated color
/// on a light ground of the same hue.
pub const WORDMARK: &str = r#"<span class="wordmark" aria-hidden="true"><span>P</span><span>I</span><span>T</span><span>T</span><span>Y</span></span>"#;
/// Legal operator of pitty.tech. The product mark stays Pitty's.
pub const OPERATOR: &str = "Ruption AI, Inc.";
pub const OPERATOR_URL: &str = "https://ruption.ai/";

fn nav_link(page: &Page, nav: Nav, href: &str, label: &str, class: &str) -> String {
    let current = if page.current == Some(nav) {
        r#" aria-current="page""#
    } else {
        ""
    };
    let class = if class.is_empty() {
        String::new()
    } else {
        format!(r#" class="{class}""#)
    };
    format!(r#"<a href="{href}"{class}{current}>{label}</a>"#)
}

fn header(page: &Page) -> String {
    let home = if page.current == Some(Nav::Product) {
        r#" aria-current="page""#
    } else {
        ""
    };
    let source = SOURCE
        .map(|url| format!(r#"<a href="{url}">GitHub</a>"#))
        .unwrap_or_default();
    format!(
        r#"<header class="site-header wrap"><a class="home" href="/" aria-label="Pitty home"{home}>{WORDMARK}</a><nav aria-label="Main">{}{}{source}{}</nav></header>"#,
        nav_link(page, Nav::Guide, "/guide/", "Guide", ""),
        nav_link(page, Nav::Mcp, "/mcp/", "MCP", ""),
        nav_link(
            page,
            Nav::Download,
            "/download/",
            "Download",
            "nav-download"
        ),
    )
}

fn footer(site: &Site) -> String {
    let source = SOURCE
        .map(|url| {
            format!(
                r#"<a href="{url}">Source code</a><a href="{url}/releases">Releases</a><a href="{url}/blob/main/SECURITY.md">Security</a>"#
            )
        })
        .unwrap_or_default();
    format!(
        r#"<footer class="site-footer"><div class="wrap footer-grid"><div class="footer-about"><a class="home" href="/" aria-label="Pitty home">{WORDMARK}</a><p>An AI agent for your terminal that asks before it changes anything.</p></div><nav aria-label="Footer"><a href="/download/">Download</a><a href="/guide/">Guide</a><a href="/mcp/">MCP setup</a>{source}</nav><a class="company" href="{OPERATOR_URL}"><img src="{mark}" alt="" width="32" height="32"><span>Made by {OPERATOR}</span></a><p class="fineprint">Pitty {RELEASE} for macOS and Linux, under the MIT License. Pitty is independent of OpenRouter, OpenAI, and model providers. <a href="{notices}">Open-source notices</a></p></div></footer>"#,
        mark = site.ruption_mark,
        notices = site.notices,
    )
}

pub fn document(site: &Site, page: &Page) -> String {
    let scripts = page.main.contains("data-copy=") || page.main.contains("data-demo");
    let csp = if scripts {
        "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'; style-src 'self'; img-src 'self'; font-src 'self'; base-uri 'none'; form-action 'none'"
    } else {
        "default-src 'self'; script-src 'none'; style-src 'self'; img-src 'self'; font-src 'self'; base-uri 'none'; form-action 'none'"
    };
    let url = page.url.unwrap_or("/");
    let indexing = match page.url {
        Some(url) => format!(r#"<link rel="canonical" href="{ORIGIN}{url}">"#),
        None => r#"<meta name="robots" content="noindex">"#.into(),
    };
    let fonts: String = site
        .fonts
        .iter()
        .map(|font| {
            format!(
                "\n<link rel=\"preload\" href=\"{font}\" as=\"font\" type=\"font/woff2\" crossorigin>"
            )
        })
        .collect();
    let browser = if scripts {
        format!(
            r#"
<link rel="preload" href="{wasm}" as="fetch" type="application/wasm" crossorigin>
<script type="module" src="{js}"></script>"#,
            wasm = site.browser_wasm,
            js = site.browser_js,
        )
    } else {
        String::new()
    };
    format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="{csp}">
<title>{title}</title>
<meta name="description" content="{description}">
{indexing}
<meta name="author" content="{OPERATOR}">
<meta name="color-scheme" content="light dark">
<meta name="theme-color" content="#eef1f5" media="(prefers-color-scheme: light)">
<meta name="theme-color" content="#0f141b" media="(prefers-color-scheme: dark)">{fonts}
<link rel="stylesheet" href="{css}">
<link rel="icon" href="{favicon}" type="image/svg+xml">
<link rel="apple-touch-icon" href="{touch}">
<meta property="og:type" content="website">
<meta property="og:site_name" content="Pitty">
<meta property="og:title" content="{title}">
<meta property="og:description" content="{description}">
<meta property="og:url" content="{ORIGIN}{url}">
<meta property="og:image" content="{ORIGIN}{social}">
<meta property="og:image:width" content="1280">
<meta property="og:image:height" content="640">
<meta property="og:image:alt" content="Pitty: the model proposes, you press Y. An AI agent for your terminal that shows every edit and command before it runs.">
<meta name="twitter:card" content="summary_large_image">{head}{browser}
</head>
<body>
<a class="skip" href="#main">Skip to content</a>
{header}
{main}
{footer}
</body>
</html>
"##,
        title = esc(page.title),
        description = esc(page.description),
        favicon = site.favicon,
        touch = site.touch_icon,
        css = site.css,
        social = site.social_image,
        head = page.head,
        header = header(page),
        main = page.main,
        footer = footer(site),
    )
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::assets::Site;

    pub fn site() -> Site {
        let show = |name: &str| format!("/{name}");
        Site {
            css: show("styles.css"),
            favicon: show("favicon.svg"),
            touch_icon: show("touch-icon.png"),
            social_image: show("social.png"),
            browser_js: show("browser.js"),
            browser_wasm: show("browser.wasm"),
            notices: show("notices.txt"),
            ruption_mark: show("ruption-mark.svg"),
            fonts: vec![show("sans.woff2"), show("mono.woff2")],
        }
    }

    fn page(main: &str) -> Page {
        Page {
            file: "index.html",
            url: Some("/"),
            current: Some(Nav::Product),
            title: "Pitty",
            description: "Review before it runs.",
            head: String::new(),
            main: main.into(),
        }
    }

    #[test]
    fn shell_names_ruption_and_links_the_source() {
        let html = document(&site(), &page(r#"<main id="main"></main>"#));
        assert!(html.contains("Made by Ruption AI, Inc."));
        assert!(html.contains("https://ruption.ai/"));
        assert!(html.contains("/ruption-mark.svg"));
        assert!(html.contains(r#"name="author" content="Ruption AI, Inc.""#));
        match SOURCE {
            Some(url) => assert!(html.contains(&format!(r#"href="{url}""#))),
            None => assert!(!html.contains("github.com")),
        }
        assert!(html.contains(r#"<link rel="preload" href="/sans.woff2" as="font""#));
        assert!(!html.contains("Inc.."));
        assert!(!html.contains("GoMain"));
        assert!(!html.contains("gomain.com"));
    }

    #[test]
    fn scripts_load_only_where_a_page_uses_them() {
        let still = document(&site(), &page(r#"<main id="main"></main>"#));
        assert!(still.contains("script-src 'none'"));
        assert!(!still.contains("/browser.js"));
        let demo = document(
            &site(),
            &page(r#"<main id="main"><div data-demo></div></main>"#),
        );
        assert!(demo.contains("'wasm-unsafe-eval'"));
        assert!(demo.contains(r#"<script type="module" src="/browser.js"></script>"#));
    }
}
