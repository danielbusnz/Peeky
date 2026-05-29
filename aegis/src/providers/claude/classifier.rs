//! Intent router. One small Claude call per voice turn that picks which
//! downstream path handles the request. Adds ~250-400ms of latency per
//! turn (one extra round-trip to Anthropic) but lets each path be a
//! focused, predictable function instead of a flag-soup mega-loop.
//!
//! Uses a forced tool call (`tool_choice: tool`) so Claude can only
//! respond with one of the five categories. Prompt is intentionally
//! short and aggressively cached.

use super::Claude;
use futures_util::StreamExt;

/// The five voice-turn paths. Routed at the top of every turn before
/// any path-specific work happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// Visual + cursor: "where is X", "click X", "type X", "press X",
    /// "show me X", "find X", "select X", "scroll". Goes to find_action.
    FindAction,
    /// Service API: "play X", "pause", "skip", "check my email",
    /// "what's my PRs", "spotify volume up". Goes to integration.
    Integration,
    /// Conversational, no screen, no tools: "what's your name",
    /// "explain X", "how does Y work", small talk. Goes to chat.
    Chat,
    /// Remember/recall: "remember X", "what did I tell you about Y",
    /// "what's my Z". Goes to memory.
    Memory,
    /// Multi-step desktop work: "go to youtube, search for X, play the
    /// top result, then fullscreen". Goes to the existing agent_loop.
    Agent,
    /// Reject class: out-of-distribution or garbled input the router should not
    /// act on. Only the local routelet classifier emits this (the Claude
    /// classifier's tool enum stays the five real intents). The orchestrator
    /// treats a `None` prediction as "defer to Claude" rather than routing it.
    None,
}

impl Intent {
    /// Parse a category string into the enum. Used by both the LLM classifier
    /// (Claude tool call output) and the local routelet ONNX classifier.
    /// Returns `Option::None` for any string that isn't one of the known labels;
    /// the recognized labels include "none", the routelet reject class.
    pub(crate) fn from_str(s: &str) -> Option<Self> {
        match s {
            "find_action" => Some(Self::FindAction),
            "integration" => Some(Self::Integration),
            "chat" => Some(Self::Chat),
            "memory" => Some(Self::Memory),
            "agent" => Some(Self::Agent),
            "none" => Some(Self::None),
            _ => Option::None,
        }
    }

    /// Inverse of `from_str`: returns the canonical label string for the intent.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::FindAction => "find_action",
            Self::Integration => "integration",
            Self::Chat => "chat",
            Self::Memory => "memory",
            Self::Agent => "agent",
            Self::None => "none",
        }
    }
}

impl Claude {
    /// Classify a voice transcript into one of the five intents. Single
    /// Haiku call, forced tool use, ~1-token output. Total round-trip
    /// ~250-400ms after the prompt cache warms on first call.
    ///
    /// Returns Err on network or API failure. Returns Ok(None) if the
    /// API succeeded but the response was malformed (no tool call, or
    /// an unrecognized category string). Callers should fail loud on
    /// Ok(None) so we can diagnose classifier drift.
    pub async fn classify_intent(
        &self,
        transcript: &str,
    ) -> Result<Option<Intent>, Box<dyn std::error::Error + Send + Sync>> {
        let body = serde_json::json!({
            "model": "claude-haiku-4-5",
            "max_tokens": 80,
            "stream": true,
            "system": [
                {
                    "type": "text",
                    "text": classifier_system_prompt(),
                    "cache_control": { "type": "ephemeral" }
                }
            ],
            "tools": [
                {
                    "name": "classify",
                    "description": "Emit the single best category for the user's voice command.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "category": {
                                "type": "string",
                                "enum": ["find_action", "integration", "chat", "memory", "agent"]
                            }
                        },
                        "required": ["category"]
                    }
                }
            ],
            "tool_choice": { "type": "tool", "name": "classify" },
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

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            return Err(format!("classify_intent API error {}: {}", status, body_text).into());
        }

        // SSE stream: pull out the tool_use input. We only care about the
        // `category` field of the classify tool's input JSON.
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut in_tool_use = false;
        let mut tool_json_buffer = String::new();

        while let Some(chunk) = stream.next().await {
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
                    match event["type"].as_str() {
                        Some("content_block_start")
                            if event["content_block"]["type"].as_str() == Some("tool_use")
                                && event["content_block"]["name"].as_str() == Some("classify") =>
                        {
                            in_tool_use = true;
                            tool_json_buffer.clear();
                        }
                        Some("content_block_delta") => {
                            if in_tool_use
                                && event["delta"]["type"].as_str() == Some("input_json_delta")
                                && let Some(j) = event["delta"]["partial_json"].as_str()
                            {
                                tool_json_buffer.push_str(j);
                            }
                        }
                        Some("content_block_stop") => {
                            in_tool_use = false;
                        }
                        _ => {}
                    }
                }
            }
        }

        let elapsed = t_send.elapsed();
        if tool_json_buffer.is_empty() {
            eprintln!("[classifier] no tool call received in {:?}", elapsed);
            return Ok(None);
        }

        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&tool_json_buffer) else {
            eprintln!(
                "[classifier] could not parse tool input JSON ({:?}): {}",
                elapsed, tool_json_buffer
            );
            return Ok(None);
        };
        let Some(category) = parsed["category"].as_str() else {
            eprintln!(
                "[classifier] tool input missing `category` ({:?}): {}",
                elapsed, tool_json_buffer
            );
            return Ok(None);
        };
        let intent = Intent::from_str(category);
        eprintln!(
            "[classifier] {:?} → {:?} ({:?})",
            transcript, intent, elapsed
        );
        Ok(intent)
    }
}

