//! One Gmail quota budget per account, shared by every client built for it.
//!
//! Gmail limits quota units per user per minute. Each sync pass, message
//! open, attachment download, draft save, and send builds its own
//! [`GmailClient`](super::GmailClient), so a budget kept on the client would
//! let concurrent paths spend it several times over. The gate instead lives
//! in a process-wide registry keyed by account, charges every request its
//! documented quota cost, and, once Gmail reports the limit exceeded, holds
//! back every request for that account until the backoff has passed.
//!
//! The budget adapts. Every installation signed in to an account spends the
//! same per-user quota, and each can see only its own requests, so a refusal
//! is the sign that the account's other traffic leaves this one less room.
//! Each overrun halves the budget, down to a floor; after a quiet spell it
//! grows back steadily. Installations that back off together settle below
//! the limit without having to know about one another.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex as StdMutex},
    time::Duration,
};

use reqwest::Method;
use tokio::{
    sync::Mutex,
    time::{sleep, Instant},
};

/// Sustained spend while Gmail hasn't refused this account: two thirds of
/// the 6,000 per-user units a minute, leaving headroom for another
/// installation signed in to the same account.
pub(super) const UNITS_PER_MINUTE: u32 = 4_000;

/// The least an overrun cuts the budget to, so mail keeps moving however
/// busy the account's other installations are.
pub(super) const MIN_UNITS_PER_MINUTE: u32 = 500;

/// Refusals this close to the last cut belong to the same overrun, so one
/// burst of 403s halves the budget once.
const CUT_WINDOW: Duration = Duration::from_secs(60);

/// How long the budget stays at a cut before growing back.
const RECOVERY_QUIET: Duration = Duration::from_secs(2 * 60);

/// How fast the budget grows back after [`RECOVERY_QUIET`], in units a
/// minute gained per minute.
const RECOVERY_UNITS_PER_MINUTE: u32 = 250;

/// Units that may be spent back to back after a quiet period. Exactly the
/// costliest single call (a send, 100 units), and small enough that several
/// installations bursting together stay under Gmail's short-window limit.
pub(super) const BURST_UNITS: u32 = 100;

/// Cost charged for an endpoint missing from [`cost`]'s table.
const UNKNOWN_COST: u32 = 10;

pub(crate) struct QuotaGate {
    /// Held from arrival until a request's units are granted, so waiters are
    /// served in order and a costly request is never starved by cheap ones.
    turn: Mutex<()>,
    state: StdMutex<State>,
}

struct State {
    available: f64,
    refilled: Instant,
    paused_until: Instant,
    /// The budget the last overrun cut to, and when; `None` until Gmail
    /// first refuses this account.
    cut: Option<(f64, Instant)>,
}

impl QuotaGate {
    pub(super) fn new() -> Self {
        let now = Instant::now();
        Self {
            turn: Mutex::new(()),
            state: StdMutex::new(State {
                available: f64::from(BURST_UNITS),
                refilled: now,
                paused_until: now,
                cut: None,
            }),
        }
    }

    /// Waits until `units` can be spent without exceeding the budget, then
    /// spends them.
    pub(super) async fn acquire(&self, units: u32) {
        let units = f64::from(units.clamp(1, BURST_UNITS));
        let _turn = self.turn.lock().await;
        loop {
            let wait = {
                let mut state = self.state.lock().unwrap();
                let now = Instant::now();
                if state.paused_until > now {
                    state.paused_until - now
                } else {
                    state.refill(now);
                    // The epsilon absorbs float error in the accrual, which
                    // would otherwise add a needless microsecond wait.
                    if state.available + 1e-6 >= units {
                        state.available = (state.available - units).max(0.0);
                        return;
                    }
                    let seconds = (units - state.available) / state.units_per_second(now);
                    Duration::from_micros((seconds * 1e6).ceil() as u64)
                }
            };
            sleep(wait).await;
        }
    }

    /// Gmail reported the quota exceeded: hold back every request on this
    /// account for at least `delay`, then resume from an empty budget so
    /// traffic ramps up instead of bursting back into the limit.
    pub(super) fn pause(&self, delay: Duration) {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        if state.cut.is_none_or(|(_, at)| now.duration_since(at) >= CUT_WINDOW) {
            let budget = (state.units_per_minute(now) / 2.0).max(f64::from(MIN_UNITS_PER_MINUTE));
            log::info!("Gmail quota exceeded; this account now spends at most {budget:.0} units a minute");
            state.cut = Some((budget, now));
        }
        let until = now + delay;
        if until > state.paused_until {
            state.paused_until = until;
        }
        state.available = 0.0;
        state.refilled = state.paused_until;
    }

    /// The account's current budget.
    #[cfg(test)]
    pub(super) fn units_per_minute(&self) -> f64 {
        self.state.lock().unwrap().units_per_minute(Instant::now())
    }

