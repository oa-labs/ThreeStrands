//! Retry bookkeeping for control objects this device cannot yet verify.
//!
//! Some control objects name a sender or invitation this device has never
//! heard of, so the sweep can neither apply nor reject them. Often that is
//! innocent: a member who joined while this device was away announces
//! itself on a later sweep. But anyone who can write to the bucket can also
//! plant such objects, so the sweep may not retry them forever, and may not
//! keep their bytes around between sweeps.
//!
//! This state is in memory only. A restart forgets it and costs, at worst,
//! one extra download per tracked object per launch.

use std::collections::HashMap;

/// Tunables for [`DeferredControlObjects`]. Injectable so tests need no
/// real clock.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RetryPolicy {
    /// Delay before the first retry; doubles on each further attempt.
    pub base_backoff_ms: i64,
    /// Ceiling for the doubling delay.
    pub max_backoff_ms: i64,
    /// How long, from the first time this device saw the object, it is
    /// retried before being ignored for good.
    pub horizon_ms: i64,
    /// Most objects tracked at once. A flood past this is ignored on first
    /// sight, so memory stays bounded.
    pub max_tracked: usize,
    /// How far past this device's clock an object's own timestamp may be
    /// before the object is treated as invalid rather than merely early.
    pub future_skew_ms: i64,
}

impl RetryPolicy {
    /// Values chosen so a late-arriving announcement is still caught:
    /// - backoff 1 minute doubling to 6 hours, so a member's announcement
    ///   that lands minutes after its rotation is picked up on the next
    ///   few sweeps, while a stuck object settles to four downloads a day;
    /// - a 7 day horizon, the longest join-code lifetime, comfortably
    ///   longer than a peer is normally offline before it reconnects and
    ///   publishes its announcement. If a rotation is still unapplied
    ///   after that, key catch-up (`share_keys_with_lagging_peers`)
    ///   delivers the key instead. The horizon counts from this process's
    ///   first sight, so a restart can only extend it;
    /// - at most 256 tracked objects, far above the handful of rotations a
    ///   real group has in flight;
    /// - 15 minutes of clock skew, generous for NTP-synced machines and
    ///   far from the `i64::MAX` an attacker would pick.
    pub(crate) const DEFAULT: Self = Self {
        base_backoff_ms: 60_000,
        max_backoff_ms: 6 * 3_600_000,
        horizon_ms: 7 * 24 * 3_600_000,
        max_tracked: 256,
        future_skew_ms: 15 * 60_000,
    };

    fn backoff_ms(&self, attempts: u32) -> i64 {
        let shift = attempts.saturating_sub(1).min(30);
        self.base_backoff_ms
            .saturating_mul(1i64 << shift)
            .min(self.max_backoff_ms)
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    first_seen_ms: i64,
    attempts: u32,
    next_due_ms: i64,
}

/// What to do with an object that just failed to apply for want of context.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Deferral {
    /// Keep it unseen; look again when due.
    Retry,
    /// Mark it seen as ignored.
    GiveUp(&'static str),
}

/// CIDs awaiting a retry, with their retry state. Never holds object bytes.
#[derive(Debug, Default)]
pub(crate) struct DeferredControlObjects {
    entries: HashMap<String, Entry>,
}

impl DeferredControlObjects {
    /// Whether `cid` is waiting out a backoff and must not be fetched yet.
    pub(crate) fn is_waiting(&self, cid: &str, now_ms: i64) -> bool {
        self.entries
            .get(cid)
            .is_some_and(|entry| now_ms < entry.next_due_ms)
    }

    /// Removes and returns every CID whose horizon has passed.
    pub(crate) fn take_expired(&mut self, now_ms: i64, policy: &RetryPolicy) -> Vec<String> {
        let expired: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| now_ms.saturating_sub(entry.first_seen_ms) >= policy.horizon_ms)
            .map(|(cid, _)| cid.clone())
            .collect();
        for cid in &expired {
            self.entries.remove(cid);
        }
        expired
    }

    /// Records a failed attempt on `cid`, whose own claimed timestamp is
    /// `created_at_ms`.
    pub(crate) fn defer(
        &mut self,
        cid: &str,
        created_at_ms: i64,
        now_ms: i64,
        policy: &RetryPolicy,
    ) -> Deferral {
        if created_at_ms > now_ms.saturating_add(policy.future_skew_ms) {
            self.entries.remove(cid);
            return Deferral::GiveUp("timestamp in the future");
        }
        if let Some(entry) = self.entries.get_mut(cid) {
            entry.attempts = entry.attempts.saturating_add(1);
            entry.next_due_ms = now_ms.saturating_add(policy.backoff_ms(entry.attempts));
            return Deferral::Retry;
        }
        if self.entries.len() >= policy.max_tracked {
            return Deferral::GiveUp("too many unverifiable objects");
        }
        self.entries.insert(
            cid.to_string(),
            Entry {
                first_seen_ms: now_ms,
                attempts: 1,
                next_due_ms: now_ms.saturating_add(policy.backoff_ms(1)),
            },
        );
        Deferral::Retry
    }

    pub(crate) fn forget(&mut self, cid: &str) {
        self.entries.remove(cid);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}
