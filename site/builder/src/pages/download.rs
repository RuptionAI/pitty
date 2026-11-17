use super::TARGETS;
use crate::{
    ARCHIVE_CACHE, RELEASE, SOURCE,
    html::{code_box, esc, keys},
    layout::{Nav, Page},
};

/// The copy-and-paste installer for one platform family. It downloads the
/// archive and its checksum, verifies before extracting, checks the binary
/// runs, installs to ~/.local/bin, and then opens Pitty.
fn installer(cases: &[(&str, &str)], refusal: &str, checksum: &str) -> String {
    let cases: String = cases
        .iter()
        .map(|(uname, target)| format!("    \"{uname}\") pitty_target={target} ;;\n"))
        .collect();
    format!(
        r#"(
  set -eu
  pitty_cwd=$PWD
  case "$(uname -sm)" in
{cases}    *) printf '{refusal}\n' >&2; exit 1 ;;
  esac
  pitty_pkg="pitty-{RELEASE}-$pitty_target"
  pitty_tmp=$(mktemp -d "${{TMPDIR:-/tmp}}/pitty-install.XXXXXX")
  trap 'rm -rf "$pitty_tmp"' EXIT
  cd "$pitty_tmp"
  curl -fLsS --retry 2 --proto '=https' -O "https://pitty.tech/downloads/$pitty_pkg.tar.gz?v={ARCHIVE_CACHE}"
  curl -fLsS --retry 2 --proto '=https' -O "https://pitty.tech/downloads/$pitty_pkg.tar.gz.sha256?v={ARCHIVE_CACHE}"
  {checksum} -c "$pitty_pkg.tar.gz.sha256"
  tar -xzf "$pitty_pkg.tar.gz"
  "$pitty_pkg/pitty" --version
  mkdir -p "$HOME/.local/bin"
  install -m 755 "$pitty_pkg/pitty" "$HOME/.local/bin/pitty"
  cd "$pitty_cwd"
  "$HOME/.local/bin/pitty"
)"#
    )
}

