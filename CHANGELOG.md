# Changelog

All notable changes to Pitty are recorded here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [0.7.0] - 2026-11-16

This covers everything since 0.6.0. A 0.6.1 build was offered on pitty.tech
for Apple Silicon Macs only. It has no entry of its own, and 0.7.0 supersedes
it.

### Added

- `--interface openrouter|openai|compatible` and `/interface` choose where chat
  requests go. OpenRouter stays the default. `openai` uses the OpenAI API and
  `OPENAI_API_KEY`. `compatible` needs a base URL, such as
  `--base-url http://127.0.0.1:11434/v1` or `/interface compatible URL`. It
  reads `PITTY_API_KEY` when one is set, and sends no `Authorization` header
  when it is not. Changing the interface starts a new conversation. `/status`
  shows the current interface.
- A local decision log at `$XDG_DATA_HOME/pitty/audit.log` (default
  `~/.local/share/pitty/audit.log`). Each decision on a file change, command,
  or credential-like read adds one JSON line: approved, declined, run without
  review under `/tools auto` or MCP `--tools auto`, or refused over MCP. After
  an approved or automatic action, and after each `/run` or `/exec`, another
  line records whether it ran or failed. Each line holds the time, the actor
  (`user`, `session`, or `mcp`), the decision, the tool, and the path or
  command, plus the SHA-256 of the previous line. File contents and command
  output are not recorded, and read-only tool calls that need no review are not
  logged.
- The log file is owner-only (0600). A path or command is cut to 240
  characters, and an API key of 8 bytes or more in it is replaced with
  `[REDACTED]`. Pitty stops adding lines once the file reaches 32 MiB.
- `/audit` checks the log's hash chain. It shows the record count, the first
  record that breaks the chain, the last 12 records, and the last failed write.
- `/peer` adds chat rooms between Pitty terminals. `/peer` alone lists rooms
  nearby. `/peer host` opens a numbered room (a random number from 1 to 99, or
  `/peer host NUMBER` from 1 to 999). `/peer join ROOM` joins by number, on this
  computer or on the local network, or by `host:port`. A room holds the host and
  up to seven guests and ends when the host leaves.
- Peer rooms carry text messages and ASCII art. `/peer art` draws your own, and
  `/peer art cat|ship|crab|spark` shares a built-in drawing. `/peer pick N`
  selects a drawing, and `/peer move DIR` or the arrow keys (with an empty
  draft) move it. `/peer leave` leaves the room, and Esc hides it. Messages and
  drawings are kept only in memory. Rooms are announced by UDP broadcast on
  port 47871 and carried over plain TCP, with no encryption and no password.
- Ctrl+J inserts a new line. Terminals that report modified keys (the kitty
  keyboard protocol or xterm `modifyOtherKeys`) also insert one for Shift+Enter
  and Alt+Enter.
- Ctrl+K deletes to the end of the line, and Ctrl+L redraws the whole screen.
- At startup Pitty asks the terminal for its background color, waits at most
  200 ms, and falls back to `COLORFGBG`. On a dark background, `/theme auto`
  uses dark shading for the composer, code blocks, and selections, and syntax
  colors follow the background.
- An empty conversation lists next steps. `/key` comes first while no key is
  configured.
- Sending a first message without a key opens the masked key field and keeps
  the message. After the key is saved, Enter sends it.
- `read_file` says when a file has CRLF or mixed line endings. In a CRLF file,
  `edit_file` matches an LF fragment against the CRLF text and writes new lines
  with CRLF.
- `tools/build` (`cargo run -p pitty-build -- engine|release|install`) builds
  the WebAssembly engine and the release binary in Rust. `install` packages an
  archive for this computer, installs `~/.local/bin/pitty`, and re-renders and
  checks `website/`. Each archive goes in a new subdirectory of
  `PITTY_ARCHIVE_DIR`, or of `dist` in the Cargo target directory when that
  variable is unset.
- `crates/leanrust` and `crates/leanrust-macros` provide attribute markers with
  no runtime effect. `src/audit_cert.rs` uses them to mark the audit log's
  admission, redaction, and length rules for the LeanRust prover, an external
  tool that is not part of this repository.

### Changed

