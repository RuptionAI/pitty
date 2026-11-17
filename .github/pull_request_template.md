## What this changes

<!-- Describe the behavior before and after. Link the issue if there is one. -->

## How it was tested

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --locked --workspace --all-targets -- -D warnings`
- [ ] `cargo test --locked --workspace`
- [ ] For site changes: `cargo run --manifest-path site/Cargo.toml -p pitty-site -- check` passes with the regenerated `website/` committed

## Checklist

- [ ] Rust only: no JavaScript, TypeScript, Python, or shell scripts added
- [ ] Tests make no network or inference requests
- [ ] `assets/pitty_core.wasm` is rebuilt if `crates/core` changed
- [ ] `README.md`, `INSTALL.md`, and `CHANGELOG.md` are updated for changes users will notice
- [ ] Changes to approvals, credential handling, commands, MCP, or the audit log are explained above
