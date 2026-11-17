//! Builds pitty.tech from Rust.
//!
//! Every page is rendered here, the copy buttons are compiled from Rust to
//! WebAssembly, and every asset is fingerprinted, so `website/` is entirely a
//! build product. The only JavaScript served is the binding module that
//! wasm-bindgen generates from the Rust crate; browsers need it to start
//! WebAssembly and reach the page.
mod assets;
mod html;
mod layout;
mod output;
mod pages;
mod serve;

use std::{path::PathBuf, process::ExitCode};

/// The release whose archives are published under /downloads/.
pub const RELEASE: &str = "0.7.0";
/// Cache-bust for archive URLs. Cloudflare kept the previous Apple Silicon
/// tarball for 180 days under the bare filename, so a replaced archive needs
/// a new query before the one-paste installer will see it.
pub const ARCHIVE_CACHE: &str = "70e1c0d5";
pub const ORIGIN: &str = "https://pitty.tech";
/// Public source repository: `Some("https://github.com/RuptionAI/pitty")` once
/// that repository is public. While it is private, the site neither links to
/// it nor calls Pitty open source.
pub const SOURCE: Option<&str> = None;

pub type Result<T> = std::result::Result<T, String>;

const USAGE: &str = "\
pitty-site: build the pitty.tech website from Rust

Usage: cargo run --manifest-path site/Cargo.toml -p pitty-site -- COMMAND

Commands:
  build        Compile the browser WebAssembly, render every page, and write website/
  pages        Render pages with the browser assets already in website/ (no Wasm toolchain)
  check        Fail if website/ differs from what `pages` would write
  serve [PORT] Preview website/ at http://127.0.0.1:PORT/ (default 8000)";

pub fn repository_root() -> PathBuf {
    // site/builder → repository root.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|site| site.parent())
        .map(PathBuf::from)
        .unwrap_or(manifest)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("build") => build(true),
        Some("pages") => build(false),
        Some("check") => check(),
        Some("serve") => {
            let port = args.get(1).map(|p| p.parse::<u16>()).transpose();
            match port {
                Ok(port) => serve::run(&repository_root().join("website"), port.unwrap_or(8000)),
                Err(_) => Err("PORT must be a number from 1 to 65535.".into()),
            }
        }
        Some("-h" | "--help" | "help") | None => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some(other) => Err(format!("Unknown command `{other}`.\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("pitty-site: {error}");
            ExitCode::FAILURE
        }
    }
}

fn render(rebuild_wasm: bool) -> Result<output::Tree> {
    let root = repository_root();
    let website = root.join("website");
    let browser = if rebuild_wasm {
        assets::build_browser_bundle(&root)?
    } else {
        assets::existing_browser_bundle(&website)?
    };
    let mut tree = output::Tree::default();
    let site = assets::collect(&root, browser, &mut tree)?;
    for page in pages::all() {
        let document = layout::document(&site, &page);
        tree.insert(page.file, document.into_bytes())?;
    }
    for (path, text) in output::server_files() {
        tree.insert(path, text.into_bytes())?;
    }
    output::validate(&tree)?;
    Ok(tree)
}

fn build(rebuild_wasm: bool) -> Result<()> {
    let tree = render(rebuild_wasm)?;
    let website = repository_root().join("website");
    let summary = output::write(&tree, &website)?;
    println!("{summary}");
    Ok(())
}

fn check() -> Result<()> {
    let tree = render(false)?;
    let website = repository_root().join("website");
    let differences = output::compare(&tree, &website)?;
    if differences.is_empty() {
        println!("website/ is up to date ({} files).", tree.len());
        Ok(())
    } else {
        Err(format!(
            "website/ is out of date. Run `cargo run --manifest-path site/Cargo.toml -p pitty-site -- pages`.\n{}",
            differences.join("\n")
        ))
    }
}
