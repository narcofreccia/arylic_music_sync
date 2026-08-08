//! Process-wide app state: the config mirror, the login session and the
//! brute-force throttle. Managed by Tauri in `run()`.

use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::discovery::ScanControl;
use crate::poller::Poller;
use crate::store::Config;

/// Failed logins allowed before the cool-down kicks in.
pub const MAX_FAILURES: u32 = 5;
/// How long the lockout lasts once tripped.
pub const LOCKOUT: Duration = Duration::from_secs(30);

/// Login session. In-memory only: quitting the app always ends the session,
/// "remember me" (FR-2) is what survives a restart, and it lives in the config.
#[derive(Debug, Clone, Copy, Default)]
pub struct Session {
    pub logged_in: bool,
}

/// Per-process brute-force throttle: 5 failures → 30 s lockout.
///
/// Deliberately not persisted — the hash is Argon2 and the attacker model here
/// is a person at the keyboard, not an offline cracker. `now` is a parameter so
/// the policy is testable without sleeping.
#[derive(Debug, Default)]
pub struct LoginThrottle {
    failures: u32,
    locked_until: Option<Instant>,
}

impl LoginThrottle {
    /// Remaining cool-down, or `None` if logins are allowed right now.
    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        match self.locked_until {
            Some(until) if until > now => Some(until - now),
            _ => None,
        }
    }

    /// Record a wrong password; trips the lockout on the Nth consecutive miss.
    pub fn record_failure(&mut self, now: Instant) {
        // A failure after the cool-down expired starts a fresh streak.
        if self.locked_until.is_some_and(|until| until <= now) {
            self.failures = 0;
            self.locked_until = None;
        }
        self.failures += 1;
        if self.failures >= MAX_FAILURES {
            self.locked_until = Some(now + LOCKOUT);
        }
    }

    /// Successful login — clear the streak.
    pub fn reset(&mut self) {
        self.failures = 0;
        self.locked_until = None;
    }
}

pub struct AppState {
    pub config: RwLock<Config>,
    pub session: RwLock<Session>,
    pub throttle: Mutex<LoginThrottle>,
    /// Per-device poll tasks (each owning a persistent Luci connection) and
    /// their last-known snapshots.
    pub poller: Poller,
    /// The one-scan-at-a-time slot (FR-4); holds the running scan's cancel token.
    pub scan: ScanControl,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        Self {
            config: RwLock::new(config),
            session: RwLock::new(Session::default()),
            throttle: Mutex::new(LoginThrottle::default()),
            poller: Poller::default(),
            scan: ScanControl::default(),
        }
    }

    pub fn logged_in(&self) -> bool {
        self.session.read().expect("session lock poisoned").logged_in
    }

    pub fn set_logged_in(&self, value: bool) {
        self.session.write().expect("session lock poisoned").logged_in = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_allows_until_max_failures() {
        let now = Instant::now();
        let mut t = LoginThrottle::default();
        for _ in 0..MAX_FAILURES - 1 {
            t.record_failure(now);
            assert!(t.remaining(now).is_none());
        }
        t.record_failure(now);
        assert!(t.remaining(now).is_some(), "5th failure must lock out");
    }

    #[test]
    fn throttle_expires_and_resets() {
        let now = Instant::now();
        let mut t = LoginThrottle::default();
        for _ in 0..MAX_FAILURES {
            t.record_failure(now);
        }
        let later = now + LOCKOUT + Duration::from_secs(1);
        assert!(t.remaining(later).is_none(), "lockout must expire");

        // A miss after expiry starts a new streak rather than re-locking.
        t.record_failure(later);
        assert!(t.remaining(later).is_none());
    }

    #[test]
    fn throttle_reset_clears_lockout() {
        let now = Instant::now();
        let mut t = LoginThrottle::default();
        for _ in 0..MAX_FAILURES {
            t.record_failure(now);
        }
        t.reset();
        assert!(t.remaining(now).is_none());
    }
}