/// The classifier's system prompt. Kept short and stable so prompt caching
/// is effective. Updating this string invalidates the cache.
fn classifier_system_prompt() -> &'static str {
    "You are a voice-command router for a desktop voice assistant. Read \
the user's transcript and pick ONE category by calling the `classify` \
tool. Never respond with plain text.\n\
\n\
Categories:\n\
- find_action: user wants the cursor to move to, or interact with, \
something visible on their screen RIGHT NOW. ANY \"where\" question \
about visible content counts (the cursor goes there): \"where is X\", \
\"where does it say X\", \"where on the page is X\", \"where on screen \
does it talk about X\", \"where can I find X\". Also: \"click X\", \
\"show me X\", \"find X\", \"point at X\", \"point to X\", \"press X\", \
\"select X\", \"type X\", \"scroll up/down\", \"navigate to X\". \
Default to find_action whenever the request is about LOCATING or \
INTERACTING with content the user can already see, even if the \
phrasing is conversational like \"where does it say...\" or \"can you \
show me where...\".\n\
- integration: user wants to use a connected service (Gmail, Spotify, \
GitHub, YouTube) without looking at the screen. Phrases like \"play \
<song>\", \"pause\", \"skip\", \"check my email\", \"what's my unread \
count\", \"my open PRs\", \"play <youtube video>\".\n\
- chat: conversational or informational, no screen access needed, no \
service action. Phrases like \"what's your name\", \"how are you\", \
\"explain X\", \"what is X\", \"tell me about Y\". Default category for \
ambiguous questions.\n\
- memory: user wants to remember a fact about themselves OR recall one \
they told you before. Phrases like \"remember my X is Y\", \"my Z is \
Q\", \"what did I tell you about R\", \"what's my W\".\n\
- agent: explicit multi-step task chaining several actions together. \
Phrases like \"open spotify and play X then check email\", \"go to \
youtube, search for X, and play the top result\". Use only when the \
request clearly contains 2+ chained actions; otherwise prefer \
find_action or integration.\n\
\n\
Be decisive. If the request fits more than one category, pick the most \
specific one. Always call the tool. Never refuse to classify."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_str_round_trip() {
        assert_eq!(Intent::from_str("find_action"), Some(Intent::FindAction));
        assert_eq!(Intent::from_str("integration"), Some(Intent::Integration));
        assert_eq!(Intent::from_str("chat"), Some(Intent::Chat));
        assert_eq!(Intent::from_str("memory"), Some(Intent::Memory));
        assert_eq!(Intent::from_str("agent"), Some(Intent::Agent));
    }

    #[test]
    fn from_str_unknown_returns_none() {
        assert_eq!(Intent::from_str(""), None);
        assert_eq!(Intent::from_str("Find_Action"), None); // case-sensitive on purpose
        assert_eq!(Intent::from_str("garbage"), None);
    }

    #[test]
    fn as_str_round_trips_from_str() {
        for intent in [
            Intent::FindAction,
            Intent::Integration,
            Intent::Chat,
            Intent::Memory,
            Intent::Agent,
        ] {
            assert_eq!(Intent::from_str(intent.as_str()), Some(intent));
        }
    }
}