- The default model is now `nvidia/nemotron-3-ultra-550b-a55b:free`.
- OpenRouter request fields (web search, PDF parsing, and plugin settings) are
  sent only to OpenRouter, and the `:online` suffix is removed only there.
  `--web` and `/web on` are refused on the other interfaces, and switching to
  one turns web search off.
- Model ids are checked for the current interface. OpenRouter ids must contain
  `/`. The OpenAI and compatible interfaces need `--model` or `/model` before
  the first message, because the default model id belongs to OpenRouter.
- `/key` checks the key format for the current interface: `sk-or-` for
  OpenRouter, `sk-` for OpenAI, or any single token for a compatible endpoint.
  It replaces only that interface's line in the key file and keeps every other
  entry. If the key file is a symlink, the key is written to the file it points
  to.
- When no key is found, the error message says how to add one.
- Commands that the model requests, and `/run` and `/exec` commands, start in a
  new session with no controlling terminal and with `GIT_TERMINAL_PROMPT=0`.
  Password, passphrase, and host-key prompts now fail at once. Before, they drew
  over Pitty or waited until the timeout.
- `write_file` gives a new file the permissions your umask allows (often 0644)
  instead of 0600. A replaced file keeps its mode.
- `list_files` skips and counts entries that disappear during a listing instead
  of failing.
- In one-shot `--json` output with tools, `truncated` is true only when the
  model reached the output-token cap. The message explains other early stops,
  such as the 16-request limit.
- Starting a new conversation keeps an unsent draft and staged files.
- `/export` accepts `~`, quoted paths, and escaped spaces, as `/attach` does.
- The header draws each letter of PITTY in its own rainbow color. It shows the
  folder with `~` for your home directory and shortens a long path in the
  middle to keep it on one line.
- `/run` and `/exec` output is labeled "Command" instead of "Assistant" in the
  transcript and in `/export`.
- Text attachments may contain form feeds, vertical tabs, and ANSI color codes.
  Terminal escape sequences, such as color codes, are removed before sending.
  `.pcm` files are accepted as 16-bit PCM audio.
- Pitty ignores a relative `HOME`, `XDG_CONFIG_HOME`, or `XDG_DATA_HOME` when it
  places its settings, keys, history, and audit log. Pitty changes into the
  project folder at startup, so a relative path would put those files inside
  the project.
- Emoji such as ✅, ❌, ⭐, and ⚡ are measured as two columns wide. Tag
  characters and U+FEFF are measured as zero columns wide.
- The pitty.tech website is generated by a Rust site builder in `site/`, a
  separate Cargo workspace. `website/` is its output, and CI fails when
  `website/` does not match the source.
- pitty.tech was redesigned around Pitty's own terminal colors. The home page
  demo replays the review for three requests and answers Y or N from the
  keyboard through the Rust browser crate (`site/browser`, formerly
  `site/clipboard`); it also works without WebAssembly. The site self-hosts
  Atkinson Hyperlegible Next and Mono, supports dark mode, and replaces the
  terminal recordings with that live demo.
- CI and the Release packages workflow build with `pitty-build`. The license
  fallback list moved to `tools/release/license-fallbacks.json`.
- The package metadata lists the repository as `github.com/RuptionAI/pitty` and
  the homepage as `pitty.tech`.

### Fixed

- `/actions` typed while a request is running opens the action list. Before, it
  opened `/agents`.
- When a tool-enabled request fails before any local action runs, the draft and
  staged files are restored and no empty turn is recorded. Causes include a
  missing key, a network error, a model without tool support, or Ctrl+C before
  a tool ran.
- N and Enter answer a review only after it has been on screen for 300 ms. Y
  already worked this way. Esc still declines at once.
- `/delete` removes the saved copy of a conversation even after `/history off`
  in the same session.
- `/copy` no longer replaces the clipboard with empty text. `/copy draft` says
  when nothing has been sent yet.
- A title set with `/rename` before the first message or `/run` is kept.
- `/copy code N` uses the same numbers as the screen when a message contains
  control characters.
- Multi-line pastes keep their line breaks in terminals that separate pasted
  lines with CR.
- A paste that would make the draft longer than 1 MB is reported instead of
  dropped without notice. A paste that looks like a file path stays in the
  draft as text when the dropped-file check cannot start.
- Ctrl+K deletes only to the end of the current line. Before, it deleted the
  rest of the draft. At the end of a line it joins the next line.
