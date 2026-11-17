//! Browser behavior for pitty.tech, written in Rust and compiled to
//! WebAssembly. Pages work without it: copy buttons stay hidden and the home
//! page demo still answers clicks through plain HTML radio controls. This
//! module adds what HTML cannot do alone: writing to the clipboard, and
//! answering the demo with the Y and N keys on a real keyboard.
mod clipboard;
mod demo;

use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| JsValue::from_str("No document"))?;
    clipboard::start(&document)?;
    demo::start(&document)?;
    Ok(())
}
