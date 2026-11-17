use super::next_steps;
use crate::{
    ORIGIN, RELEASE, SOURCE,
    html::esc,
    layout::{Nav, OPERATOR, OPERATOR_URL, Page, WORDMARK},
};

/// The folder the demo runs in, as the review prints full paths.
const ROOT: &str = "/Users/you/hello";

/// One line of the terminal: a style class and its text. An empty class is
/// plain base text; an empty text is a blank line.
type Line<'a> = (&'a str, String);

fn plain(text: impl Into<String>) -> Line<'static> {
    ("", text.into())
}

fn styled(class: &'static str, text: impl Into<String>) -> Line<'static> {
    (class, text.into())
}

fn blank() -> Line<'static> {
    ("", String::new())
}

fn render(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|(class, text)| {
            let class = if class.is_empty() {
                "t".to_owned()
            } else {
                format!("t {class}")
            };
            format!(r#"<span class="{class}">{}</span>"#, esc(text))
        })
        .collect()
}

/// The review panel, laid out as Pitty draws it: a title line, the action's
/// detail, then the answer and scroll hints.
fn review(summary: &str, detail: &[Line<'static>]) -> Vec<Line<'static>> {
    let mut lines = vec![
        styled("accent", format!("Review · Pitty · {summary}")),
        blank(),
    ];
    lines.extend(detail.iter().map(|(class, text)| (*class, text.clone())));
    lines.extend([
        blank(),
        plain("Y run once · A approve all · N / Enter decline"),
        styled("muted", "↑↓ / PgUp / PgDn scroll · Ctrl+C stops"),
    ]);
    lines
}

fn you(request: &str) -> Vec<Line<'static>> {
    vec![
        styled("accent", "› You  · /copy message 1"),
        plain(format!("  {request}")),
    ]
}

fn reply(request: &str, text: &str) -> Vec<Line<'static>> {
    let mut lines = you(request);
    lines.extend([
        styled("accent", "● Assistant  · /copy message 2"),
        plain(format!("  {text}")),
    ]);
    lines
}

struct Request {
    id: &'static str,
    prompt: &'static str,
    review: Vec<Line<'static>>,
    review_say: &'static str,
    reply: &'static str,
    yes_say: &'static str,
    no_say: &'static str,
}

