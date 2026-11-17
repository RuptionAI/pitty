//! Decision rules in the `leanrust-core-1` fragment.
//!
//! `admit`, `redact_secret`, and `kept_chars` are the certified functions.
//! `leanrust prove` in the LeanRust project checks this file with the kernel.
//! The names stored in the log, the hash, the disk, and the rest of Pitty are
//! outside that proof. The model uses mathematical integers. These three
//! functions do not add, so they do not meet a wrapping `u64`.
//!
//! From the LeanRust project:
//!
//! `cargo run -p leanrust-cli -- prove path/to/src/audit_cert.rs`

use leanrust::{ensures, input, noninterference, trusted, unhackable};

/// `1` when `actor` is 1..=3 and `decision` is 1..=6. Otherwise `0`.
#[unhackable]
#[noninterference]
#[trusted]
#[ensures(
    "old(actor) >= 1 && old(actor) <= 3 && old(decision) >= 1 && old(decision) <= 6 -> result == 1"
)]
#[ensures(
    "old(actor) < 1 || old(actor) > 3 || old(decision) < 1 || old(decision) > 6 -> result == 0"
)]
#[input(actor, trusted)]
#[input(decision, trusted)]
pub(crate) fn admit(actor: u64, decision: u64) -> u64 {
    if actor < 1 {
        return 0;
    }
    if actor > 3 {
        return 0;
    }
    if decision < 1 {
        return 0;
    }
    if decision > 6 {
        return 0;
    }
    1
}

/// `1` when the secret is at least 8 bytes. Otherwise `0`.
#[unhackable]
#[noninterference]
#[trusted]
#[ensures("old(secret_len) >= 8 -> result == 1")]
#[ensures("old(secret_len) < 8 -> result == 0")]
#[input(secret_len, trusted)]
pub(crate) fn redact_secret(secret_len: u64) -> u64 {
    if secret_len < 8 {
        return 0;
    }
    1
}

/// The smaller of `len` and `limit`.
#[unhackable]
#[noninterference]
#[trusted]
#[ensures("old(len) > old(limit) -> result == old(limit)")]
#[ensures("old(len) <= old(limit) -> result == old(len)")]
#[ensures("result <= old(len)")]
#[ensures("result <= old(limit)")]
#[input(len, trusted)]
#[input(limit, trusted)]
pub(crate) fn kept_chars(len: u64, limit: u64) -> u64 {
    if len > limit {
        return limit;
    }
    len
}

/// The log stores these words. The numbers are what [`admit`] accepts.
/// `0` is not a word.
pub(crate) fn known(actor: &str, decision: &str) -> bool {
    admit(actor_code(actor), decision_code(decision)) == 1
}

pub(crate) fn hides_secret(byte_len: usize) -> bool {
    redact_secret(byte_len as u64) == 1
}

pub(crate) fn kept(len: usize, limit: usize) -> usize {
    kept_chars(len as u64, limit as u64) as usize
}

fn actor_code(actor: &str) -> u64 {
    match actor {
        "user" => 1,
        "session" => 2,
        "mcp" => 3,
        _ => 0,
    }
}

fn decision_code(decision: &str) -> u64 {
    match decision {
        "approved" => 1,
        "declined" => 2,
        "auto" => 3,
        "refused" => 4,
        "ran" => 5,
        "failed" => 6,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_the_codes_the_kernel_admits() {
        let actors = ["user", "session", "mcp"];
        let decisions = ["approved", "declined", "auto", "refused", "ran", "failed"];
        for actor in actors {
            for decision in decisions {
                assert!(known(actor, decision), "{actor} {decision}");
            }
        }
        assert!(!known("root", "approved"));
        assert!(!known("user", "maybe"));
        assert!(!known("", ""));
        assert_eq!(admit(0, 1), 0);
        assert_eq!(admit(4, 1), 0);
        assert_eq!(admit(1, 0), 0);
        assert_eq!(admit(1, 7), 0);
        assert_eq!(admit(2, 5), 1);
    }

    #[test]
    fn redaction_starts_at_eight_bytes() {
        assert!(!hides_secret(0));
        assert!(!hides_secret(7));
        assert!(hides_secret(8));
        assert!(hides_secret(64));
        assert_eq!(redact_secret(8), 1);
        assert_eq!(redact_secret(7), 0);
    }

    #[test]
    fn kept_is_the_shorter_count() {
        assert_eq!(kept(10, 240), 10);
        assert_eq!(kept(240, 240), 240);
        assert_eq!(kept(241, 240), 240);
        assert_eq!(kept(0, 240), 0);
        assert_eq!(kept_chars(241, 240), 240);
        assert_eq!(kept_chars(3, 240), 3);
    }
}
