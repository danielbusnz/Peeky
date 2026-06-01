//! Client-side cache for short-lived tokens the proxy mints (STT, TTS).
//!
//! The proxy hands out tokens with a bounded TTL. Minting one on every turn
//! (STT) or every sentence (TTS) adds an HTTPS round trip to the hot path and
//! burns a usage turn each time. This caches the token and only re-mints once
//! it nears expiry, so the common case is a lock plus a clone with no network.
//! Shared by both providers, which are `Clone` and used across the turn loop,
//! so the cache lives behind an `Arc<Mutex<_>>`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub(crate) struct TokenCache {
    inner: Arc<Mutex<Option<Cached>>>,
}

struct Cached {
    value: String,
    expires_at: Instant,
}

impl TokenCache {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
        }
    }

    /// The cached token if it is still comfortably valid, else `None`.
    pub(crate) fn get(&self) -> Option<String> {
        // reason: the lock guards a small Option and is never held across an
        // await, so it cannot deadlock. Poisoning would require a holder to
        // panic mid-mutation, which the trivial bodies here never do.
        let guard = self.inner.lock().expect("token cache lock poisoned");
        match &*guard {
            Some(c) if Instant::now() < c.expires_at => Some(c.value.clone()),
            _ => None,
        }
    }

    /// Store a freshly minted token, treating it as expired `margin_secs`
    /// before its real TTL so we refresh before Deepgram/Cartesia would reject.
    pub(crate) fn put(&self, value: String, ttl_secs: u64, margin_secs: u64) {
        let live = ttl_secs.saturating_sub(margin_secs).max(1);
        let expires_at = Instant::now() + Duration::from_secs(live);
        // reason: see `get`. Held only for the assignment below, no await.
        let mut guard = self.inner.lock().expect("token cache lock poisoned");
        *guard = Some(Cached { value, expires_at });
    }
}