pub fn page() -> Page {
    let mac = installer(
        &[
            ("Darwin arm64", "aarch64-apple-darwin"),
            ("Darwin x86_64", "x86_64-apple-darwin"),
        ],
        "Use this command on a Mac.",
        "shasum -a 256",
    );
    let linux = installer(
        &[("Linux x86_64", "x86_64-unknown-linux-gnu")],
        "Use this command on 64-bit Intel/AMD Linux.",
        "sha256sum",
    );
    let rows: String = TARGETS
        .iter()
        .map(|(target, title, note)| {
            let archive = format!("/downloads/pitty-{RELEASE}-{target}.tar.gz");
            let bust = format!("?v={ARCHIVE_CACHE}");
            format!(
                r#"<div class="download-row"><h3>{title}</h3><p>{note}</p><a class="download" download href="{archive}{bust}">Download archive</a><a class="checksum" download href="{archive}.sha256{bust}" aria-label="SHA-256 checksum for {title}">SHA-256</a></div>"#,
                title = esc(title),
                note = esc(note),
            )
        })
        .collect();
    let releases = SOURCE
        .map(|url| {
            format!(r#" The same files are on <a href="{url}/releases">GitHub Releases</a>."#)
        })
        .unwrap_or_default();
    let main = format!(
        r##"<main id="main" class="wrap page">
<header class="page-head"><h1>Install Pitty.</h1><p class="lede">Paste one block into your terminal. It downloads Pitty, checks the download, installs it, and opens it. The first change a model asks for waits for your Y.</p></header>
<ol class="steps"><li><strong>Choose your computer</strong><span>A Mac, or 64-bit Intel or AMD Linux.</span></li><li><strong>Copy its block</strong><span>The Copy button takes the whole thing.</span></li><li><strong>Paste it into Terminal</strong><span>Press Return. Pitty opens when it finishes.</span></li></ol>
<nav class="jumps" aria-label="Choose your computer"><a href="#mac">macOS<span>Apple silicon and Intel</span></a><a href="#linux">Linux<span>Intel and AMD x64</span></a><a href="#archives">Archives<span>Files and checksums</span></a></nav>
<section id="mac" class="platform" aria-labelledby="mac-title"><div class="platform-text"><h2 id="mac-title">On a Mac</h2><p>One block covers both Apple silicon and Intel Macs on macOS 11 or later. To open Terminal, press {spotlight}, type <strong>Terminal</strong>, and press Return.</p><p class="note">This release is not signed with an Apple Developer ID or notarized; that is the next Mac build. The installer downloads with <code>curl</code>, which macOS does not quarantine. <a href="/guide/#mac-security">If macOS blocks a download from your browser</a></p></div><div class="platform-code">{mac_box}<p class="note">Installs to <code>~/.local/bin/pitty</code>. No administrator password or Rust toolchain needed. Run it again to update.</p></div></section>
<section id="linux" class="platform" aria-labelledby="linux-title"><div class="platform-text"><h2 id="linux-title">On Linux</h2><p>For 64-bit Intel or AMD Linux with glibc 2.35 or later, which includes Ubuntu 22.04 and newer. It needs <code>curl</code>, <code>tar</code>, and <code>sha256sum</code>.</p><p class="note">Linux on ARM is not in this release. Windows users can try this build inside x64 WSL2; that path is untested.</p></div><div class="platform-code">{linux_box}<p class="note">The checksum must pass before anything is unpacked, and the new Pitty must run before it replaces an installed one.</p></div></section>
<section id="first-run" class="first-run" aria-labelledby="first-run-title"><h2 id="first-run-title">When Pitty opens</h2><ol><li>Type <code>/key</code> and paste your <a href="https://openrouter.ai/keys">OpenRouter API key</a> into the masked field. OpenRouter is the default.</li><li>Prefer another provider? Type <code>/interface openai</code>, or <code>/interface compatible</code> with a base URL, then <code>/model</code> and a model id. A compatible endpoint can run without a key.</li><li>On OpenRouter, type <code>/models free</code> and pick a model. <code>/help</code> lists every command.</li></ol><p>Model requests use the account behind that interface and may cost money. Web search and PDF reading belong to OpenRouter. Your own <code>/run</code> commands and MCP mode need no chat key.</p><p class="more"><a class="text-link" href="/guide/">Read the guide</a><a class="text-link" href="/mcp/">Connect an MCP client</a></p></section>
<section id="archives" class="archives" aria-labelledby="archives-title"><h2 id="archives-title">Or download an archive</h2><p>Version {RELEASE}. Save the archive and its checksum, then follow the <a href="/guide/#manual-install">manual installation steps</a>.{releases}</p><div class="download-list">{rows}</div><p class="note">A SHA-256 checksum detects a damaged or altered archive; it is not a publisher signature. Each archive holds the program, installation notes, build details, and license notices.</p></section>
<section class="reopen" aria-labelledby="reopen-title"><div><h2 id="reopen-title">Next time</h2><p>This starts Pitty even before <code>~/.local/bin</code> is on your PATH. <a href="/guide/#command-not-found">Add it to your PATH</a></p></div>{reopen}</section>
</main>"##,
        spotlight = keys(&["⌘", "Space"]),
        mac_box = code_box(
            "install-mac",
            "Terminal on macOS",
            "macOS installation commands",
            "Copy",
            &mac
        ),
        linux_box = code_box(
            "install-linux",
            "Terminal on Linux",
            "Linux installation commands",
            "Copy",
            &linux
        ),
        reopen = code_box(
            "reopen",
            "Terminal",
            "the command that starts Pitty",
            "Copy",
            "\"$HOME/.local/bin/pitty\""
        ),
    );
    Page {
        file: "download/index.html",
        url: Some("/download/"),
        current: Some(Nav::Download),
        title: "Install Pitty on macOS or Linux",
        description: "Paste one block on macOS or Linux. It verifies the SHA-256 checksum, installs Pitty, and opens it. The first change a model asks for waits for your Y.",
        head: String::new(),
        main,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installers_verify_before_extracting_and_name_the_release() {
        let mac = installer(
            &[("Darwin arm64", "aarch64-apple-darwin")],
            "Mac only.",
            "shasum -a 256",
        );
        let verify = mac.find("shasum -a 256 -c").unwrap();
        assert!(verify < mac.find("tar -xzf").unwrap());
        assert!(mac.contains(&format!("pitty-{RELEASE}-$pitty_target")));
        assert!(mac.contains(&format!("?v={ARCHIVE_CACHE}")));
        let html = page().main;
        assert!(html.contains(&format!(
            "/downloads/pitty-{RELEASE}-aarch64-apple-darwin.tar.gz.sha256?v={ARCHIVE_CACHE}"
        )));
        assert!(!html.contains(&format!("?v={ARCHIVE_CACHE}.sha256")));
        assert!(mac.contains("${TMPDIR:-/tmp}"));
        assert!(mac.starts_with("(\n  set -eu"));
    }
}
