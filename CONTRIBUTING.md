# Contributing to Pitty

## Requirements

- macOS or Linux.
- Rust 1.98 or newer, with the WebAssembly target:

  ```sh
  rustup target add wasm32-unknown-unknown
  ```

## Build and test

Run these from the repository root before you open a pull request. CI runs the
same checks.

```sh
cargo run -p pitty-build -- release
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
```

`release` rebuilds the WebAssembly engine into `assets/pitty_core.wasm`, then
builds the native release binary. `cargo run -p pitty-build -- engine` rebuilds
only the engine. The binary embeds `assets/pitty_core.wasm`, so commit the
rebuilt file when you change `crates/core`.

Build the committed engine with the release toolchain, Rust 1.98.0:

```sh
cargo +1.98.0 run --locked -p pitty-build -- engine
```

The Release packages workflow rebuilds the engine with 1.98.0. If its result
differs from the committed file, the working tree changes and every archive's
`BUILD.txt` says "uncommitted changes; candidate only".

## Website

`website/` is generated, so do not edit it by hand. Change the Rust source in
`site/`, then rebuild:

```sh
cargo run --manifest-path site/Cargo.toml -p pitty-site -- build   # compile Wasm, render website/
cargo run --manifest-path site/Cargo.toml -p pitty-site -- pages   # re-render pages only
cargo run --manifest-path site/Cargo.toml -p pitty-site -- check   # fail if website/ is stale
cargo run --manifest-path site/Cargo.toml -p pitty-site -- serve   # preview at 127.0.0.1:8000
```

Commit the regenerated `website/` together with its source. CI runs `check` and
fails when they differ. It also runs `cargo fmt`, `cargo clippy`, and
`cargo test` with `--manifest-path site/Cargo.toml`. The site is a separate
Cargo workspace, so its dependencies stay out of the CLI's `Cargo.lock`.

## Rules

- **Rust only.** Tools, build and release steps, and tests are written in Rust.
  Browser behavior is Rust compiled to WebAssembly, with plain HTML and CSS.
  Do not add JavaScript, TypeScript, Python, or shell code. That includes build
  scripts and Node packages. The only JavaScript that pitty.tech serves is the
  binding module that wasm-bindgen generates from the Rust browser crate.
- **Tests stay offline.** Tests must not make network or inference requests.
  They must not contact model providers or the internet, or broadcast on the
  local network. A test that needs a socket uses a loopback (`127.0.0.1`)
  fixture. Test files go in a temporary directory, never in your real
  `~/.config/pitty` or `~/.local/share/pitty`.
- **Describe changes to the review boundary.** A change to approvals,
  credential handling, command execution, MCP, or the audit log needs tests and
  an explanation in the pull request.
- **Document what users will notice.** Update `README.md`, and `INSTALL.md`
  when installation changes. Add an entry under `[Unreleased]` in
  `CHANGELOG.md`.

Report security problems privately, as described in [SECURITY.md](SECURITY.md).
