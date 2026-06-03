//! Tier 0: the live working context for the current session. A bounded buffer
//! of the most recent voice turns plus a running summary of older ones,
//! injected into chat/agent requests so Aegis remembers the current
//! conversation, not just stored facts.
//!
//! Not the source of truth: every turn is also written to the durable Tier 2
//! history log. When `recent` grows past WORKING_CONTEXT_COMPACT_AT, the turns
//! older than WORKING_CONTEXT_RECENT_TURNS are folded into `summary` off the
//! hot path (driven by the orchestrator), keeping the live buffer bounded the
//! way Claude Code compacts a long chat.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::Claude;
use crate::tuning::{WORKING_CONTEXT_COMPACT_AT, WORKING_CONTEXT_RECENT_TURNS};

/// One finished voice turn: what the user said and what Aegis spoke back.
#[derive(Clone)]
struct Turn {
    user: String,
    reply: String,
}

/// Thread-safe live conversation context for the session. Cloning is cheap
/// (Arc bumps a refcount); the session holds one and passes it into
/// chat() / agent_loop() the same way it passes MemoryStore.
#[derive(Clone)]
pub struct WorkingContext {
    inner: Arc<Mutex<Inner>>,
    /// True while a compaction is in flight, so only one runs at a time.
    compacting: Arc<AtomicBool>,
}

struct Inner {
    /// Running summary of turns older than the verbatim window. Empty until
    /// the first compaction.
    summary: String,
    /// Recent turns, oldest-first. Bounded by compaction.
    recent: VecDeque<Turn>,
}

impl WorkingContext {
    /// Empty context. One per session, built at startup alongside MemoryStore.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                summary: String::new(),
                recent: VecDeque::new(),
            })),
            compacting: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Claim the right to compact. Returns a guard (held for the duration of
    /// the compaction, releases on drop) if no compaction is already running,
    /// or None if one is. Prevents two summarizer calls racing on the buffer.
    pub fn try_begin_compaction(&self) -> Option<CompactionGuard> {
        if self
            .compacting
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            Some(CompactionGuard {
                flag: self.compacting.clone(),
            })
        } else {
            None
        }
    }

    /// Append a finished turn. Called after TTS completes, off the action path.
    /// A poisoned mutex drops the turn rather than panicking the loop.
    pub fn record(&self, user: &str, reply: &str) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.recent.push_back(Turn {
            user: user.trim().to_string(),
            reply: reply.trim().to_string(),
        });
    }

    /// Render summary + recent turns for injection into a system prompt.
    /// Returns None when empty so callers can skip the block (mirrors
    /// MemoryStore::as_prompt_block). This block changes every turn, so callers
    /// must place it AFTER any cache_control breakpoint.
    pub fn as_prompt_block(&self) -> Option<String> {
        let inner = self.inner.lock().ok()?;
        if inner.summary.is_empty() && inner.recent.is_empty() {
            return None;
        }
        let mut s = String::new();
        if !inner.summary.is_empty() {
            s.push_str("Earlier in this conversation:\n");
            s.push_str(&inner.summary);
            s.push_str("\n\n");
        }
        if !inner.recent.is_empty() {
            s.push_str("Recent turns:\n");
            for t in &inner.recent {
                s.push_str(&format!("User: {}\nAegis: {}\n", t.user, t.reply));
            }
        }
        Some(s)
    }

    /// The current running summary, so the summarizer can extend it during
    /// compaction rather than discarding earlier context.
    pub fn summary(&self) -> String {
        self.inner
            .lock()
            .map(|i| i.summary.clone())
            .unwrap_or_default()
    }

    /// True once `recent` has grown past the compaction threshold.
    pub fn needs_compaction(&self) -> bool {
        self.inner
            .lock()
            .map(|i| i.recent.len() > WORKING_CONTEXT_COMPACT_AT)
            .unwrap_or(false)
    }

    /// The turns older than the verbatim window, rendered as text for the
    /// summarizer, plus how many turns that is. None when there is nothing to
    /// compact. The caller summarizes (prior summary + this text), then calls
    /// `fold` with the result and `drained`.
    pub fn overflow(&self) -> Option<(String, usize)> {
        let inner = self.inner.lock().ok()?;
        if inner.recent.len() <= WORKING_CONTEXT_RECENT_TURNS {
            return None;
        }
        let drained = inner.recent.len() - WORKING_CONTEXT_RECENT_TURNS;
        let mut s = String::new();
        for t in inner.recent.iter().take(drained) {
            s.push_str(&format!("User: {}\nAegis: {}\n", t.user, t.reply));
        }
        Some((s, drained))
    }

    /// Replace the running summary and drop the `drained` oldest turns. Only
    /// the front is removed, so turns recorded while the summarizer ran (pushed
    /// to the back) are preserved. Clamped to the current length.
    pub fn fold(&self, new_summary: String, drained: usize) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let drained = drained.min(inner.recent.len());
        inner.recent.drain(..drained);
        inner.summary = new_summary;
    }
}

