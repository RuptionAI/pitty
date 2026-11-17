use super::next_steps;
use crate::layout::Page;

pub fn page() -> Page {
    let main = format!(
        r#"<main id="main" class="not-found">
<header class="wrap page-head"><h1>Nothing here.</h1><p class="lede">This page moved, or never existed. Pick up from one of these.</p></header>
{next}
</main>"#,
        next = next_steps("Where to next"),
    );
    Page {
        file: "404.html",
        url: None,
        current: None,
        title: "Pitty: page not found",
        description: "This page does not exist. Install Pitty, read the guide, or set up MCP.",
        head: String::new(),
        main,
    }
}
