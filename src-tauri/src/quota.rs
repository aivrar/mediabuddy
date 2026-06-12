//! Live quota tracking. Every successful (or failing) API call updates a
//! per-source slot with whatever rate-limit headers the provider returned.
//! The Settings tab + search bar read this snapshot to render quota chips.

use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
pub struct QuotaSlot {
    /// Last response's `x-ratelimit-remaining` if present.
    pub remaining: Option<u64>,
    /// Last response's `x-ratelimit-limit` if present.
    pub limit: Option<u64>,
    /// Seconds-until-reset (Pexels/Pixabay) — interpreted server-side as
    /// "the moment the count snaps back". Stored as an absolute epoch
    /// second so the frontend can render `resets in 14m` correctly even
    /// after some idle time.
    pub reset_epoch: Option<u64>,
    /// HTTP status of the last call (200, 401, 429, …).
    pub last_status: Option<u16>,
    /// Wall clock the slot was last touched, in epoch seconds.
    pub last_seen: Option<u64>,
    /// Total successful calls observed for this source since process start.
    pub total_calls: u64,
}

#[derive(Debug, Default, Serialize, Clone)]
pub struct QuotaSnapshot {
    pub pixabay: QuotaSlot,
    pub pexels: QuotaSlot,
    pub unsplash: QuotaSlot,
}

#[derive(Debug, Clone, Copy)]
pub enum Source {
    Pixabay,
    Pexels,
    Unsplash,
}

#[derive(Debug)]
pub struct QuotaTracker {
    inner: RwLock<QuotaSnapshot>,
}

impl QuotaTracker {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(QuotaSnapshot::default()),
        }
    }

    pub fn snapshot(&self) -> QuotaSnapshot {
        self.inner.read().unwrap().clone()
    }

    /// Update the slot for `source` from a finished `reqwest::Response`.
    /// Pulls `x-ratelimit-{limit,remaining,reset}` headers; reset is
    /// interpreted as either delta-seconds (small numbers) or absolute
    /// epoch (large numbers).
    pub fn record(&self, source: Source, response: &reqwest::Response) {
        let mut snap = self.inner.write().unwrap();
        let slot = match source {
            Source::Pixabay => &mut snap.pixabay,
            Source::Pexels => &mut snap.pexels,
            Source::Unsplash => &mut snap.unsplash,
        };
        slot.last_status = Some(response.status().as_u16());
        slot.last_seen = Some(now_secs());
        if response.status().is_success() {
            slot.total_calls = slot.total_calls.saturating_add(1);
        }

        let h = response.headers();
        if let Some(v) = header_u64(h, "x-ratelimit-remaining") {
            slot.remaining = Some(v);
        }
        if let Some(v) = header_u64(h, "x-ratelimit-limit") {
            slot.limit = Some(v);
        }
        if let Some(v) = header_u64(h, "x-ratelimit-reset") {
            // Pexels returns absolute epoch; Pixabay returns seconds-from-now.
            // Heuristic: anything > 1e9 is epoch, anything else is delta.
            let now = now_secs();
            let abs = if v > 1_000_000_000 {
                v
            } else {
                now.saturating_add(v)
            };
            slot.reset_epoch = Some(abs);
        }
    }
}

fn header_u64(h: &reqwest::header::HeaderMap, key: &str) -> Option<u64> {
    h.get(key)?.to_str().ok()?.trim().parse().ok()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