- F1, F3, F4, keypad keys, Linux console function keys, and Alt with a
  non-ASCII character no longer type stray characters. The terminal's reply to
  the startup color query is never typed into the draft. Home and End also work
  in terminals that send `ESC[1~`, `ESC[4~`, `ESC[7~`, or `ESC[8~`.
- A panic on the interface thread restores the terminal before printing, so the
  message stays visible. A signal that interrupts the terminal read is no longer
  treated as the end of input.
- `write_file` can create files on volumes without hard links, such as FAT,
  exFAT, and many SMB shares. It uses an exclusive rename or an exclusive
  create, so an existing file is never replaced. Writes skip `chmod` when the
  mode already matches, for volumes that refuse it.
- MCP mode no longer switches its output to nonblocking mode. When stdin and
  stdout share one terminal or socket, as in a manual launch from a shell, that
  made reads fail. A client that closes its input still receives every reply,
  including replies for operations that were still running.
- Closing the terminal (SIGHUP), SIGTERM, or an external SIGINT now ends the
  terminal interface through its normal exit. Requests are canceled, command
  process groups are stopped and reaped, and the terminal is restored. One-shot
  tool mode now also handles SIGTERM and SIGHUP, and MCP mode also handles
  SIGHUP. Before, closing the terminal could leave a command running.

### Security

- MCP mode declines credential-like files in every `--tools` mode. These
  include `.env`, `.ssh`, private keys, and secrets files. In 0.6.0,
  `--tools auto` read them without asking, because the MCP server cannot ask
  its user for approval.
- Credential detection also covers names that some volumes resolve to a
  credential file. Examples include names with invisible Unicode characters,
  names with trailing dots or spaces, and case-folded spellings such as
  `ſecrets.json` (with a long s). Reading such a file now needs approval.
- The API key is replaced with `[REDACTED]` in `/run` and `/exec` output before
  that output is saved or sent with later turns.

### Removed

- `scripts/build.sh` and `scripts/build-website-ui.sh`, replaced by
  `pitty-build` and the Rust site builder.
- The `website-ui/` crate, now part of `site/`.
- The separate `website/install.html` page. `/install.html` redirects to
  `/download/`.

## [0.6.0] - 2026-11-09

### Added

- The model can use local tools: `list_files`, `read_file`, `search_files`,
  `write_file`, `edit_file`, and `run_command`. They work inside the working
  folder (`--directory`/`-C`) and connected folders.
- `/tools ask` is the default. Each file change, command, and credential-like
  read is shown for review. Y runs it once, and N, Enter, or Esc declines it.
  `/tools auto` runs tools without review for the session, and `/tools off`
  turns them off. The CLI option is `--tools ask|auto|off`.
- File changes are checked against the version that was read before approval.
  File tools do not follow symlinks.
- Commands run through `/bin/sh -c` with a cleared environment that has no API
  key variables. They get no stdin, bounded output, and a timeout. Ctrl+C stops
  the command's process group.
- A tool-enabled turn makes at most 16 model requests. A batch of more than
  eight tool calls is refused.
- `/run` and `/exec` run a command you type, without a model request. `/actions`
  lists completed actions and their output.
- MCP server over stdio: `pitty --mcp --directory PATH`. `--tools ask` exposes
  read-only tools. `--tools auto` also exposes file changes and commands.
- One-shot mode can use tools, with approvals asked on the terminal. With tools,
  `--json` prints a `ChatReply` object.
- pitty.tech gained download, guide, and MCP pages. Its copy buttons are written
  in Rust and compiled to WebAssembly.

### Changed

- `/agent` tasks can use local tools and share the review queue.
- `/retry` does not repeat a turn that already performed local actions.
- History also saves tool traces and the list of completed actions.

## [0.5.1] - 2026-11-09

The first version offered as download archives.

### Added

- Drag over chat text to select it, then click Copy selection or press Ctrl+Y.
  Selected code keeps its tabs and line breaks and leaves out display borders.
- The Release packages workflow, run by hand, builds archives for Apple Silicon
  Macs, Intel Macs, and x64 Linux. Each archive holds the binary, `LICENSE`,
  `INSTALL.md`, `BUILD.txt` (version, target, commit, source state, and
  compiler), and third-party notices, with a `.sha256` file beside it.
