# Demo recorder

Records the video on [pitty.tech/demo](https://pitty.tech/demo/): real Pitty
sessions in a pseudo-terminal, with the model choosing every action, drawn into
captioned 1080p frames and encoded as an MP4. It runs on macOS and needs Google
Chrome (for drawing frames) and `ffmpeg`.

Each scene starts Pitty in its own `HOME`, so the demo's settings, chats, and
decision log never mix with yours. Reviews stay on screen long enough to read,
then the recorder answers **Y**. Waits for the model are compressed when
rendering, and every shortened stretch carries a "Sped up N×" badge.

```sh
cargo build --release --locked   # in the repository root: the pitty binary to record

export DEMO_ROOT=/path/with/space/for/takes   # takes/ and render/ go here
export DEMO_HOME=/Users/Shared/demo           # Pitty's HOME; its paths appear on screen
export PITTY_BIN=/path/to/target/release/pitty
export OPENROUTER_API_KEY=...                 # passed to Pitty only

cargo run --release --locked --manifest-path site/recorder/Cargo.toml -- record edit web auto audit
cargo run --release --locked --manifest-path site/recorder/Cargo.toml -- render edit web auto audit
```

Choose a `DEMO_HOME` whose path says nothing private: reviews print absolute
paths. The scenes use Pitty's default free model and `/web duckduckgo`, which
needs no key and costs nothing; `/web on` with OpenRouter's hosted search would
add charges. `record edit` resets the demo project; later scenes build on it.
Re-record any scene on its own, then render them all again.

To publish, copy `render/pitty-demo.mp4` to `site/static/media/pitty-demo.mp4`,
save a representative frame from `render/` as
`site/static/media/pitty-demo-poster.png`, and rebuild the site.
