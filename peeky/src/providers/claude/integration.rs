//! Integration tool dispatcher. Used when the classifier returns
//! Intent::Integration (the user wants to use a connected service like
//! Gmail, Spotify, GitHub, or YouTube and doesn't need the screen).
//!
//! Differences from `run_agent_loop`:
//!   * Tools array is ONLY integration tools, no computer/open_url/etc.
//!     Claude can't accidentally try to find a button on screen when the
//!     user said "play despacito."
//!   * No screenshot. Saves ~270KB upload + ~1500 input tokens.
//!   * Bounded tool loop: up to `INTEGRATION_MAX_TOOL_CALLS` dispatches,
//!     then a forced text-only summary. The old shape was exactly one
//!     dispatch + summary, which silently dropped follow-up calls:
//!     "open that lease pdf" would spotlight_search, narrate "opening it
//!     now", and never call finder_open. Tools whose descriptions promise
//!     a follow-up (spotlight_search → finder_open) need the chain.
//!   * tool_choice: any on the first call. Claude must call a tool
//!     (can't say "I'd love to but..."). Later calls are auto: another
//!     tool continues the chain, plain text ends the turn as the spoken
//!     summary. Narration text alongside a tool call is streamed to TTS
//!     as it arrives, so the user hears progress on multi-call turns.
//!
//! `dispatch` is the caller's integration-registry hook. We pass it the
//! tool name + input JSON and it returns the tool result string.

use super::Claude;
use futures_util::StreamExt;

/// One streamed model response: the narration text (already forwarded to
/// the caller's TTS sink) and the first tool call, when the model made one.
struct Round {
    text: String,
    tool: Option<(String, String, serde_json::Value)>, // (name, id, input)
}

