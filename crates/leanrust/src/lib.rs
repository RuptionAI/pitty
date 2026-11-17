//! Markers for programs certified by the LeanRust prover.
//!
//! The attributes erase. Nothing in this crate checks a proof. `leanrust prove`
//! in the LeanRust project reads the Rust source, writes a Lean file, and asks
//! the kernel to accept it. `declassify` and `endorse` are identities; the
//! prover rejects them. This fragment has no certified downgrade. `invariant!`
//! erases to `()`.

pub use leanrust_macros::{
    ensures, input, invariant, noninterference, public, secret, tainted, trusted, unhackable,
    Labels,
};

#[inline(always)]
pub fn declassify<T>(value: T) -> T {
    value
}

#[inline(always)]
pub fn endorse<T>(value: T) -> T {
    value
}