    /// How much longer requests are held back by a quota pause.
    #[cfg(test)]
    pub(super) fn paused_for(&self) -> Duration {
        self.state.lock().unwrap().paused_until.saturating_duration_since(Instant::now())
    }
}

impl State {
    /// The budget at `now`: the full rate until Gmail first refuses this
    /// account; after an overrun, the cut rate for [`RECOVERY_QUIET`], then
    /// growing by [`RECOVERY_UNITS_PER_MINUTE`] each minute back to the full
    /// rate.
    fn units_per_minute(&self, now: Instant) -> f64 {
        let full = f64::from(UNITS_PER_MINUTE);
        let Some((cut, at)) = self.cut else {
            return full;
        };
        let recovering = now.saturating_duration_since(at).saturating_sub(RECOVERY_QUIET);
        (cut + f64::from(RECOVERY_UNITS_PER_MINUTE) * recovering.as_secs_f64() / 60.0).min(full)
    }

    fn units_per_second(&self, now: Instant) -> f64 {
        self.units_per_minute(now) / 60.0
    }

    fn refill(&mut self, now: Instant) {
        if now <= self.refilled {
            return;
        }
        let earned = (now - self.refilled).as_secs_f64() * self.units_per_second(now);
        self.available = (self.available + earned).min(f64::from(BURST_UNITS));
        self.refilled = now;
    }
}

static GATES: LazyLock<StdMutex<HashMap<String, Arc<QuotaGate>>>> =
    LazyLock::new(Default::default);

/// The gate every Gmail client for `account` shares. Kept for the life of
/// the process: a pause must outlive the short-lived client that saw it.
pub(super) fn for_account(account: &str) -> Arc<QuotaGate> {
    GATES
        .lock()
        .unwrap()
        .entry(account.to_ascii_lowercase())
        .or_insert_with(|| Arc::new(QuotaGate::new()))
        .clone()
}

