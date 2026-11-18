use crate::layout::Page;

/// The demo video. `%DEMO_VIDEO%` and `%DEMO_POSTER%` are replaced with the
/// fingerprinted asset paths when the page is rendered.
pub fn page() -> Page {
    let main = format!(
        r##"<main id="main" class="wrap page watch">
<header class="page-head"><h1>Watch Pitty work.</h1><p class="lede">A real session with Pitty 0.8.0 and a free cloud model, recorded in a terminal. Nothing is staged: the model chose every action you see. Waits for the model are sped up, and the video marks each one.</p></header>
<figure class="watch-video"><video controls playsinline preload="metadata" width="1920" height="1080" poster="%DEMO_POSTER%" aria-describedby="watch-steps"><source src="%DEMO_VIDEO%" type="video/mp4"></video><figcaption><a href="%DEMO_VIDEO%">Download the video (MP4)</a></figcaption></figure>
<section class="watch-steps" aria-labelledby="watch-steps-title"><h2 id="watch-steps-title">What happens in the video</h2><ol id="watch-steps">
<li>Asked to add a <code>--name</code> option and run the program, the model proposes an edit. Pitty shows the exact old text and its replacement, and nothing is written until <kbd>Y</kbd>. The command waits the same way.</li>
<li>With <code>/web duckduckgo</code>, Pitty searches DuckDuckGo itself, with no key. Every search and page fetch waits for a yes, and the answer lists the sources it used.</li>
<li><kbd>Shift</kbd>+<kbd>Tab</kbd> switches to Auto. The model adds a test and runs it without stopping, and every action is still recorded.</li>
<li><code>/audit</code> checks the decision log: each line holds the SHA-256 of the line before it, so an edited or deleted line breaks the chain.</li>
</ol></section>
{next}
</main>"##,
        next = super::next_steps("Try it yourself.")
    );
    Page {
        file: "demo/index.html",
        url: Some("/demo/"),
        current: None,
        title: "Watch Pitty work: a real session",
        description: "A real Pitty 0.8.0 session with a free cloud model: an edit and a command that wait for Y, web search with sources, Auto mode, and the decision log. Waits for the model are sped up and marked.",
        head: String::new(),
        main,
    }
}
