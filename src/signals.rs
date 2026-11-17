//! Termination signals for every mode.
//!
//! By default SIGINT, SIGTERM and SIGHUP end Pitty at once, and local
//! commands run in their own sessions, which those signals never reach: they
//! would outlive Pitty. [`install`] replaces that default with an
//! async-signal-safe handler that only records the signal number. Each mode
//! polls [`received`] from its own loop and takes its normal cancellation
//! path, which kills every command's process group and reaps it.
//!
//! The terminal UI wires it the same way: keep the guard for the whole
//! session and poll in the event loop.
//!
//! ```ignore
//! let _signals = crate::signals::install(&[libc::SIGHUP, libc::SIGTERM])?;
//! // In the event loop:
//! if crate::signals::received().is_some() {
//!     // Cancel requests and commands, wait for them, restore the terminal, exit.
//! }
//! ```
//!
//! Dropping the guard restores the previous dispositions; a signal recorded
//! earlier still reads back until the next `install`. Signals ignored when
//! Pitty started (such as SIGHUP under `nohup`) stay ignored. Only one guard
//! may exist at a time.
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

static RECEIVED: AtomicI32 = AtomicI32::new(0);
static INSTALLED: AtomicBool = AtomicBool::new(false);

extern "C" fn record(signal: libc::c_int) {
    // A lock-free atomic store only: no allocation, locking, I/O, or errno use.
    let _ = RECEIVED.compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst);
}

/// Restores the previous signal dispositions when dropped.
#[must_use = "dropping the guard immediately restores the previous signal handling"]
pub struct Guard {
    previous: Vec<(libc::c_int, libc::sigaction)>,
}

/// Records `signals` instead of letting them terminate Pitty, and clears any
/// signal recorded under an earlier guard.
pub fn install(signals: &[libc::c_int]) -> Result<Guard, String> {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return Err("Signal handling is already installed.".into());
    }
    RECEIVED.store(0, Ordering::SeqCst);
    // From here on, an early return drops the guard, which restores every
    // handler installed so far and releases INSTALLED.
    let mut guard = Guard {
        previous: Vec::new(),
    };
    for &signal in signals {
        let failed = || format!("Cannot install the handler for signal {signal}.");
        // SAFETY: an all-zero sigaction is a valid output buffer, and a null
        // new action only queries the current one.
        let mut previous: libc::sigaction = unsafe { std::mem::zeroed() };
        if unsafe { libc::sigaction(signal, std::ptr::null(), &mut previous) } != 0 {
            return Err(failed());
        }
        if previous.sa_sigaction == libc::SIG_IGN {
            continue;
        }
        // SAFETY: `record` has the C signal ABI and is async-signal-safe.
        // SA_RESTART keeps the previous `signal()` semantics; every loop polls.
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = record as *const () as libc::sighandler_t;
        action.sa_flags = libc::SA_RESTART;
        if unsafe { libc::sigemptyset(&mut action.sa_mask) } != 0
            || unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } != 0
        {
            return Err(failed());
        }
        guard.previous.push((signal, previous));
    }
    Ok(guard)
}

/// The first signal recorded since the most recent [`install`].
pub fn received() -> Option<libc::c_int> {
    match RECEIVED.load(Ordering::SeqCst) {
        0 => None,
        signal => Some(signal),
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        for (signal, previous) in self.previous.iter().rev() {
            // SAFETY: `previous` was returned by sigaction for this signal.
            unsafe {
                libc::sigaction(*signal, previous, std::ptr::null_mut());
            }
        }
        INSTALLED.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(signal: libc::c_int) -> libc::sigaction {
        let mut current: libc::sigaction = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::sigaction(signal, std::ptr::null(), &mut current) },
            0
        );
        current
    }
    fn disposition(signal: libc::c_int) -> libc::sighandler_t {
        query(signal).sa_sigaction
    }
    fn apply(signal: libc::c_int, action: &libc::sigaction) {
        assert_eq!(
            unsafe { libc::sigaction(signal, action, std::ptr::null_mut()) },
            0
        );
    }
    fn set(signal: libc::c_int, handler: libc::sighandler_t) {
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = handler;
        apply(signal, &action);
    }

    // The only test that changes signal dispositions; SIGHUP is raised only
    // while it is handled or ignored, never while it would end the test run.
    #[test]
    fn hangup_is_recorded_and_previous_handling_is_restored() {
        let original = query(libc::SIGHUP);
        set(libc::SIGHUP, libc::SIG_DFL);
        let guard = install(&[libc::SIGHUP]).unwrap();
        assert_eq!(received(), None);
        assert!(install(&[libc::SIGTERM]).is_err());
        assert_eq!(unsafe { libc::raise(libc::SIGHUP) }, 0);
        assert_eq!(received(), Some(libc::SIGHUP));
        drop(guard);
        assert_eq!(disposition(libc::SIGHUP), libc::SIG_DFL);
        assert_eq!(received(), Some(libc::SIGHUP));

        // A signal ignored at startup (nohup) stays ignored.
        set(libc::SIGHUP, libc::SIG_IGN);
        let guard = install(&[libc::SIGHUP]).unwrap();
        assert_eq!(received(), None);
        assert_eq!(unsafe { libc::raise(libc::SIGHUP) }, 0);
        assert_eq!(received(), None);
        drop(guard);
        assert_eq!(disposition(libc::SIGHUP), libc::SIG_IGN);
        apply(libc::SIGHUP, &original);
    }
}
