//! The review demo on the home page.
//!
//! The page is complete without this module: the demo is a pair of radio
//! groups, and CSS shows the screen that matches the checked request and
//! answer. This adds the keyboard Pitty itself uses (Y runs the action once;
//! N or Esc declines), returns to the review when a new request is chosen,
//! and announces each outcome to screen readers.
use std::cell::Cell;
use wasm_bindgen::{JsCast, prelude::*};
use web_sys::{Document, Element, Event, HtmlElement, HtmlInputElement, KeyboardEvent};

thread_local! {
    static PRESS: Cell<u64> = const { Cell::new(0) };
}

/// The answer a key gives, when it gives one. Enter declines only when no
/// control has focus, because a focused link or radio owns Enter.
fn answer_for(key: &str, focus_on_page: bool) -> Option<&'static str> {
    match key {
        "y" | "Y" => Some("yes"),
        "n" | "N" | "Escape" => Some("no"),
        "Enter" if focus_on_page => Some("no"),
        _ => None,
    }
}

fn focus_on_page(event: &Event) -> bool {
    event
        .target()
        .and_then(|target| target.dyn_into::<Element>().ok())
        .is_none_or(|target| matches!(target.tag_name().as_str(), "BODY" | "HTML"))
}

fn checked_value(demo: &Element, name: &str) -> Option<String> {
    demo.query_selector(&format!("input[name=\"{name}\"]:checked"))
        .ok()
        .flatten()?
        .dyn_into::<HtmlInputElement>()
        .ok()
        .map(|input| input.value())
}

fn input(document: &Document, id: &str) -> Option<HtmlInputElement> {
    document.get_element_by_id(id)?.dyn_into().ok()
}

/// Read the visible caption into the live region, for screen readers.
fn announce(demo: &Element) {
    let (Some(request), Some(answer)) = (
        checked_value(demo, "request"),
        checked_value(demo, "answer"),
    ) else {
        return;
    };
    let selector = format!(".caption[data-request=\"{request}\"][data-answer=\"{answer}\"]");
    let said = demo
        .query_selector(&selector)
        .ok()
        .flatten()
        .and_then(|caption| caption.text_content());
    if let (Some(said), Ok(Some(status))) = (said, demo.query_selector("[data-demo-status]")) {
        status.set_text_content(Some(&said));
    }
}

/// Briefly show a key cap pressed, as a physical key press would.
fn press(document: &Document, answer: &str) {
    let Some(cap) = document
        .query_selector(&format!("label[for=\"answer-{answer}\"]"))
        .ok()
        .flatten()
    else {
        return;
    };
    let serial = PRESS.with(|press| {
        let next = press.get().wrapping_add(1);
        press.set(next);
        next
    });
    let _ = cap.class_list().add_1("is-pressed");
    let Some(window) = web_sys::window() else {
        return;
    };
    let release = Closure::once_into_js(move || {
        if PRESS.with(|press| press.get() == serial) {
            let _ = cap.class_list().remove_1("is-pressed");
        }
    });
    let _ =
        window.set_timeout_with_callback_and_timeout_and_arguments_0(release.unchecked_ref(), 160);
}

/// Keys typed into a field belong to that field.
fn typing(event: &Event) -> bool {
    let Some(target) = event
        .target()
        .and_then(|target| target.dyn_into::<Element>().ok())
    else {
        return false;
    };
    if let Some(input) = target.dyn_ref::<HtmlInputElement>() {
        return input.type_() != "radio" && input.type_() != "checkbox";
    }
    if matches!(target.tag_name().as_str(), "TEXTAREA" | "SELECT") {
        return true;
    }
    target
        .dyn_ref::<HtmlElement>()
        .is_some_and(HtmlElement::is_content_editable)
}

/// The demo answers keys only while some of it is on screen.
fn visible(demo: &Element) -> bool {
    let rect = demo.get_bounding_client_rect();
    let height = web_sys::window()
        .and_then(|window| window.inner_height().ok())
        .and_then(|height| height.as_f64())
        .unwrap_or(0.0);
    rect.bottom() > 0.0 && rect.top() < height
}

pub fn start(document: &Document) -> Result<(), JsValue> {
    let Some(demo) = document.query_selector("[data-demo]")? else {
        return Ok(());
    };

    // Choosing another request brings back its review, as Pitty would.
    let changes = {
        let demo = demo.clone();
        let document = document.clone();
        Closure::<dyn FnMut(Event)>::new(move |event: Event| {
            let Some(target) = event
                .target()
                .and_then(|target| target.dyn_into::<HtmlInputElement>().ok())
            else {
                return;
            };
            if target.name() == "request"
                && let Some(wait) = input(&document, "answer-wait")
            {
                wait.set_checked(true);
            }
            announce(&demo);
        })
    };
    demo.add_event_listener_with_callback("change", changes.as_ref().unchecked_ref())?;
    changes.forget();

    let keys = {
        let demo = demo.clone();
        let document = document.clone();
        Closure::<dyn FnMut(KeyboardEvent)>::new(move |event: KeyboardEvent| {
            if event.default_prevented()
                || event.ctrl_key()
                || event.meta_key()
                || event.alt_key()
                || event.repeat()
                || typing(&event)
                || !visible(&demo)
            {
                return;
            }
            let Some(answer) = answer_for(&event.key(), focus_on_page(&event)) else {
                return;
            };
            let Some(choice) = input(&document, &format!("answer-{answer}")) else {
                return;
            };
            event.prevent_default();
            choice.set_checked(true);
            press(&document, answer);
            announce(&demo);
        })
    };
    document.add_event_listener_with_callback("keydown", keys.as_ref().unchecked_ref())?;
    keys.forget();

    demo.set_attribute("data-demo-ready", "true")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::answer_for;

    #[test]
    fn keys_match_pittys_review() {
        assert_eq!(answer_for("y", false), Some("yes"));
        assert_eq!(answer_for("Y", false), Some("yes"));
        assert_eq!(answer_for("n", false), Some("no"));
        assert_eq!(answer_for("Escape", false), Some("no"));
        assert_eq!(answer_for("Enter", true), Some("no"));
        assert_eq!(answer_for("Enter", false), None);
        assert_eq!(answer_for("x", true), None);
    }
}
