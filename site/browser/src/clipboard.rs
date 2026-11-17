//! Copy buttons. The displayed code is the single source for copied text, and
//! each button stays hidden until this module has wired it.
use js_sys::{Function, Promise, Reflect};
use std::cell::Cell;
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{Document, Element, HtmlButtonElement};

thread_local! {
    static LATEST_COPY: Cell<u64> = const { Cell::new(0) };
}

fn announce(status: Option<&Element>, message: &str, state: &str) {
    if let Some(status) = status {
        status.set_text_content(Some(message));
        let _ = status.set_attribute("data-state", state);
    }
}

fn matching_status(document: &Document, id: &str) -> Option<Element> {
    let nodes = document.query_selector_all("[data-copy-status]").ok()?;
    (0..nodes.length())
        .filter_map(|i| nodes.item(i)?.dyn_into::<Element>().ok())
        .find(|element| element.get_attribute("data-copy-status").as_deref() == Some(id))
}

fn select_text(document: &Document, target: &Element) -> Result<(), JsValue> {
    let range = document.create_range()?;
    range.select_node_contents(target)?;
    let selection = document
        .get_selection()?
        .ok_or_else(|| JsValue::from_str("Text selection is unavailable."))?;
    selection.remove_all_ranges()?;
    selection.add_range(&range)
}

fn fallback(document: &Document, target: &Element, status: Option<&Element>, latest: bool) {
    let selected = latest && select_text(document, target).is_ok();
    announce(
        status,
        if selected {
            "Text selected. Press ⌘C on Mac or Ctrl+C to copy, or use your device’s Copy menu."
        } else {
            "Clipboard access was blocked. Select the code and use your device’s Copy command."
        },
        "fallback",
    );
}

// Calling writeText synchronously inside the click handler preserves the browser's
// transient user activation. Reflect also catches a missing API or synchronous
// exception; async permission failures are handled when its promise settles.
fn begin_copy(text: &str) -> Result<Promise, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window"))?;
    let navigator = window.navigator();
    let clipboard = Reflect::get(navigator.as_ref(), &JsValue::from_str("clipboard"))?;
    let write =
        Reflect::get(&clipboard, &JsValue::from_str("writeText"))?.dyn_into::<Function>()?;
    write
        .call1(&clipboard, &JsValue::from_str(text))?
        .dyn_into::<Promise>()
}

/// Restore a button's label after a short confirmation, unless it was clicked
/// again in the meantime (each click stamps a newer serial on the button).
fn restore_label_later(button: HtmlButtonElement, label: String, serial: u64) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let marker = serial.to_string();
    let callback = Closure::once_into_js(move || {
        if button.get_attribute("data-copy-serial").as_deref() == Some(marker.as_str()) {
            button.set_text_content(Some(&label));
        }
    });
    let _ = window
        .set_timeout_with_callback_and_timeout_and_arguments_0(callback.unchecked_ref(), 2000);
}

pub fn start(document: &Document) -> Result<(), JsValue> {
    let buttons = document.query_selector_all("button[data-copy]")?;
    for index in 0..buttons.length() {
        let Some(button) = buttons
            .item(index)
            .and_then(|node| node.dyn_into::<HtmlButtonElement>().ok())
        else {
            continue;
        };
        let Some(id) = button.get_attribute("data-copy") else {
            continue;
        };
        let Some(target) = document.get_element_by_id(&id) else {
            continue;
        };
        let status = matching_status(document, &id);
        let label = button
            .text_content()
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| "Copy".into());
        button.set_type("button");
        let _ = button.set_attribute("aria-controls", &id);
        let listener_button = button.clone();
        let listener_document = document.clone();
        let listener = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
            let text = target.text_content().unwrap_or_default();
            if text.is_empty() {
                announce(status.as_ref(), "There is no code to copy.", "fallback");
                return;
            }
            let serial = LATEST_COPY.with(|latest| {
                let serial = latest.get().wrapping_add(1);
                latest.set(serial);
                serial
            });
            let _ = listener_button.set_attribute("data-copy-serial", &serial.to_string());
            let promise = begin_copy(&text);
            let Ok(promise) = promise else {
                listener_button.set_text_content(Some(&label));
                fallback(&listener_document, &target, status.as_ref(), true);
                return;
            };
            listener_button.set_disabled(true);
            listener_button.set_text_content(Some("Copying…"));
            announce(status.as_ref(), "Copying…", "pending");
            let button = listener_button.clone();
            let document = listener_document.clone();
            let target = target.clone();
            let status = status.clone();
            let label = label.clone();
            spawn_local(async move {
                if JsFuture::from(promise).await.is_ok() {
                    button.set_text_content(Some("Copied!"));
                    announce(status.as_ref(), "Copied to clipboard.", "success");
                    restore_label_later(button.clone(), label.clone(), serial);
                } else {
                    button.set_text_content(Some(&label));
                    let latest = LATEST_COPY.with(|latest| latest.get() == serial);
                    fallback(&document, &target, status.as_ref(), latest);
                }
                button.set_disabled(false);
            });
        });
        button.add_event_listener_with_callback("click", listener.as_ref().unchecked_ref())?;
        // Each listener has page lifetime. The only timer restores a label once.
        listener.forget();
        button.set_disabled(false);
        let _ = button.set_attribute("data-copy-ready", "true");
    }
    Ok(())
}
