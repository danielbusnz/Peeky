//! Integration tool dispatcher. Used when the classifier returns
//! Intent::Integration (the user wants to use a connected service like
//! Gmail, Spotify, GitHub, or YouTube and doesn't need the screen).
//!
//! Differences from `run_agent_loop`:
//!   * Tools array is ONLY integration tools, no computer/open_url/etc.
//!     Claude can't accidentally try to find a button on screen when the
//!     user said "play despacito."
//!   * No screenshot. Saves ~270KB upload + ~1500 input tokens.
//!   * Two-step: first request picks the tool, we dispatch it inline,
//!     second request lets Claude compose a spoken summary from the
//!     result. Most integration replies are short ("3 unread", "now
//!     playing X by Y") so the summary stays fast.
//!   * tool_choice: any on the first call. Claude must call a tool
//!     (can't say "I'd love to but..."). Falls back to chat path if we
//!     somehow get a non-tool response.
//!
//! `dispatch` is the caller's integration-registry hook. We pass it the
//! tool name + input JSON and it returns the tool result string.

use super::Claude;
use futures_util::StreamExt;

impl Claude {
    /// Run an integration turn. Picks one integration tool, dispatches
    /// it, then has Claude compose a short spoken summary streamed
    /// through `on_text_delta`. Returns the full final text.
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

        // Step 1: pick a tool. tool_choice: any → claude MUST call one.
        let pick_body = serde_json::json!({
            "model": "claude-haiku-4-5",
            "max_tokens": 500,
            "stream": true,
            "system": [system_block.clone()],
            "tools": integration_tools.clone(),
            "tool_choice": { "type": "any" },
            "messages": [
                { "role": "user", "content": transcript }
            ]
        });

        let t_pick = std::time::Instant::now();
        let pick_response = self
            .apply_auth(self.http.post(&self.endpoint))
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&pick_body)
            .send()
            .await?;

        if !pick_response.status().is_success() {
            let status = pick_response.status();
            let body_text = pick_response.text().await.unwrap_or_default();
            crate::upgrade::on_proxy_error(status.as_u16(), &body_text);
            return Err(format!("integration pick API error {}: {}", status, body_text).into());
        }

        let mut stream = pick_response.bytes_stream();
        let mut buffer = String::new();
        let mut tool_name: Option<String> = None;
        let mut tool_id: Option<String> = None;
        let mut tool_json_buffer = String::new();
        let mut current_block_is_tool = false;

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
                            if event["content_block"]["type"].as_str() == Some("tool_use") {
                                current_block_is_tool = true;
                                if tool_name.is_none() {
                                    tool_name =
                                        event["content_block"]["name"].as_str().map(str::to_string);
                                    tool_id =
                                        event["content_block"]["id"].as_str().map(str::to_string);
                                }
                                tool_json_buffer.clear();
                            } else {
                                current_block_is_tool = false;
                            }
                        }
                        Some("content_block_delta") => {
                            if current_block_is_tool
                                && event["delta"]["type"].as_str() == Some("input_json_delta")
                                && let Some(j) = event["delta"]["partial_json"].as_str()
                            {
                                tool_json_buffer.push_str(j);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        eprintln!(
            "[integration] tool picked → {:?} ({:?})",
            tool_name,
            t_pick.elapsed()
        );

        let (tool_name, tool_id) = match (tool_name, tool_id) {
            (Some(n), Some(i)) => (n, i),
            _ => {
                return Err("integration: claude did not emit a tool call".into());
            }
        };
        let tool_input: serde_json::Value = if tool_json_buffer.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&tool_json_buffer).unwrap_or(serde_json::json!({}))
        };

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

        // Step 2: compose a short spoken summary. Pass the tool call and
        // its result back to Claude. No tools on this step (we want a
        // text answer, not another tool call).
        let summary_body = serde_json::json!({
            "model": "claude-haiku-4-5",
            "max_tokens": 1024,
            "stream": true,
            "system": [system_block],
            "tools": integration_tools,
            "messages": [
                { "role": "user", "content": transcript },
                {
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": tool_id,
                        "name": tool_name,
                        "input": tool_input
                    }]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tool_id,
                        "content": result
                    }]
                }
            ]
        });

        let t_summary = std::time::Instant::now();
        let summary_response = self
            .apply_auth(self.http.post(&self.endpoint))
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&summary_body)
            .send()
            .await?;

        if !summary_response.status().is_success() {
            let status = summary_response.status();
            let body_text = summary_response.text().await.unwrap_or_default();
            crate::upgrade::on_proxy_error(status.as_u16(), &body_text);
            return Err(format!("integration summary API error {}: {}", status, body_text).into());
        }

        let mut stream = summary_response.bytes_stream();
        let mut buffer = String::new();
        let mut text_content = String::new();

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
            "[integration] summary stream complete → {:?} ({} chars)",
            t_summary.elapsed(),
            text_content.len()
        );

        Ok(text_content)
    }
}

/// Integration system prompt. Aware of the user_profile when present so
/// "send to me" / "my repo" can resolve without asking.
fn integration_system_prompt(user_profile: Option<&str>) -> String {
    let base = "You are aegis, a voice assistant that operates connected \
services (Gmail, Spotify, GitHub, YouTube) on behalf of the user via \
tool calls. The user is speaking to you and hearing your replies via \
TTS, so:\n\
- After the tool result comes back, compose a short spoken summary. \
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
