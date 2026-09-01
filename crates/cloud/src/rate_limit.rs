//! In-memory sliding-window failure counter, used by `web.rs`'s
//! `POST /web/login` (task spec: "Rate-limit /web/login: 10 failures per 10
//! min per email → 429 (simple in-memory map is fine, document the
//! single-instance assumption)").
//!
//! SINGLE-INSTANCE ASSUMPTION: this state lives only in this process's
//! memory. It bounds brute-force guessing against *one* `kikimimi-cloud`
//! process; a deployment that runs several replicas behind a load balancer
//! would let an attacker get `max_failures` attempts against *each*
//! replica before any of them 429s, since the replicas don't share this
//! map. Acceptable for Stage 0 (architecture.md §12) — a real multi-instance
//! deployment would need this counter backed by something shared (Postgres,
//! Redis, ...) instead.
//!
//! Unbounded-growth note: a distinct `email` key is created per attempted
//! login, live entries are pruned lazily on next access, and there is no
//! separate sweeper — a sustained flood of one-off emails would grow this
//! map. Not addressed here (same Stage 0 scope note as above); the ingest
//! path's `AppState::ingest_semaphore` is the only other overload guard this
//! crate has, and it's a different shape of problem (concurrency, not
//! per-key history).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct LoginRateLimiter {
    max_failures: usize,
    window: Duration,
    failures: Mutex<HashMap<String, Vec<Instant>>>,
}

impl LoginRateLimiter {
    pub fn new(max_failures: usize, window: Duration) -> Self {
        Self {
            max_failures,
            window,
            failures: Mutex::new(HashMap::new()),
        }
    }

    /// `true` once `key` has `max_failures` or more failures recorded within
    /// the trailing `window` — the caller should reject with 429 *without*
    /// even checking credentials once this is true (so a blocked attacker
    /// can't use response timing/content to keep probing).
    ///
    /// Also prunes `key`'s stale (outside-the-window) entries as a side
    /// effect, so a key that stops failing eventually shrinks back down
    /// rather than accumulating forever.
    pub fn is_blocked(&self, key: &str) -> bool {
        let mut map = self.lock();
        let now = Instant::now();
        let entry = map.entry(key.to_string()).or_default();
        entry.retain(|t| now.duration_since(*t) < self.window);
        entry.len() >= self.max_failures
    }

    pub fn record_failure(&self, key: &str) {
        let mut map = self.lock();
        map.entry(key.to_string()).or_default().push(Instant::now());
    }

    /// Drops all recorded failures for `key` — called on a successful login
    /// so a user who mistyped their invite code a few times isn't left
    /// halfway to a lockout after finally getting it right.
    pub fn clear(&self, key: &str) {
        self.lock().remove(key);
    }

    /// Poisoned-mutex recovery: a panic elsewhere while holding this lock
    /// must not turn every subsequent login attempt into a 500 — the
    /// recovered map is still perfectly usable (worst case: a few stale
    /// entries), so continuing with it is strictly better than propagating
    /// the poison.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Vec<Instant>>> {
        self.failures.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Default for LoginRateLimiter {
    /// Task spec default: 10 failures / 10 minutes.
    fn default() -> Self {
        Self::new(10, Duration::from_secs(10 * 60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_blocked_below_threshold() {
        let rl = LoginRateLimiter::new(3, Duration::from_secs(60));
        assert!(!rl.is_blocked("a@example.com"));
        rl.record_failure("a@example.com");
        rl.record_failure("a@example.com");
        assert!(
            !rl.is_blocked("a@example.com"),
            "2 failures < max_failures 3"
        );
    }

    #[test]
    fn blocked_at_threshold() {
        let rl = LoginRateLimiter::new(3, Duration::from_secs(60));
        rl.record_failure("a@example.com");
        rl.record_failure("a@example.com");
        rl.record_failure("a@example.com");
        assert!(
            rl.is_blocked("a@example.com"),
            "3rd failure hits max_failures 3"
        );
    }

    #[test]
    fn keys_are_independent() {
        let rl = LoginRateLimiter::new(1, Duration::from_secs(60));
        rl.record_failure("a@example.com");
        assert!(rl.is_blocked("a@example.com"));
        assert!(
            !rl.is_blocked("b@example.com"),
            "a different email must not share a's count"
        );
    }

    #[test]
    fn clear_resets_the_count() {
        let rl = LoginRateLimiter::new(1, Duration::from_secs(60));
        rl.record_failure("a@example.com");
        assert!(rl.is_blocked("a@example.com"));
        rl.clear("a@example.com");
        assert!(!rl.is_blocked("a@example.com"));
    }

    #[test]
    fn old_failures_outside_the_window_expire() {
        let rl = LoginRateLimiter::new(1, Duration::from_millis(20));
        rl.record_failure("a@example.com");
        assert!(rl.is_blocked("a@example.com"));
        std::thread::sleep(Duration::from_millis(40));
        assert!(
            !rl.is_blocked("a@example.com"),
            "failure is now outside the window"
        );
    }
}
