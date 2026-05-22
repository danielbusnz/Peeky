//! Conversational path. Used when the classifier returns Intent::Chat
//! (the user asked something that doesn't need the screen, doesn't need
//! a service tool, and doesn't need multi-step planning). Just talk back.
//!
//! Differences from `run_agent_loop`:
//!   * No tools at all. Claude can't accidentally try to call gmail or
//!     move the cursor on a casual question.
//!   * No screenshot attached.
//!   * No agent loop. One streaming response, every text delta is piped
//!     to the caller's `on_text_delta` so TTS can start speaking on the
//!     first sentence boundary.
//!   * Optional user_profile string (loaded from memory) gets injected
//!     into the system prompt so Claude knows who it's talking to.
//!
//! The fast path most voice turns will take. Target latency: ~600-800ms
//! release-to-speech when cache is warm.

use super::Claude;
use futures_util::StreamExt;

impl Claude {
    /// Run a chat turn. Streams text deltas via `on_text_delta` and
    /// returns the full assembled text when the stream ends.
    pub async fn chat<T>(
        &self,
        transcript: &str,
        user_profile: Option<&str>,
        mut on_text_delta: T,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>
    where
        T: FnMut(&str),
    {
        // System prompt has two cacheable blocks: the stable behavioral
        // chunk (always identical) and the user profile (stable per
        // session, changes only when memory is rewritten). Two breakpoints
        // let Claude reuse the first block even when profile changes.
        let mut system_blocks = vec![serde_json::json!({
            "type": "text",
            "text": chat_system_prompt(),
            "cache_control": { "type": "ephemeral" }
        })];
        if let Some(profile) = user_profile.filter(|p| !p.trim().is_empty()) {
            system_blocks.push(serde_json::json!({
                "type": "text",
                "text": format!("User profile (facts the user told you to remember):\n{}", profile),
                "cache_control": { "type": "ephemeral" }
            }));
        }

        let body = serde_json::json!({
            "model": "claude-haiku-4-5",
            "max_tokens": 1024,
            "stream": true,
            "system": system_blocks,
            "messages": [
                { "role": "user", "content": transcript }
            ]
        });

        let t_send = std::time::Instant::now();
        let response = self
            .apply_auth(self.http.post(&self.endpoint))
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;
        eprintln!("[chat] upload + response headers → {:?}", t_send.elapsed());

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            return Err(format!("chat API error {}: {}", status, body_text).into());
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut text_content = String::new();
        let mut first_byte_logged = false;
        let t_stream_start = std::time::Instant::now();

        while let Some(chunk) = stream.next().await {
            if !first_byte_logged {
                eprintln!("[chat] first SSE byte → {:?}", t_stream_start.elapsed());
                first_byte_logged = true;
            }
            let chunk = chunk?;
            let s = std::str::from_utf8(&chunk)?;
            buffer.push_str(s);

            while let Some(idx) = buffer.find("\n\n") {
                let frame: String = buffer.drain(..idx + 2).collect();
                for line in frame.lines() {
                    let Some(data) = line.strip_prefix("data: ") else {
                        continue;
                    };
                    let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                        continue;
                    };
                    if event["type"].as_str() == Some("content_block_delta")
                        && event["delta"]["type"].as_str() == Some("text_delta")
                        && let Some(t) = event["delta"]["text"].as_str()
                    {
                        text_content.push_str(t);
                        on_text_delta(t);
                    }
                }
            }
        }

        eprintln!(
            "[chat] stream complete → {:?} ({} chars)",
            t_stream_start.elapsed(),
            text_content.len()
        );
        Ok(text_content)
    }
}

/// Chat's behavioral system prompt. Stable across the whole session so
/// it caches well. Keep short. Every token here is sent on every chat
/// turn.
fn chat_system_prompt() -> &'static str {
    "You are aegis, a voice assistant running on the user's desktop. The \
user is speaking to you and hearing your replies via TTS, so:\n\
- Be concise. Aim for 1-3 sentences unless the user asks for detail.\n\
- Plain prose only. No markdown, no lists, no code blocks. They sound \
weird when read aloud.\n\
- Conversational tone. Imagine you're talking, not writing.\n\
- Don't restate the question. Just answer it.\n\
- If the user asks something you don't know, say so briefly. Don't \
guess and don't pad with disclaimers.\n\
\n\
You don't have access to the user's screen, files, or any external \
services in this conversation. If they ask for something that needs \
those, tell them to phrase it differently (e.g. \"check my email\" or \
\"where is X on screen\")."
}
