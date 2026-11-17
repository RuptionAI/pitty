use super::TARGETS;
use crate::{
    RELEASE,
    html::{code_box, command_list, esc, keys},
    layout::{Nav, Page},
};

fn manual_install() -> String {
    TARGETS
        .iter()
        .map(|(target, title, _)| {
            let package = format!("pitty-{RELEASE}-{target}");
            let checksum = if target.contains("apple") {
                "shasum -a 256"
            } else {
                "sha256sum"
            };
            let commands = format!(
                "{checksum} -c {package}.tar.gz.sha256 &&\ntar -xzf {package}.tar.gz &&\nmkdir -p \"$HOME/.local/bin\" &&\ninstall -m 755 {package}/pitty \"$HOME/.local/bin/pitty\" &&\n\"$HOME/.local/bin/pitty\" --version"
            );
            code_box(
                &format!("manual-{target}"),
                &format!("{title}: run in the download folder"),
                &format!("manual installation commands for {title}"),
                "Copy commands",
                &commands,
            )
        })
        .collect()
}

fn shortcuts() -> String {
    let rows: &[(&str, &str)] = &[
        ("Enter", "Send a message or run a command"),
        (
            "Ctrl + J",
            "Insert a new line (Alt + Enter works where the terminal sends it)",
        ),
        ("/", "Open the command menu"),
        (
            "↑ / ↓",
            "Scroll the chat when the composer is empty; move through a picker or menu",
        ),
        (
            "Page Up / Page Down",
            "Scroll one page, even while you type",
        ),
        ("Tab", "Complete the selected command"),
        ("Ctrl + U", "Clear the draft"),
        ("Ctrl + K", "Delete to the end of the line"),
        ("Ctrl + L", "Redraw the screen"),
        ("Ctrl + Y", "Copy the selected text, or the latest reply"),
        ("F2", "Native terminal selection; F2 or Esc returns"),
        (
            "Esc",
            "Clear a selection, close a panel, or clear the draft",
        ),
        (
            "Ctrl + C",
            "Stop a request and restore your draft; when idle, clear the draft, then ask about closing",
        ),
        ("Ctrl + Q", "Ask about closing the session"),
    ];
    let body: String = rows
        .iter()
        .map(|(key, action)| {
            let key = keys(&key.split(" + ").collect::<Vec<_>>());
            format!("<tr><td>{key}</td><td>{}</td></tr>", esc(action))
        })
        .collect();
    format!(
        r#"<table class="shortcuts"><caption class="visually-hidden">Keyboard controls</caption><thead><tr><th scope="col">Key</th><th scope="col">What it does</th></tr></thead><tbody>{body}</tbody></table>"#
    )
}

