# Install Pitty

Pitty runs locally in a macOS or Linux terminal. Prebuilt downloads include the WebAssembly engine; Rust, Node.js, and a browser are not required. macOS downloads are not signed with an Apple Developer ID and are not notarized.

## Choose a download

| Your computer | Archive suffix |
| --- | --- |
| Apple Silicon Mac (M1 or newer) | `aarch64-apple-darwin.tar.gz` |
| Intel Mac | `x86_64-apple-darwin.tar.gz` |
| 64-bit Intel/AMD Linux, Ubuntu 22.04 or newer / glibc 2.35 or newer | `x86_64-unknown-linux-gnu.tar.gz` |

macOS packages target macOS 11 or later and are tested on macOS 15. Linux packages are tested on Ubuntu 22.04. Native Windows and Linux ARM packages are not included; Windows users can try the Linux build inside an x64 WSL2 Ubuntu environment; WSL2 has not been tested for this release. A terminal with `stty` is required. Linux clipboard support also needs `wl-copy` (Wayland) or `xclip` (X11).

## Verify and install

Download the archive and its matching `.sha256` file from the same release at https://pitty.tech/download/. In that download directory, verify the checksum before extracting:

```sh
# macOS
shasum -a 256 -c pitty-VERSION-TARGET.tar.gz.sha256

# Linux
sha256sum -c pitty-VERSION-TARGET.tar.gz.sha256
```

Replace `VERSION-TARGET` with the downloaded filename. After it reports `OK`:

```sh
tar -xzf pitty-VERSION-TARGET.tar.gz
cd pitty-VERSION-TARGET
mkdir -p "$HOME/.local/bin"
install -m 755 pitty "$HOME/.local/bin/pitty"
export PATH="$HOME/.local/bin:$PATH"
pitty --version
pitty
```

Add `export PATH="$HOME/.local/bin:$PATH"` to your shell configuration (`~/.zshrc` or `~/.bashrc`) to keep the command available in new terminals. Installation and updates use the same steps. The included `BUILD.txt` identifies the version and source revision; the checksum detects a damaged or changed download, but is not a publisher signature.

These macOS binaries are not signed with an Apple Developer ID or notarized. macOS may block a binary downloaded in a browser; the one-paste installer at https://pitty.tech/download/ downloads with `curl`, which macOS does not quarantine. Pitty does not change Gatekeeper settings. A signed and notarized release is planned.

## Start a conversation

1. Run `pitty`.
2. Type `/key`, press Enter, and paste your OpenRouter API key into the masked field.
3. Use `/models free` to browse models and Enter to choose one.
4. Type a message and press Enter. Without a saved key, Pitty asks for it first and keeps your message. `/help` lists commands.

OpenRouter is the default. `pitty --interface openai --model gpt-4.1-mini` uses `OPENAI_API_KEY`. `pitty --interface compatible --base-url http://127.0.0.1:11434/v1 --model llama3.2` uses `PITTY_API_KEY` when the endpoint asks for one, and sends no Authorization header when it does not. Web search and PDFs stay on OpenRouter. `/models free` is the OpenRouter catalog.

Bring your own key for the interface you chose. Model requests may incur provider charges. On OpenRouter, set a spending limit on the key. Free-model availability and provider limits can change.

Enter sends a message; **Ctrl+J** inserts a new line. Drag over chat text and click **Copy selection**. **Ctrl+Y** copies the selected text, or the last reply when nothing is selected. **F2** enables your terminal's native selection. Ctrl+C stops a request or clears the draft; a subsequent Ctrl+C asks before closing.

History is stored locally by default. Use `pitty --no-history` for a session without saved history. API keys are stored locally with owner-only permissions, not in chat history. Submitting a message sends staged attachments plus the selected conversation to the chat endpoint you chose. Follow-up messages and named agent tasks also include attachments already present in that conversation. `/connect` only remembers a folder; it does not upload the folder. With local tools enabled, the model can read working-folder and connected-folder files, edit them, and run commands. File contents and tool results become part of the model conversation.

## Files, commands, and MCP

Choose your working folder when launching:

```sh
pitty --directory /path/to/project
```

Use `/run cargo test` or `/exec python3 script.py` to run a command directly. These commands do not need an API key. `/actions` shows completed actions and output. Ctrl+C stops running commands and their child processes.

The default `/tools ask` mode previews model-requested file changes, commands, and sensitive reads. Press **Y** to approve an action; **N**, **Enter**, or **Esc** declines. `/tools auto` runs tools without review for new requests in this session; `/tools off` disables model tools. Commands run with your account permissions. Use a model that supports function calling when tools are enabled.

To expose the same tools to an MCP client, configure a local stdio server with this command:

```sh
pitty --mcp --directory /path/to/project --tools auto
```

This mode uses the MCP client's model and approval controls and needs no OpenRouter key. The default `--tools ask` mode exposes read-only MCP tools; `--tools auto` enables file changes and command execution. Use an absolute executable path in the client configuration if its PATH does not include Pitty.

## Remove the command

Remove only the executable you installed:

```sh
rm "$HOME/.local/bin/pitty"
```

This leaves your key, preferences, and conversation history intact. Use `/status` before uninstalling to find your history location. Keep these files private when asking for support; never post an API key or an unreviewed conversation export.
