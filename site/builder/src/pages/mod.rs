//! Page content. Text lives here, in Rust; layout.rs supplies the shell.
mod download;
mod guide;
mod index;
mod mcp;
mod not_found;
mod video;

use crate::layout::Page;

/// Public pages, in sitemap order.
pub const INDEXED: &[&str] = &["/", "/download/", "/guide/", "/mcp/", "/demo/"];

pub fn all() -> Vec<Page> {
    vec![
        index::page(),
        download::page(),
        guide::page(),
        mcp::page(),
        video::page(),
        not_found::page(),
    ]
}

/// Supported release targets: archive triple, title, and requirement note.
pub const TARGETS: &[(&str, &str, &str)] = &[
    (
        "aarch64-apple-darwin",
        "Mac with Apple silicon",
        "M1 or newer, macOS 11 or later",
    ),
    ("x86_64-apple-darwin", "Mac with Intel", "macOS 11 or later"),
    (
        "x86_64-unknown-linux-gnu",
        "Linux on x64",
        "Intel or AMD, glibc 2.35 or later",
    ),
];

/// The closing links shared by the home and 404 pages.
pub fn next_steps(title: &str) -> String {
    format!(
        r#"<section class="next wrap" aria-labelledby="next-title"><h2 id="next-title">{title}</h2><div class="next-links"><a href="/download/"><strong>Install Pitty</strong><span>One paste on macOS or Linux. The checksum is verified first.</span></a><a href="/guide/"><strong>Read the guide</strong><span>Keys, models, the review, files, and the record.</span></a><a href="/mcp/"><strong>Connect an MCP client</strong><span>The same tools for Claude Code, Codex, or Cursor.</span></a></div></section>"#
    )
}