pub fn page() -> Page {
    let main = format!(
        r##"<main id="main" class="wrap page manual">
<header class="page-head"><h1>Using Pitty.</h1><p class="lede">Your key and model, the review, files and commands, and the record of what ran. New here? <a href="/download/">Install Pitty first</a>.</p></header>
<div class="manual-body">
<nav class="toc" aria-label="On this page"><p>On this page</p><a href="#first-chat">First chat</a><a href="#tools">Local tools</a><a href="#files">Files &amp; tasks</a><a href="#copy">Keys &amp; copying</a><a href="#data">Data &amp; costs</a><a href="#troubleshooting">Installation help</a></nav>
<div class="manual-content">
<section id="first-chat" class="manual-section" aria-labelledby="first-chat-title"><h2 id="first-chat-title">Start a conversation.</h2><p>Pitty opens on OpenRouter. Enter <code>/key</code> and paste your <a href="https://openrouter.ai/keys">OpenRouter API key</a> into the masked field; it is saved on your computer with owner-only permissions. Then choose a model:</p>{models}<p>OpenAI and another chat endpoint use the same review. Name the model. Web search and PDF parsing stay on OpenRouter. A compatible endpoint can run with no key.</p>{interfaces}<p>Send a message. <code>/help</code> lists every command, and typing <code>/</code> opens the command menu. Local tools need a model that supports function calling; use <code>/tools off</code> for a chat-only model.</p><h3>Choose a working folder</h3>{folder}<p>The header shows the working folder. <code>/pwd</code> prints its full path and any connected folders.</p></section>
<section id="tools" class="manual-section" aria-labelledby="tools-title"><h2 id="tools-title">Files. Commands. Results.</h2><p>Ask Pitty to inspect a file, make a change, and run a test. To run a command yourself, start it with <code>/run</code> or <code>/exec</code>:</p>{local}<dl class="manual-modes"><div><dt><code>/tools ask</code>, the default</dt><dd>Preview file changes, commands, and sensitive reads. <kbd>Y</kbd> approves once; <kbd>N</kbd>, <kbd>Enter</kbd>, or <kbd>Esc</kbd> declines. Ordinary reads and searches run without individual approval.</dd></div><div><dt><code>/tools auto</code></dt><dd>Allow model-requested tools without individual review, for new requests in this session.</dd></div><div><dt><code>/tools off</code></dt><dd>Turn off model-requested tools. Chat and your own commands still work.</dd></div></dl><p>Your <code>/run</code> commands execute exactly what you typed. Commands have your account’s filesystem and network permissions, including access outside connected folders. They use noninteractive input, a timeout, and bounded output. A model-requested shell starts from a cleared environment, so API-key variables, including <code>OPENROUTER_API_KEY</code>, <code>OPENAI_API_KEY</code>, and <code>PITTY_API_KEY</code>, are not inherited.</p><p>Pitty checks a write against the file it showed you. If the file changed after approval, the write does not land and Pitty reports <strong>File changed after approval; prepare the edit again.</strong> A credential-like path, such as <code>.env</code> or an SSH key, needs its own approval before a read, and the review says the contents will be sent to the model. Search skips those files. Over MCP, a credential read is declined in either mode.</p><p>{ctrl_c} stops active work; completed file changes stay. <code>/actions</code> lists what ran and its output. <code>/audit</code> checks the decision log: each approval, decline, and run is one line that stores the SHA-256 of the line before it, so an edited or deleted line breaks the chain. The log keeps decisions, never file contents or command output. When nothing is running, Ctrl+C clears the draft, and a second Ctrl+C asks before closing. <code>/retry</code> will not repeat a turn that already performed local actions.</p></section>
<section id="files" class="manual-section" aria-labelledby="files-title"><h2 id="files-title">Bring the work with you.</h2>{files}<p>Browse connected local or mounted network folders, or drag file paths into the composer. Review staged files before sending. <code>/detach 1</code> removes one staged file; <code>/detach all</code> clears the list. Images, PDFs, audio, video, and UTF-8 text depend on the selected model and provider; unsupported files are named. OpenRouter can parse a PDF in the request. Other chat interfaces send the conversation without that parser.</p><p>Connecting a folder does not upload it. Enabled model tools may read its files as part of your task, and those results are sent to the model.</p><h3>Give tasks their own space</h3>{agents}<p>Up to four model requests run at once, including the main chat. Tasks inherit the conversation, attachments, and tool policy. Use <code>/agent open 1</code> to continue from a finished result; handoffs are manual.</p><h3>Turn web search on or off</h3>{web}<p>The footer always shows whether search is on. It is off at startup, and searches can add charges. Web search belongs to the OpenRouter interface. This switch does not limit a shell command’s network access.</p></section>
<section id="copy" class="manual-section" aria-labelledby="copy-title"><h2 id="copy-title">Keep your place.</h2><p>Scroll with the mouse wheel, arrow keys, or Page Up and Page Down; the composer stays in place. Drag over chat text, then click <strong>Copy selection</strong> or press {ctrl_y}. With nothing selected, Ctrl+Y copies the latest reply.</p>{copy}{shortcuts}<p><kbd>F2</kbd> switches to your terminal’s own selection. On Linux, copying needs <code>wl-copy</code> or <code>xclip</code>. <code>/name</code> and <code>/team</code> set local display labels.</p></section>
<section id="data" class="manual-section" aria-labelledby="data-title"><h2 id="data-title">Your account &amp; your history.</h2><p>Model chat uses the key for the interface you chose. On OpenRouter, inference, web search, and parallel tasks may incur charges, so set a spending limit on the key. OpenAI and another endpoint bill on their own terms. A compatible endpoint can run with no key. A tool-assisted turn can make up to 16 model requests, and the token cap applies to each one.</p><p>Conversations, attachments, and tool results are saved locally in plaintext by default. To keep a session in memory only:</p>{no_history}<p>Sent conversations, files, and tool results go to the chat endpoint you chose, whose retention policies vary. Turning off local history does not change provider retention.</p></section>
<section id="troubleshooting" class="manual-section" aria-labelledby="troubleshooting-title"><h2 id="troubleshooting-title">Installation help.</h2><h3 id="mac-security">macOS security prompts</h3><p>This release is not signed with an Apple Developer ID and is not notarized. The copy-and-paste installer downloads with <code>curl</code>, which macOS does not quarantine. If macOS blocks a copy you downloaded in a browser, follow <a href="https://support.apple.com/en-us/102445">Apple’s instructions for opening apps safely</a>. Pitty never changes Gatekeeper settings. A notarized Mac install is the next Mac build.</p><h3 id="manual-install">Installing a downloaded archive</h3><p><a href="/download/#archives">Download your archive and its checksum</a>, open a terminal in the download folder, and run the commands for your computer. Nothing is extracted unless the checksum reports <strong>OK</strong>.</p>{manual}<p>The <a href="/download/">copy-and-paste installer</a> does all of this in one step.</p><h3 id="command-not-found">“Command not found”</h3><p>Start Pitty by its full path:</p>{full_path}<p>To make <code>pitty</code> work in new terminals, add <code>~/.local/bin</code> to your PATH. For zsh, the macOS default:</p>{zsh}<p>For bash, use <code>~/.bashrc</code> instead of <code>~/.zshrc</code>. Open a new terminal afterwards.</p><h3>Updates &amp; removal</h3><p>Run the current <a href="/download/">installation block</a> again to update. Deleting <code>~/.local/bin/pitty</code> removes the program; your key, preferences, and history stay. <code>/status</code> shows where history is stored.</p></section>
</div>
</div>
</main>"##,
        models = code_box(
            "first-chat-commands",
            "Inside Pitty",
            "the model catalog command",
            "Copy",
            "/models free"
        ),
        interfaces = command_list(
            "interface-commands",
            "Inside Pitty",
            &[
                "/interface",
                "/interface openai",
                "/interface compatible http://127.0.0.1:11434/v1",
                "/model llama3.2",
            ],
        ),
        folder = code_box(
            "working-folder",
            "Terminal (replace the project path)",
            "the command that opens Pitty in a project folder",
            "Copy",
            "\"$HOME/.local/bin/pitty\" --directory /path/to/project",
        ),
        local = command_list(
            "local-commands",
            "Inside Pitty",
            &["/run cargo test", "/actions"]
        ),
        ctrl_c = keys(&["Ctrl", "C"]),
        ctrl_y = keys(&["Ctrl", "Y"]),
        files = command_list(
            "files-commands",
            "Inside Pitty (replace the example paths)",
            &[
                "/connect ~/Projects/example",
                "/files",
                "/attach ./diagram.png",
                "/attachments"
            ],
        ),
        agents = command_list(
            "agent-commands",
            "Inside Pitty",
            &[
                "/agent planner Outline the changes",
                "/agent reviewer Check the implementation",
                "/agents"
            ],
        ),
        web = command_list("web-commands", "Inside Pitty", &["/web on", "/web off"]),
        copy = command_list(
            "copy-commands",
            "Inside Pitty",
            &["/copy code 1", "/export review.md"]
        ),
        shortcuts = shortcuts(),
        no_history = code_box(
            "no-history",
            "Terminal",
            "the command that starts Pitty without saving history",
            "Copy",
            "\"$HOME/.local/bin/pitty\" --no-history"
        ),
        manual = manual_install(),
        full_path = code_box(
            "full-path",
            "Terminal",
            "Pitty’s full path",
            "Copy",
            "\"$HOME/.local/bin/pitty\""
        ),
        zsh = code_box(
            "path-zsh",
            "Terminal (zsh)",
            "the command that adds ~/.local/bin to PATH",
            "Copy",
            "echo 'export PATH=\"$HOME/.local/bin:$PATH\"' >> ~/.zshrc",
        ),
    );
    Page {
        file: "guide/index.html",
        url: Some("/guide/"),
        current: Some(Nav::Guide),
        title: "Using Pitty: keys, the review, files, and the record",
        description: "How to use Pitty: the Y/N review, the chat interface you choose, refused stale edits, credential reads, a shell that drops API keys, and /actions.",
        head: String::new(),
        main,
    }
}
