use crate::{
    html::code_box,
    layout::{Nav, Page},
};

const CONFIG: &str = r#"{
  "mcpServers": {
    "pitty": {
      "command": "/absolute/path/to/pitty",
      "args": [
        "--mcp",
        "--directory", "/absolute/path/to/project",
        "--tools", "ask"
      ]
    }
  }
}"#;

pub fn page() -> Page {
    let tools: String = [
        ("list_files", "List a directory", "Ask / Auto"),
        ("read_file", "Read text from a file", "Ask / Auto"),
        ("search_files", "Find text in project files", "Ask / Auto"),
        ("write_file", "Create or overwrite a file", "Auto"),
        ("edit_file", "Replace exact text", "Auto"),
        ("run_command", "Run a command and return its output", "Auto"),
    ]
    .iter()
    .map(|(name, what, mode)| {
        format!("<tr><td><code>{name}</code></td><td>{what}</td><td>{mode}</td></tr>")
    })
    .collect();
    let main = format!(
        r##"<main id="main" class="wrap page manual">
<header class="page-head"><h1>Same tools. The client’s model.</h1><p class="lede">The same file and command tools, behind the same approval boundary, in a client that already has a model. No OpenRouter key is used. Ask is read-only: standard input carries the protocol, so Pitty cannot show a Y/N prompt and declines anything that needs one. New here? <a href="/download/">Install Pitty first</a>.</p></header>
<div class="manual-body">
<nav class="toc" aria-label="On this page"><p>On this page</p><a href="#quick">One-command setup</a><a href="#configuration">Other clients</a><a href="#permissions">Permissions</a></nav>
<div class="manual-content">
<section id="quick" class="manual-section" aria-labelledby="quick-title"><h2 id="quick-title">Add Pitty in one command.</h2><p>Open a terminal in the project folder you want the client to work in, then run the line for your client. The shell fills in both absolute paths for you.</p>{claude}{codex}<p>Each command starts Pitty in Ask mode, which exposes read-only tools. Claude Code saves the server for the current project. Codex saves it in <code>~/.codex/config.toml</code>, which every project shares, so give each additional project its own name, such as <code>pitty-site</code>.</p></section>
<section id="configuration" class="manual-section" aria-labelledby="configuration-title"><h2 id="configuration-title">Configure any other client.</h2><p>Apps such as Claude Desktop and Cursor read a JSON configuration. First print the two absolute paths it needs, from inside your project folder:</p>{paths}<p>Then add a local <strong>stdio</strong> server to the client’s MCP settings, replacing both paths with the ones printed above:</p>{config}<p>Clients do not expand <code>~</code> or <code>$HOME</code> inside JSON, so use the full paths. Some clients use a form instead: set the command, then add each entry in <code>args</code> as its own argument. VS Code names the top-level key <code>servers</code> instead of <code>mcpServers</code>.</p><p>Starting <code>pitty --mcp</code> by hand waits silently for protocol messages; it does not open the chat. Your client starts and stops the process.</p></section>
<section id="permissions" class="manual-section" aria-labelledby="permissions-title"><h2 id="permissions-title">Choose what the client can do.</h2><dl class="manual-modes"><div><dt><code>ask</code>: read-only tools</dt><dd>List directories, read files, and search text. Anything that would need your approval, including sensitive reads, is declined, because standard input carries MCP messages and cannot show a prompt.</dd></div><div><dt><code>auto</code>: all six tools</dt><dd>Change <code>ask</code> to <code>auto</code> to add file creation, editing, and command execution. Set up confirmation in your MCP client. Credential-like files, such as <code>.env</code> and SSH keys, are never read through MCP in either mode.</dd></div></dl><table><thead><tr><th scope="col">Tool</th><th scope="col">What it does</th><th scope="col">Mode</th></tr></thead><tbody>{tools}</tbody></table><p>File tools stay inside the project directory you pass; folders connected in the terminal chat do not widen that scope. Commands run with your account’s filesystem and network permissions. Cancellation, a closed connection, and termination signals stop running work and its commands. The client receives every tool result and decides where it goes next.</p></section>
</div>
</div>
</main>"##,
        claude = code_box(
            "mcp-claude",
            "Claude Code: run in your project folder",
            "the Claude Code command that adds Pitty",
            "Copy",
            r#"claude mcp add pitty -- "$HOME/.local/bin/pitty" --mcp --directory "$PWD" --tools ask"#,
        ),
        codex = code_box(
            "mcp-codex",
            "Codex CLI: run in your project folder",
            "the Codex command that adds Pitty",
            "Copy",
            r#"codex mcp add pitty -- "$HOME/.local/bin/pitty" --mcp --directory "$PWD" --tools ask"#,
        ),
        paths = code_box(
            "mcp-paths",
            "Terminal: prints the program path, then the project path",
            "the commands that print both paths",
            "Copy",
            "printf '%s\\n' \"$HOME/.local/bin/pitty\"\npwd -P",
        ),
        config = code_box(
            "mcp-config",
            "JSON: replace both paths",
            "the MCP configuration template",
            "Copy",
            CONFIG,
        ),
    );
    Page {
        file: "mcp/index.html",
        url: Some("/mcp/"),
        current: Some(Nav::Mcp),
        title: "Pitty over MCP: the same tools for your client",
        description: "Serve Pitty’s local file and command tools to an MCP client. Ask is read-only. No OpenRouter key is used. Credential-like files are never read.",
        head: String::new(),
        main,
    }
}