/// Gmail's documented quota cost of a request, from its method and its path
/// below `users/me` (query string allowed).
pub(super) fn cost(method: &Method, path: &str) -> u32 {
    let path = path.split(['?', '#']).next().unwrap_or_default().trim_matches('/');
    let segments: Vec<&str> = path.split('/').collect();
    let get = method == Method::GET;
    let delete = method == Method::DELETE;
    match segments.as_slice() {
        ["profile"] => 1,
        ["history"] => 2,
        ["labels", ..] if get => 1,
        ["labels", ..] => 5,
        ["settings", ..] if get => 1,
        ["settings", ..] => 100,
        ["threads", _] if delete => 20,
        ["threads", ..] => 10,
        ["messages", "send"] => 100,
        ["messages", "batchModify" | "batchDelete"] => 50,
        ["messages", "import" | "insert"] => 25,
        ["messages", _] if delete => 10,
        ["messages", ..] => 5,
        ["drafts", "send"] => 100,
        ["drafts", ..] if get => 5,
        ["drafts"] => 10,
        ["drafts", _] if delete => 10,
        ["drafts", _] => 15,
        _ => UNKNOWN_COST,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_near(actual: Duration, expected: Duration) {
        let gap = actual.abs_diff(expected);
        assert!(gap < Duration::from_millis(1), "{actual:?} is not {expected:?}");
    }

    #[test]
    fn requests_are_charged_gmails_documented_cost() {
        let cases = [
            (Method::GET, "/threads/t-1?format=full", 10),
            (Method::GET, "/threads?maxResults=100&labelIds=INBOX", 10),
            (Method::POST, "/threads/t-1/modify", 10),
            (Method::DELETE, "/threads/t-1", 20),
            (Method::GET, "/messages/m-1?format=full", 5),
            (Method::GET, "/messages?q=in:sent", 5),
            (Method::GET, "/messages/m-1/attachments/a-1", 5),
            (Method::POST, "/messages/m-1/modify", 5),
            (Method::POST, "/messages/batchModify", 50),
            (Method::POST, "/messages/send", 100),
            (Method::GET, "/history?startHistoryId=1", 2),
            (Method::GET, "/profile", 1),
            (Method::GET, "/labels", 1),
            (Method::POST, "/labels", 5),
            (Method::PATCH, "/labels/l-1", 5),
            (Method::DELETE, "/labels/l-1", 5),
            (Method::GET, "/settings/sendAs", 1),
            (Method::GET, "/drafts", 5),
            (Method::POST, "/drafts", 10),
            (Method::PUT, "/drafts/d-1", 15),
            (Method::DELETE, "/drafts/d-1", 10),
            (Method::POST, "/somethingNew", UNKNOWN_COST),
        ];
        for (method, path, expected) in cases {
            assert_eq!(cost(&method, path), expected, "{method} {path}");
        }
    }

    #[test]
    fn clients_for_one_account_share_a_gate_and_other_accounts_do_not() {
        let first = for_account("quota-share@example.com");
        assert!(Arc::ptr_eq(&first, &for_account("Quota-Share@Example.com")));
        assert!(!Arc::ptr_eq(&first, &for_account("quota-other@example.com")));
    }

    #[tokio::test(start_paused = true)]
    async fn a_burst_is_served_immediately_then_paced_to_the_sustained_rate() {
        let gate = QuotaGate::new();
        let start = Instant::now();
        // Ten thread fetches spend the whole burst without waiting.
        for _ in 0..BURST_UNITS / 10 {
            gate.acquire(10).await;
        }
        assert_eq!(Instant::now(), start);
        // The next one waits for ten units to accrue: 150ms at 4,000 a minute.
        gate.acquire(10).await;
        assert_near(Instant::now() - start, Duration::from_millis(150));
    }

    #[tokio::test(start_paused = true)]
    async fn sustained_traffic_never_exceeds_the_per_minute_budget() {
        let gate = QuotaGate::new();
        let start = Instant::now();
        let mut spent = 0;
        while Instant::now() - start < Duration::from_secs(60) {
            gate.acquire(10).await;
            spent += 10;
        }
        // Whatever finished inside the first minute fits the burst plus a
        // minute of sustained spend.
        assert!(spent <= BURST_UNITS + UNITS_PER_MINUTE + 10, "spent {spent}");
        assert!(spent >= UNITS_PER_MINUTE, "spent {spent}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_pause_holds_every_waiter_and_resumes_from_an_empty_budget() {
        let gate = Arc::new(QuotaGate::new());
        let start = Instant::now();
        gate.pause(Duration::from_secs(15));
        let waiters: Vec<_> = (0..2)
            .map(|_| {
                let gate = gate.clone();
                tokio::spawn(async move {
                    gate.acquire(5).await;
                    Instant::now() - start
                })
            })
            .collect();
        let mut finished = Vec::new();
        for waiter in waiters {
            finished.push(waiter.await.unwrap());
        }
        finished.sort();
        // Nothing passes during the pause, and the budget restarts empty at
        // half the rate, so even the first request waits for its own units.
        assert_near(finished[0], Duration::from_millis(15_150));
        assert_near(finished[1], Duration::from_millis(15_300));
    }

    #[tokio::test(start_paused = true)]
    async fn a_shorter_pause_never_cuts_a_longer_one_short() {
        let gate = QuotaGate::new();
        let start = Instant::now();
        gate.pause(Duration::from_secs(30));
        gate.pause(Duration::from_secs(5));
        gate.acquire(1).await;
        assert!(Instant::now() - start >= Duration::from_secs(30));
    }

    #[tokio::test(start_paused = true)]
    async fn one_overrun_halves_the_budget_once_down_to_a_floor() {
        let gate = QuotaGate::new();
        assert_eq!(gate.units_per_minute(), 4_000.0);
        // A burst of refusals is one overrun.
        for _ in 0..3 {
            gate.pause(Duration::from_secs(1));
        }
        assert_eq!(gate.units_per_minute(), 2_000.0);
        let mut expected = Vec::new();
        let mut seen = Vec::new();
        for budget in [1_000.0, 500.0, 500.0] {
            tokio::time::advance(CUT_WINDOW).await;
            gate.pause(Duration::from_secs(1));
            expected.push(budget);
            seen.push(gate.units_per_minute());
        }
        assert_eq!(seen, expected);
    }

    #[tokio::test(start_paused = true)]
    async fn the_budget_grows_back_after_a_quiet_spell() {
        let gate = QuotaGate::new();
        gate.pause(Duration::from_secs(1));
        tokio::time::advance(RECOVERY_QUIET).await;
        assert_eq!(gate.units_per_minute(), 2_000.0);
        tokio::time::advance(Duration::from_secs(4 * 60)).await;
        assert_eq!(gate.units_per_minute(), 3_000.0);
        tokio::time::advance(Duration::from_secs(30 * 60)).await;
        assert_eq!(gate.units_per_minute(), f64::from(UNITS_PER_MINUTE));
    }

    #[tokio::test(start_paused = true)]
    async fn an_overrun_while_recovering_halves_the_recovered_budget() {
        let gate = QuotaGate::new();
        gate.pause(Duration::from_secs(1));
        tokio::time::advance(RECOVERY_QUIET + Duration::from_secs(4 * 60)).await;
        gate.pause(Duration::from_secs(1));
        assert_eq!(gate.units_per_minute(), 1_500.0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_cut_budget_paces_requests_more_slowly() {
        let gate = QuotaGate::new();
        gate.pause(Duration::ZERO);
        let start = Instant::now();
        // 2,000 units a minute: ten units every 300ms, from an empty budget.
        for _ in 0..3 {
            gate.acquire(10).await;
        }
        assert_near(Instant::now() - start, Duration::from_millis(900));
    }
}