impl Claude {
    /// Run an integration turn: a bounded chain of tool dispatches ending
    /// in a short spoken summary streamed through `on_text_delta`.
    /// Returns the full spoken text.
    pub async fn integration<D, T>(
        &self,
        transcript: &str,
        integration_tools: Vec<serde_json::Value>,
        user_profile: Option<&str>,
        mut dispatch: D,
        mut on_text_delta: T,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>
    where
        D: FnMut(&str, &serde_json::Value) -> Option<String>,
        T: FnMut(&str),
    {
        if integration_tools.is_empty() {
            return Err("integration() called with empty tools list".into());
        }

        let system_block = serde_json::json!({
            "type": "text",
            "text": integration_system_prompt(user_profile),
            "cache_control": { "type": "ephemeral" }
        });

        let mut messages = vec![serde_json::json!({ "role": "user", "content": transcript })];
        let mut spoken = String::new();

        for call in 1..=crate::tuning::INTEGRATION_MAX_TOOL_CALLS {
            // First call must pick a tool; later calls may chain another
            // tool or finish with the spoken summary.
            let tool_choice = if call == 1 {
                serde_json::json!({ "type": "any" })
            } else {
                serde_json::json!({ "type": "auto" })
            };
            let body = serde_json::json!({
                "model": "claude-haiku-4-5",
                "max_tokens": 1024,
                "stream": true,
                "system": [system_block.clone()],
                "tools": integration_tools.clone(),
                "tool_choice": tool_choice,
                "messages": messages
            });

            let t_round = std::time::Instant::now();
            let round = self.integration_round(body, &mut on_text_delta).await?;
            spoken.push_str(&round.text);

            let Some((tool_name, tool_id, tool_input)) = round.tool else {
                // Text-only response: the chain is done, this was the summary.
                eprintln!(
                    "[integration] summary after {} tool call(s) → {:?} ({} chars)",
                    call - 1,
                    t_round.elapsed(),
                    round.text.len()
                );
                return Ok(spoken);
            };
            eprintln!(
                "[integration] call {} picked '{}' ({:?})",
                call,
                tool_name,
                t_round.elapsed()
            );

            // Dispatch the integration. Result is a short string the caller's
            // handler produces (e.g. "3 unread", "now playing X by Y").
            let t_disp = std::time::Instant::now();
            let result = dispatch(&tool_name, &tool_input).unwrap_or_else(|| {
                format!(
                    "integration tool '{}' has no handler; tool input was {}",
                    tool_name, tool_input
                )
            });
            eprintln!(
                "[integration] dispatch '{}' → {} chars in {:?}",
                tool_name,
                result.len(),
                t_disp.elapsed()
            );

            // Append the assistant turn (any narration + the tool call) and
            // the tool result, then loop for the next call or the summary.
            let mut content = Vec::new();
            if !round.text.trim().is_empty() {
                content.push(serde_json::json!({ "type": "text", "text": round.text }));
            }
            content.push(serde_json::json!({
                "type": "tool_use",
                "id": tool_id,
                "name": tool_name,
                "input": tool_input
            }));
            messages.push(serde_json::json!({ "role": "assistant", "content": content }));
            messages.push(serde_json::json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": tool_id,
                    "content": result
                }]
            }));
        }

        // Dispatch budget spent with the model still chaining: force a
        // text-only summary so the turn always ends in speech.
        let body = serde_json::json!({
            "model": "claude-haiku-4-5",
            "max_tokens": 1024,
            "stream": true,
            "system": [system_block],
            "tools": integration_tools,
            "tool_choice": { "type": "none" },
            "messages": messages
        });
        let t_summary = std::time::Instant::now();
        let round = self.integration_round(body, &mut on_text_delta).await?;
        spoken.push_str(&round.text);
        eprintln!(
            "[integration] forced summary at max tool calls → {:?} ({} chars)",
            t_summary.elapsed(),
            round.text.len()
        );
        Ok(spoken)
    }

    /// POST one streaming request. Text deltas are forwarded to
    /// `on_text_delta` as they arrive; the first tool_use block (if any) is
    /// captured and returned. Extra tool blocks in the same response are
    /// ignored, matching the old single-pick behavior.
    async fn integration_round<T>(
        &self,
        body: serde_json::Value,
        on_text_delta: &mut T,
    ) -> Result<Round, Box<dyn std::error::Error + Send + Sync>>
    where
        T: FnMut(&str),
    {
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
            crate::upgrade::on_proxy_error(status.as_u16(), &body_text);
            return Err(format!("integration API error {}: {}", status, body_text).into());
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut text = String::new();
        let mut tool_name: Option<String> = None;
        let mut tool_id: Option<String> = None;
        let mut tool_json = String::new();
        // Which block the parser is inside: only the FIRST tool block's
        // input json is captured; text deltas stream out from text blocks.
        let mut in_captured_tool_block = false;
        let mut in_text_block = false;

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
                        Some("content_block_start") => {
                            let is_tool =
                                event["content_block"]["type"].as_str() == Some("tool_use");
                            in_text_block = !is_tool;
                            in_captured_tool_block = is_tool && tool_name.is_none();
                            if in_captured_tool_block {
                                tool_name =
                                    event["content_block"]["name"].as_str().map(str::to_string);
                                tool_id = event["content_block"]["id"].as_str().map(str::to_string);
                                tool_json.clear();
                            }
                        }
                        Some("content_block_delta") => {
                            if in_captured_tool_block
                                && event["delta"]["type"].as_str() == Some("input_json_delta")
                                && let Some(j) = event["delta"]["partial_json"].as_str()
                            {
                                tool_json.push_str(j);
                            } else if in_text_block
                                && event["delta"]["type"].as_str() == Some("text_delta")
                                && let Some(t) = event["delta"]["text"].as_str()
                            {
                                text.push_str(t);
                                on_text_delta(t);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        let tool = match (tool_name, tool_id) {
            (Some(name), Some(id)) => {
                let input: serde_json::Value = if tool_json.is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::from_str(&tool_json).unwrap_or(serde_json::json!({}))
                };
                Some((name, id, input))
            }
            _ => None,
        };
        Ok(Round { text, tool })
    }
}

/// Integration system prompt. Aware of the user_profile when present so
/// "send to me" / "my repo" can resolve without asking.
fn integration_system_prompt(user_profile: Option<&str>) -> String {
    let base = "You are peeky, a voice assistant that operates connected \
services (Gmail, Spotify, GitHub, YouTube) on behalf of the user via \
tool calls. The user is speaking to you and hearing your replies via \
TTS, so:\n\
- Chain tool calls when the task needs more than one (e.g. find a file, \
then open it). Finish the task before summarizing.\n\
- After the last tool result, compose a short spoken summary. \
1-2 sentences. Plain prose, no markdown.\n\
- Confirm what you did or report what you found. Don't restate the \
request.\n\
- If the tool result is an error, say what went wrong briefly, not the \
raw error message.\n\
- The user can't see the screen here. Translate any technical details \
into something natural to hear.";
    match user_profile.filter(|p| !p.trim().is_empty()) {
        Some(profile) => format!(
            "{}\n\nUser profile (facts the user told you to remember):\n{}",
            base, profile
        ),
        None => base.to_string(),
    }
}
