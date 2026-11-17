# Security policy

## Supported versions

Only the latest release gets security fixes. Fixes ship in a new release, and
older versions are not patched. Run `pitty --version` to check your version.

## Reporting a vulnerability

Report it privately through GitHub. Open the
[Security tab](https://github.com/RuptionAI/pitty/security) of
github.com/RuptionAI/pitty and choose **Report a vulnerability**. This opens a
private advisory that only you and the maintainers can see. Do not open a
public issue, discussion, or pull request for a suspected vulnerability.

Please include:

- the output of `pitty --version`, your operating system, and your terminal
  app;
- the mode (terminal chat, one-shot, or `--mcp`), the `--tools` setting, and
  the chat interface;
- the steps to reproduce it, and what an attacker gains.

Use a test key and test files. Never send a real API key, key file, or private
conversation.

## In scope

- **The approval boundary.** In `/tools ask`, the default, a file change,
  command, or credential-like read that the model requests runs only after you
  press Y. It must run exactly what the review showed. A change is refused if
  the file changed after Pitty read it, and file tools stay inside the working
  and connected folders without following symlinks. A model that proposes a
  harmful action is not a vulnerability by itself, because the review exists
  for that. A way around the review, or a review that misstates what will run,
  is in scope.
- **Credential-file handling.** Credential-like paths need approval to read,
  are skipped by `search_files`, and are never read over MCP. These include
  `.env` files, `.ssh`, `.aws`, `.gnupg`, `credentials`, `secrets.json`,
  `secrets.toml`, `id_rsa*`, `id_ed25519*`, `*.pem`, `*.key`, and spellings
  that some volumes resolve to those names. API keys are saved with owner-only
  permissions. They are redacted from error messages, from saved `/run` and
  `/exec` output, and from the audit log.
- **Environment scrubbing for commands.** Commands that the model requests, and
  `/run` and `/exec` commands, start with an empty environment. Only `PATH`,
  `LANG`, `TERM`, `GIT_TERMINAL_PROMPT`, `HOME`, `TMPDIR`, `CARGO_HOME`,
  `CARGO_TARGET_DIR`, and `RUSTUP_HOME` are set, and there is no controlling
  terminal. An API key or other inherited variable that reaches a command is in
  scope.
- **MCP mode (`--mcp`).** `--tools ask` exposes only read-only tools. File tools
  stay inside `--directory`. Credential-like files are declined in every mode.
  Running commands stop when the client closes Pitty's output or when the
  server receives SIGINT, SIGTERM, or SIGHUP.
- **Web tools.** With Pitty's own web search on, `web_search` sends a query to
  the provider you chose and `web_fetch` downloads one page. In Ask mode each
  one must show for review, naming the query or address and where it goes.
  `web_fetch` must refuse loopback, private, link-local, and other reserved
  addresses, including through redirects and DNS answers that change between
  the check and the connection. A fetch that reaches a non-public address, or
  a web request that runs in Ask mode without a review, is in scope.
- **Auto-approval.** Pressing A, Shift+Tab, and `/tools` change one shared
  setting. Turning Auto off must take effect at the next action of every
  running request, and a long task in Ask must stop when you decline its
  check-in.
- **The audit log check.** `/audit` must not report an intact chain when a line
  in the middle of the log was changed, removed, or reordered. File contents and
  command output must never be written to the log.
- **Terminal escape filtering.** Model replies, tool output, and peer room
  messages must not send escape sequences or other control characters to your
  terminal.
- **Release archives and checksums.** This covers the `.tar.gz` archives and
  `.sha256` files offered for download, and the steps in this repository that
  produce them: `tools/build`, `tools/release`, and the Release packages
  workflow.

## Not vulnerabilities

These are documented limits:

- **Commands run with your own permissions.** Pitty is not a sandbox. An
  approved command, a `/run` or `/exec` command, or a command under
  `/tools auto` can do anything your user account can do, including reading
  files outside the working folder and using the network.
- **Auto skips review on purpose.** `/tools auto`, A at a review, and
  Shift+Tab run file changes, commands, web requests, and credential-like
  reads without asking, including in work that is already running, until you
  switch back. In Auto, a web page the model reads can steer what it does
  next, including which queries and addresses it sends, so use Auto only for
  tasks and sources you trust. In MCP mode, `--tools auto` leaves approval to
  the MCP client.
- **Web search providers see your queries.** DuckDuckGo, SearXNG, Brave,
  Tavily, and OpenRouter's hosted search receive the queries sent to them, and
  the sites `web_fetch` reads see a request from your computer. With
  `/tools off` and web search on, the text of each message you send is the
  query.
- **Local model servers are trusted as you configure them.** `/local` and
  `--local` connect to servers on this computer without a key. Pitty does not
  verify which program answers on those ports.
- **Local history is plain text.** Conversations, attachment snapshots, and tool
  output are saved as unencrypted JSON under `$XDG_DATA_HOME/pitty/chats`. Only
  owner-only file permissions protect them.
- **Anyone who can write the audit log can rebuild it.** Each line stores the
  hash of the line before it, so a changed, removed, or reordered middle line
  breaks the check. Someone who can rewrite the whole file can build a new
  valid chain, and removing the newest lines leaves a shorter chain that is
  still valid. `/audit` cannot detect either. The log is kept on the same
  computer and account as Pitty.
- **`/peer` rooms are open to your local network.** A hosted room listens on
  every network interface, announces itself by UDP broadcast on port 47871,
  and carries messages over plain TCP with no password or encryption. Anyone
  who can reach your computer can join while the room is open, up to seven
  guests. Host rooms only on networks you trust. Rooms carry chat text and
  ASCII art, never files, tools, or model access.
- **macOS builds are not notarized.** They are not signed with an Apple
  Developer ID, and macOS may block a downloaded binary.
- **A checksum is not a signature.** The `.sha256` file detects a damaged or
  changed download. It does not prove who published the archive.
