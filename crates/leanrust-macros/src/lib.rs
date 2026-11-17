//! Attribute markers for the LeanRust prover.
//!
//! Item attributes erase to the original item. `leanrust check` and
//! `leanrust prove` read them back from source. Nothing here changes runtime
//! behavior.
//!
//! Field labels are not attribute macros. Put `#[derive(Labels)]` on the struct
//! and `#[label(secret, trusted)]` on the fields. Mark a parameter with
//! `#[input(name, trusted)]` on the function. A parameter is public and tainted
//! unless an `input` attribute says otherwise.

use proc_macro::TokenStream;

fn erase(item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn unhackable(_attr: TokenStream, item: TokenStream) -> TokenStream {
    erase(item)
}

#[proc_macro_attribute]
pub fn ensures(_attr: TokenStream, item: TokenStream) -> TokenStream {
    erase(item)
}

#[proc_macro_attribute]
pub fn noninterference(_attr: TokenStream, item: TokenStream) -> TokenStream {
    erase(item)
}

#[proc_macro_attribute]
pub fn secret(_attr: TokenStream, item: TokenStream) -> TokenStream {
    erase(item)
}

#[proc_macro_attribute]
pub fn public(_attr: TokenStream, item: TokenStream) -> TokenStream {
    erase(item)
}

#[proc_macro_attribute]
pub fn trusted(_attr: TokenStream, item: TokenStream) -> TokenStream {
    erase(item)
}

#[proc_macro_attribute]
pub fn tainted(_attr: TokenStream, item: TokenStream) -> TokenStream {
    erase(item)
}

/// `#[input(account, trusted)]` marks parameter `account` trusted.
///
/// Allowed marks are `secret`, `public`, `trusted`, and `tainted`.
#[proc_macro_attribute]
pub fn input(_attr: TokenStream, item: TokenStream) -> TokenStream {
    erase(item)
}

/// Allows `#[label(...)]` on fields. The derive generates no code.
#[proc_macro_derive(Labels, attributes(label))]
pub fn derive_labels(_input: TokenStream) -> TokenStream {
    TokenStream::new()
}

/// `invariant!("acc == old(acc) + i")` is a proof annotation.
///
/// It erases to `()`. The prover reads the string from source.
#[proc_macro]
pub fn invariant(input: TokenStream) -> TokenStream {
    let _ = input;
    "()".parse().unwrap()
}