- `INSTALL.md`, plus a pitty.tech landing page and install page.

### Changed

- Ctrl+Y copies the selection, or the latest reply when nothing is selected. Esc
  clears a selection before doing anything else.

## [0.5.0] - 2026-11-08

### Added

- Named background tasks: `/agent NAME [--model ID] TASK`, `/agents`,
  `/agent open ID`, and `/agent stop ID|all`. At most four inference requests
  run at once, counting the main chat.
- `/name` and `/team` display labels, stored in
  `$XDG_CONFIG_HOME/pitty/profile.json`. `/pwd` shows the full working folder
  path and every connected folder, and the header shows the working folder.
- `/select` and F2 switch to the terminal's own text selection. Ctrl+Y copies
  the latest reply.
- Each response shows how long it has been waiting.

### Changed

- The composer and code blocks have rounded borders.
- Transcript layout is cached across draft edits and scrolling, and decoding of
  fragmented pastes is bounded.
- Attachments are read in a background worker and checked for file identity,
  size, and modification races. Images, PDFs, and WAV, FLAC, OGG, and AIFF audio
  are checked for their format signature. The same snapshot is not staged twice.
- Folder connection updates take an OS lock and merge the latest saved list.

## [0.4.1] - 2026-11-08

### Added

- Connected folders: `/connect`, `/folders`, `/files`, and `/disconnect`. The
  browser lists up to 2,000 entries of one folder at a time, without recursion,
  and stages only the files you choose. Connections are saved in
  `$XDG_CONFIG_HOME/pitty/folders.json` with owner-only permissions.
- The mouse wheel and trackpad scroll the transcript. `/mouse off` and
  `--no-mouse` return to the terminal's own selection, and `/mouse on` turns
  wheel scrolling back on.

### Changed

- Page Up and Page Down scroll one page, also while you edit a draft. Replies
  that arrive while you read older messages keep your place.
- The footer no longer shows the model id.

### Fixed

- Leaving the file browser restores the composer hints.

## [0.4.0] - 2026-11-07

### Added

- Attachments: `/attach`, `/attachments`, and `/detach`, or drag files into the
  terminal. Supported files are images, PDFs, audio, video, and UTF-8 text. Size
  limits can be changed with `PITTY_MAX_FILE_MIB`, `PITTY_MAX_TEXT_MIB`, and
  `PITTY_MAX_CHAT_MIB`.
- Model-directed web search through OpenRouter's `openrouter:web_search` tool,
  using Exa with one search and three results per request. Turn it on with
  `/web on` or `--web`. It is off at startup.
- `/model` opens a searchable model picker.
- `/copy` opens a searchable list of messages and code blocks. `/copy last`,
  `/copy message N`, and `/copy code N` copy one item directly.
- Code blocks show a language label and syntax colors.

### Changed

- With no request running, Ctrl+C clears the draft and staged files. Up and Down
  scroll the transcript when the composer is empty.
- The startup mark fades into the transcript, and the composer stays in place
  while the conversation scrolls.
- History stores attachment snapshots. Startup loads up to 100 recent history
  files.

## [0.3.0] - 2026-11-07

The first version.

### Added

- Terminal chat with OpenRouter chat models, built on `pitty-ui`. This terminal
  library uses only the Rust standard library for its renderer, editor,
  Unicode-aware layout, input decoder, and bracketed paste.
- Slash-command parsing and conversation checks are written in Rust and
  compiled to WebAssembly (`pitty-core`). The module is embedded in the binary
  and runs in Wasmi with no filesystem or network imports.
- Masked `/key` entry. The key is read from `--key-file`, `OPENROUTER_API_KEY`,
  or `$XDG_CONFIG_HOME/pitty/.env`.
- `/models` and `/model` use the live OpenRouter catalog, with prices and
  context sizes.
- Conversation commands, including `/new`, `/chats`, `/open`, `/rename`,
  `/delete`, `/clear`, `/copy`, `/export`, `/search`, and `/retry`.
- One-shot mode: `pitty "PROMPT"`, with input on stdin, `--json`, and
  `--no-history`.
- History is saved as JSON under `$XDG_DATA_HOME/pitty/chats`.
- ANSI and OSC control sequences are removed from model output before it is
  drawn.