impl Default for WorkingContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Held for the duration of a compaction; clears the in-flight flag on drop
/// (including on early return or panic), so a failed compaction doesn't wedge
/// the buffer into a permanently-"compacting" state.
pub struct CompactionGuard {
    flag: Arc<AtomicBool>,
}

impl Drop for CompactionGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

impl Claude {
    /// Fold `new_turns` into `prior_summary` and return the updated running
    /// summary. A small non-streaming Haiku call used for Tier 0 compaction;
    /// the orchestrator runs it off the hot path.
    pub async fn summarize_conversation(
        &self,
        prior_summary: &str,
        new_turns: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let prior = if prior_summary.trim().is_empty() {
            String::new()
        } else {
            format!("Summary so far:\n{prior_summary}\n\n")
        };
        let body = serde_json::json!({
            "model": "claude-haiku-4-5",
            "max_tokens": 256,
            "system": "You maintain a running summary of a voice assistant conversation. \
                       Output only the updated summary as plain prose, no preamble.",
            "messages": [{
                "role": "user",
                "content": format!(
                    "{prior}New turns to fold in:\n{new_turns}\n\nRewrite the summary so it \
                     captures everything useful for continuing the conversation (facts, topics, \
                     decisions, open threads). A few sentences, no preamble."
                )
            }]
        });

        let response = self
            .apply_auth(self.http.post(&self.endpoint))
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            crate::upgrade::on_proxy_error(status.as_u16(), &text);
            return Err(format!("summarize API error {status}: {text}").into());
        }
        let v: serde_json::Value = response.json().await?;
        let text = v["content"][0]["text"]
            .as_str()
            .ok_or("summarize: no text in response")?
            .trim()
            .to_string();
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tuning::{WORKING_CONTEXT_COMPACT_AT, WORKING_CONTEXT_RECENT_TURNS};

    #[test]
    fn empty_renders_none() {
        assert!(WorkingContext::new().as_prompt_block().is_none());
    }

    #[test]
    fn records_and_renders_recent() {
        let wc = WorkingContext::new();
        wc.record("what's the weather", "I can't check that yet");
        let block = wc.as_prompt_block().expect("non-empty");
        assert!(block.contains("Recent turns:"));
        assert!(block.contains("User: what's the weather"));
        assert!(block.contains("Aegis: I can't check that yet"));
    }

    #[test]
    fn compaction_triggers_past_threshold() {
        let wc = WorkingContext::new();
        for i in 0..WORKING_CONTEXT_COMPACT_AT {
            wc.record(&format!("u{i}"), &format!("r{i}"));
        }
        assert!(!wc.needs_compaction(), "at threshold, not over it");
        wc.record("one more", "reply");
        assert!(wc.needs_compaction(), "now over the threshold");
    }

    #[test]
    fn overflow_returns_only_the_oldest_beyond_window() {
        let wc = WorkingContext::new();
        let total = WORKING_CONTEXT_RECENT_TURNS + 3;
        for i in 0..total {
            wc.record(&format!("u{i}"), &format!("r{i}"));
        }
        let (text, drained) = wc.overflow().expect("has overflow");
        assert_eq!(drained, 3);
        assert!(text.contains("u0") && text.contains("u2"));
        assert!(
            !text.contains(&format!("u{}", total - 1)),
            "newest kept verbatim"
        );
    }

    #[test]
    fn fold_drops_oldest_and_sets_summary() {
        let wc = WorkingContext::new();
        for i in 0..WORKING_CONTEXT_RECENT_TURNS + 2 {
            wc.record(&format!("u{i}"), &format!("r{i}"));
        }
        wc.fold("they asked about u0 and u1".to_string(), 2);
        assert_eq!(wc.summary(), "they asked about u0 and u1");
        let block = wc.as_prompt_block().expect("non-empty");
        assert!(block.contains("Earlier in this conversation:"));
        assert!(!block.contains("User: u0"), "u0 was folded out of recent");
        assert!(block.contains("User: u2"), "u2 remains verbatim");
    }

    #[test]
    fn compaction_guard_is_exclusive() {
        let wc = WorkingContext::new();
        let g1 = wc.try_begin_compaction();
        assert!(g1.is_some(), "first claim succeeds");
        assert!(wc.try_begin_compaction().is_none(), "second is blocked");
        drop(g1);
        assert!(wc.try_begin_compaction().is_some(), "released after drop");
    }
}