fn requests() -> Vec<Request> {
    let main = format!("{ROOT}/src/main.rs");
    let env = format!("{ROOT}/.env");
    vec![
        Request {
            id: "edit",
            prompt: "Add a period to the greeting in src/main.rs.",
            review: review(
                &format!("Edit {main}"),
                &[
                    plain(format!("Edit {main}")),
                    blank(),
                    plain("Exact old text:"),
                    plain(r#"    println!("Hello");"#),
                    blank(),
                    plain("Replacement:"),
                    plain(r#"    println!("Hello.");"#),
                ],
            ),
            review_say: "Pitty shows the exact edit and waits. Y runs it once; N declines.",
            reply: "Wrote 38 bytes to src/main.rs.",
            yes_say: "Y ran the edit once. Pitty wrote 38 bytes to src/main.rs.",
            no_say: "N declined the edit. Nothing was written.",
        },
        Request {
            id: "command",
            prompt: "Run the tests.",
            review: review(
                &format!("Run command in {ROOT}"),
                &[
                    plain(format!("Working directory: {ROOT}")),
                    plain("Timeout: 60s"),
                    blank(),
                    plain("cargo test"),
                    blank(),
                    plain(
                        "Runs with your OS permissions. File and network access are not restricted to connected folders. Only a small set of nonsecret environment variables is inherited.",
                    ),
                ],
            ),
            review_say: "Pitty shows the exact command, where it runs, and its timeout, then waits.",
            reply: "cargo test passed: 3 passed, 0 failed.",
            yes_say: "Y ran cargo test once, without your API keys in its environment.",
            no_say: "N declined the command. Nothing ran.",
        },
        Request {
            id: "env",
            prompt: "What's in .env?",
            review: review(
                &format!("Read {env}"),
                &[plain(format!(
                    "Read lines 1–200 from {env}. This path may contain credentials; approved contents will be sent to the model."
                ))],
            ),
            review_say: "A credential-like file needs its own approval, and the review says where its contents would go.",
            reply: ".env sets PORT=8080 and DATABASE_URL.",
            yes_say: "Y approved one read. The contents of .env went to the model.",
            no_say: "N declined the read. The model never saw .env.",
        },
    ]
}

fn screen(id: &str, answer: &str, lines: &[Line]) -> String {
    format!(
        r#"<div class="screen state" data-request="{id}" data-answer="{answer}">{body}</div>"#,
        body = render(lines),
    )
}

fn caption(id: &str, answer: &str, text: &str) -> String {
    format!(
        r#"<p class="caption state" data-request="{id}" data-answer="{answer}">{}</p>"#,
        esc(text)
    )
}

fn demo() -> String {
    let requests = requests();
    let choices: String = requests
        .iter()
        .enumerate()
        .map(|(index, request)| {
            let checked = if index == 0 { " checked" } else { "" };
            format!(
                r#"<input class="visually-hidden" type="radio" name="request" id="request-{id}" value="{id}"{checked}><label for="request-{id}"><span aria-hidden="true">› </span>{prompt}</label>"#,
                id = request.id,
                prompt = esc(request.prompt),
            )
        })
        .collect();
    let screens: String = requests
        .iter()
        .map(|request| {
            let review = &request.review;
            [
                screen(request.id, "wait", review),
                screen(request.id, "yes", &reply(request.prompt, request.reply)),
                screen(request.id, "no", &you(request.prompt)),
            ]
            .concat()
        })
        .collect();
    let captions: String = requests
        .iter()
        .map(|request| {
            [
                caption(request.id, "wait", request.review_say),
                caption(request.id, "yes", request.yes_say),
                caption(request.id, "no", request.no_say),
            ]
            .concat()
        })
        .collect();
    let statuses = [
        (
            "wait",
            "Review action · Y run once · A approve all · N / Enter decline · Ctrl+C stops",
        ),
        (
            "yes",
            "Ready in 2s · Ctrl+Y copies the last reply · F2 selects text.",
        ),
        ("no", "Action declined."),
    ]
    .iter()
    .map(|(answer, text)| {
        format!(
            r#"<span class="t status" data-answer="{answer}">{}</span>"#,
            esc(text)
        )
    })
    .collect::<String>();
    format!(
        r#"<div class="demo" data-demo>
<div class="requests" role="radiogroup" aria-labelledby="requests-label"><p id="requests-label">Ask Pitty</p>{choices}</div>
<figure class="terminal" aria-labelledby="terminal-caption">
<div class="term-head"><span class="t">{WORDMARK}<span class="model"> / nemotron-3-ultra-550b-a55b:free</span></span><span class="t muted">Folder  ~/hello · 0 connected (/pwd)</span></div>
<div class="screens">{screens}</div>
<div class="term-foot"><span class="t composer">› Message, or / for commands…</span>{statuses}<span class="t footline"><span class="muted">4096 tok · web off · tools ask</span><span class="accent">/ commands · Enter send · Ctrl+C stop</span></span></div>
<figcaption id="terminal-caption" class="visually-hidden">Pitty in a terminal, reviewing the request you chose.</figcaption>
</figure>
<div class="answer" role="radiogroup" aria-label="Answer the review">
<input class="visually-hidden" type="radio" name="answer" id="answer-wait" value="wait" checked>
<input class="visually-hidden" type="radio" name="answer" id="answer-yes" value="yes"><label class="cap cap-yes" for="answer-yes"><kbd>Y</kbd><span>run it once</span></label>
<input class="visually-hidden" type="radio" name="answer" id="answer-no" value="no"><label class="cap cap-no" for="answer-no"><kbd>N</kbd><span>decline</span></label>
<label class="again" for="answer-wait">Show the review again</label>
</div>
<div class="captions">{captions}</div>
<p class="demo-keys">Your keyboard works too: press <kbd>Y</kbd> or <kbd>N</kbd>. In Pitty, Enter and Esc also decline.</p>
<p class="visually-hidden" data-demo-status role="status" aria-live="polite"></p>
</div>"#
    )
}

fn said(lines: &[(&str, &str)]) -> String {
    let rows: String = lines
        .iter()
        .map(|(quote, meaning)| {
            format!(
                r#"<div><dt><samp>{}</samp></dt><dd>{meaning}</dd></div>"#,
                esc(quote)
            )
        })
        .collect();
    format!(r#"<dl class="said">{rows}</dl>"#)
}

fn commands(lines: &[(&str, &str)]) -> String {
    let rows: String = lines
        .iter()
        .map(|(command, meaning)| {
            format!(
                r#"<div><dt><code>{}</code></dt><dd>{meaning}</dd></div>"#,
                esc(command)
            )
        })
        .collect();
    format!(r#"<dl class="said">{rows}</dl>"#)
}

fn structured_data() -> String {
    let same_as = SOURCE
        .map(|url| format!(r#","sameAs":["{url}"]"#))
        .unwrap_or_default();
    format!(
        r#"
<script type="application/ld+json">{{"@context":"https://schema.org","@type":"SoftwareApplication","name":"Pitty","applicationCategory":"DeveloperApplication","operatingSystem":"macOS, Linux","softwareVersion":"{RELEASE}","description":"An AI agent for your terminal that shows every file change and command exactly as it will run, and waits for your answer.","url":"{ORIGIN}/","downloadUrl":"{ORIGIN}/download/","license":"https://opensource.org/licenses/MIT"{same_as},"offers":{{"@type":"Offer","price":"0","priceCurrency":"USD"}},"publisher":{{"@type":"Organization","name":"{OPERATOR}","url":"{OPERATOR_URL}"}}}}</script>"#
    )
}

pub fn page() -> Page {
    let (secondary, facts, open_title, open_intro, open_link) = match SOURCE {
        Some(url) => (
            format!(r#"<a class="text-link" href="{url}">Read the source</a>"#),
            "Free and open source under the MIT License.",
            "Open source, and Rust all the way down.",
            "Read every line before you trust it with your files.",
            format!(
                r#"<p class="section-link"><a class="button button-quiet" href="{url}">{name} on GitHub</a></p>
"#,
                name = url.trim_start_matches("https://github.com/"),
            ),
        ),
        None => (
            r#"<a class="text-link" href="/guide/">Read the guide</a>"#.to_owned(),
            "Free under the MIT License.",
            "Rust all the way down.",
            "How Pitty is built, and what it never does.",
            String::new(),
        ),
    };
    let checks = said(&[
        (
            "Y run once · A approve all · N / Enter decline",
            "Ask is the mode Pitty starts in. File changes, commands, web requests, and reads of credential-like files wait on screen until you answer. Y runs that one action once. A approves it and switches to Auto, where the rest of the task runs without stopping; Shift+Tab switches back at any time.",
        ),
        (
            "File changed after approval; prepare the edit again.",
            "Pitty compares the file with the version you reviewed. If anything changed after you said yes, the write does not land.",
        ),
        (
            "This path may contain credentials; approved contents will be sent to the model.",
            "Files such as <code>.env</code> and SSH keys need their own approval, and search skips them. Over MCP, they are never read.",
        ),
        (
            "Only a small set of nonsecret environment variables is inherited.",
            "Commands the model asks for start from a cleared environment, so <code>OPENROUTER_API_KEY</code>, <code>OPENAI_API_KEY</code>, and <code>PITTY_API_KEY</code> stay out. They still run with your account’s permissions, and Ctrl+C stops the whole process group.",
        ),
    ]);
    let models = commands(&[
        (
            "pitty",
            "OpenRouter, the default: free models, the live catalog, web search, and PDFs. Paste a key with <code>/key</code>, or set <code>OPENROUTER_API_KEY</code>.",
        ),
        (
            "pitty --interface openai --model gpt-4.1-mini",
            "OpenAI. You name the model, and the key is <code>OPENAI_API_KEY</code>.",
        ),
        (
            "pitty --local",
            "A model on this computer. Pitty finds a running Ollama, LM Studio, llama.cpp, vLLM, Jan, or GPT4All server and lists its models; inside Pitty, type <code>/local</code>. Chat stays on your machine and needs no key.",
        ),
        (
            "pitty --interface compatible --base-url http://127.0.0.1:11434/v1 --model llama3.2",
            "Any other endpoint that speaks the same chat protocol. A key is optional.",
        ),
        (
            "/web on",
            "Web search for every model, local ones included. Pitty searches DuckDuckGo with no key, or your SearXNG, Brave, or Tavily, reads pages the model picks, and lists the sources under the answer. Each search shows for review in Ask mode.",
        ),
        (
            "pitty --mcp --directory /path/to/project --tools ask",
            "Or lend the same file and command tools to an MCP client, such as Claude Code or Codex, which brings its own model.",
        ),
    ]);
    let record = said(&[
        (
            "/actions",
            "Lists every action that ran in the conversation, with its output. Stopping a request keeps the work already finished, and <code>/retry</code> never repeats a turn that already changed something.",
        ),
        (
            "Chain intact · 12 records · /Users/you/.local/share/pitty/audit.log",
            "What <code>/audit</code> reports. Each approval, decline, and run adds one line to a log on your computer, and each line stores the SHA-256 of the line before it, so an edited or deleted line breaks the chain. It records decisions, never file contents or output. Someone able to rewrite the whole file could build a new chain; the check cannot see that.",
        ),
    ]);
    let main = format!(
        r#"<main id="main">
<section class="hero" aria-labelledby="title">
<div class="wrap hero-copy">
<h1 id="title"><span>The model proposes.</span> <span>You press&nbsp;Y.</span></h1>
<p class="lede">Pitty is an AI agent for your terminal. By default, every file change and command it wants to make appears exactly as it will run, and waits for your answer.</p>
<p class="actions"><a class="button" href="/download/">Install in one paste</a>{secondary}</p>
<p class="facts">{facts} Version {RELEASE}, for macOS and Linux.</p>
</div>
<div class="stage"><div class="wrap">{demo}</div></div>
</section>
<section class="section wrap" aria-labelledby="checks-title">
<div class="section-head"><h2 id="checks-title">It asks before anything lands.</h2><p>What this build already checks, quoted as Pitty prints it.</p></div>
{checks}
</section>
<section class="section wrap" aria-labelledby="models-title">
<div class="section-head"><h2 id="models-title">Any model, cloud or local. The same review.</h2><p>Chat goes wherever you point it, including a model on your own machine. The review in front of each action stays.</p></div>
{models}
<p class="section-link"><a class="text-link" href="/mcp/">Connect an MCP client</a></p>
</section>
<section class="section wrap" aria-labelledby="record-title">
<div class="section-head"><h2 id="record-title">Every answer is on the record.</h2><p>What ran stays with the conversation, and every decision goes into a log you can check.</p></div>
{record}
</section>
<section class="section wrap open" aria-labelledby="open-title">
<div class="section-head"><h2 id="open-title">{open_title}</h2><p>{open_intro}</p></div>
<ul class="open-list">
<li>The terminal interface is Pitty’s own Rust library with no dependencies: its renderer, editor, Unicode layout, and input decoding.</li>
<li>Command parsing and conversation checks run in a WebAssembly engine inside the program, with no access to files or the network.</li>
<li>No account and no telemetry. Pitty talks to the chat endpoint you choose, and to other Pitty terminals only when you open a <code>/peer</code> room.</li>
<li>Downloads come with SHA-256 checksums, and the installer checks them before anything is unpacked.</li>
<li>This site is built the same way: a Rust program renders every page, and the demo above runs on Rust compiled to WebAssembly.</li>
</ul>
{open_link}</section>
<section class="section wrap ahead" aria-labelledby="ahead-title">
<div class="section-head"><h2 id="ahead-title">Still ahead.</h2><p>What this build does not do yet, in the order we plan to close it.</p></div>
<ol>
<li><span><strong>A notarized Mac build.</strong> Today’s macOS downloads are not signed with an Apple Developer ID.</span></li>
<li><span><strong>Windows.</strong> Pitty runs on macOS and 64-bit Linux. Windows users can try the Linux build in WSL2; that path is untested.</span></li>
<li><span><strong>Streaming replies.</strong> A reply appears when the model finishes it.</span></li>
<li><span><strong>Rules a team can share.</strong> Ask or auto set once for a whole team, with a record everyone can read.</span></li>
</ol>
</section>
{next}
</main>"#,
        demo = demo(),
        next = next_steps("Start here"),
    );
    Page {
        file: "index.html",
        url: Some("/"),
        current: Some(Nav::Product),
        title: "Pitty: the model proposes, you press Y",
        description: if SOURCE.is_some() {
            "Pitty is an AI agent for your terminal. Every file change and command waits on screen, exactly as it will run, until you answer. Free, open source, and written in Rust."
        } else {
            "Pitty is an AI agent for your terminal. Every file change and command waits on screen, exactly as it will run, until you answer. Free, and written in Rust."
        },
        head: structured_data(),
        main,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_quotes_what_pitty_prints() {
        let main = page().main;
        for line in [
            "Review · Pitty · Edit /Users/you/hello/src/main.rs",
            "Exact old text:",
            "Replacement:",
            "Y run once · A approve all · N / Enter decline",
            "↑↓ / PgUp / PgDn scroll · Ctrl+C stops",
            "Review action · Y run once · A approve all · N / Enter decline · Ctrl+C stops",
            "Action declined.",
            "Wrote 38 bytes to src/main.rs.",
            "Message, or / for commands…",
            "This path may contain credentials; approved contents will be sent to the model.",
            "Only a small set of nonsecret environment variables is inherited.",
            "File changed after approval; prepare the edit again.",
            "Chain intact",
        ] {
            assert!(main.contains(&esc(line)), "missing {line}");
        }
    }

    #[test]
    fn every_request_has_a_screen_for_each_answer() {
        let main = page().main;
        for request in ["edit", "command", "env"] {
            assert!(main.contains(&format!(r#"id="request-{request}""#)));
            for answer in ["wait", "yes", "no"] {
                for kind in ["screen", "caption"] {
                    assert!(
                        main.contains(&format!(
                            r#"class="{kind} state" data-request="{request}" data-answer="{answer}""#
                        )),
                        "{kind} {request}/{answer}"
                    );
                }
            }
        }
        for answer in ["wait", "yes", "no"] {
            assert!(main.contains(&format!(r#"id="answer-{answer}""#)));
        }
        assert!(main.contains(r#"id="request-edit" value="edit" checked"#));
        assert!(main.contains(r#"id="answer-wait" value="wait" checked"#));
    }

    #[test]
    fn a_private_repository_is_neither_linked_nor_called_open_source() {
        let page = page();
        if SOURCE.is_none() {
            assert!(!page.main.contains("github.com"));
            assert!(!page.main.to_lowercase().contains("open source"));
            assert!(!page.head.contains("sameAs"));
            assert!(!page.description.contains("open source"));
        }
    }

    #[test]
    fn homepage_keeps_its_limits_in_view() {
        let page = page();
        assert!(page.main.contains("Still ahead."));
        assert!(page.main.contains("not signed with an Apple Developer ID"));
        assert!(page.main.contains("the check cannot see that"));
        assert!(page.main.contains("No account and no telemetry."));
        assert!(page.head.contains(r#""softwareVersion":""#));
        assert!(!page.main.contains("EARLY PREVIEW"));
    }
}
