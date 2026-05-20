//! The multi-step agent loop: streams Claude responses, dispatches tool
//! calls back through the caller's callbacks, captures fresh screenshots
//! between iterations, and accumulates the final spoken text.

use super::parsing::{parse_tool_call, tools_array_value, trim_old_screenshots};
use super::prompt::system_prompt_for_actions;
use super::{Action, Claude};
use crate::screenshot::pick_declared_resolution;
use futures_util::StreamExt;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

impl Claude {
    /// Multi-step agent loop. Each iteration calls Claude with the running
    /// message history, executes any tool_use blocks via `on_action`, waits
    /// SETTLE_MS for the UI to settle, captures a fresh screenshot via
    /// `take_screenshot`, appends the assistant turn + a user tool_result
    /// turn (containing the new screenshot), and recurses. Exits when
    /// Claude returns a response with no tool calls (text-only answer),
    /// when MAX_STEPS is reached, or when the future is dropped by an
    /// outer barge-in select.
    ///
    /// Returns the final text content from the last iteration. voice.rs
    /// pipes mid-chain text deltas to TTS via `on_text_delta` and treats
    /// the final return value as the spoken summary.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_agent_loop<F, S, D, T>(
        &self,
        prompt: &str,
        initial_screenshot_b64: &str,
        running_apps: &[String],
        user_email: Option<&str>,
        window_x: i64,
        window_y: i64,
        window_width: i64,
        window_height: i64,
        integration_tools: Vec<serde_json::Value>,
        early_exit: CancellationToken,
        mut take_screenshot: S,
        mut on_action: F,
        mut dispatch_integration: D,
        mut on_text_delta: T,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnMut(Action),
        S: FnMut() -> Result<String, Box<dyn std::error::Error + Send + Sync>>,
        D: FnMut(&str, &serde_json::Value) -> Option<String>,
        T: FnMut(&str),
    {
        use crate::tuning::{
            AGENT_KEEP_RECENT_SCREENSHOTS as KEEP_RECENT_SCREENSHOTS,
            AGENT_MAX_STEPS as MAX_STEPS, AGENT_SETTLE_MS as SETTLE_MS,
        };

        let (declared_w, declared_h) = pick_declared_resolution(window_width, window_height);

        // Build initial user turn: screenshot + transcript prompt.
        let running_list = if running_apps.is_empty() {
            "(none detected)".to_string()
        } else {
            running_apps.join(", ")
        };
        let user_prompt = format!(
            "The user said: \"{}\". \n\n\
             Currently-running app window classes (from Hyprland): {}.\n\n\
             Tool preference order for actions targeting an app:\n\
             1. SERVICE-SPECIFIC INTEGRATION TOOLS FIRST (e.g. spotify_play, \
                spotify_pause, gmail_search, gmail_read, gmail_send, \
                gmail_unread_count). These let you access the service's \
                data and actions directly via API, regardless of what is \
                visible on screen. Dramatically faster than visual \
                automation: one tool call vs. 5-10 steps of click+type. \
                If a gmail_ or spotify_ tool exists for what the user is \
                asking for, USE IT, even if the screen shows something \
                unrelated like a terminal. Do NOT tell the user you cannot \
                access their email/music when these tools are available; \
                just call the tool.\n\
             2. If no integration tool exists and the target app IS in the \
                running list above, prefer switch_to_window to focus it and \
                interact via click+type.\n\
             3. If no integration tool exists and the app is NOT running, \
                use launch_app to start it (or open_url for web services).\n\
             4. open_url is for pure web destinations: sites without a \
                desktop app, or when the user explicitly says \"in the browser\".\n\n\
             Pick the best action(s) and invoke their tools. If the request \
             needs multiple steps, call multiple tools across iterations \
             (you'll get a fresh screenshot after each batch). When the task \
             is fully done, respond with plain text and no tool calls to end \
             the chain.",
            prompt, running_list
        );
        // Build initial user content. Skip the image block when no screenshot
        // was passed in (integration-keyword queries don't need vision and
        // benefit from a much smaller request body).
        let mut initial_content: Vec<serde_json::Value> = vec![];
        if !initial_screenshot_b64.is_empty() {
            initial_content.push(serde_json::json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": "image/jpeg",
                    "data": initial_screenshot_b64,
                },
            }));
        }
        initial_content.push(serde_json::json!({ "type": "text", "text": user_prompt }));
        let mut messages: Vec<serde_json::Value> = vec![serde_json::json!({
            "role": "user",
            "content": initial_content,
        })];

        let mut final_text = String::new();

        // Build the system prompt once per turn. If the caller passed the
        // user's Gmail address (resolved at startup by the gmail integration
        // and cached), append it so Claude can resolve "send X to me" /
        // "email myself" without asking. Allocating one String per turn
        // (not per step) keeps prompt caching effective.
        let system: String = match user_email {
            Some(email) => format!(
                "{}\n\nThe user's own Gmail address is {email}. When the user says \
                 'me' / 'myself' / 'send to me' / 'email myself' in an email context, \
                 this is the recipient. Never ask the user for their email address.",
                system_prompt_for_actions()
            ),
            None => system_prompt_for_actions().to_string(),
        };

        for step in 0..MAX_STEPS {
            // First-feedback short circuit: a previous step has already
            // either fired a visible cursor action or pushed the first PCM
            // chunk to the speaker, so the user is already getting feedback.
            // Don't start another round trip on top of that.
            if early_exit.is_cancelled() {
                eprintln!(
                    "[agent-loop] early exit before step {} (first feedback fired)",
                    step + 1
                );
                break;
            }

            let t_step_start = std::time::Instant::now();
            eprintln!(
                "[agent-loop] step {}/{} starting (messages history: {} turns)",
                step + 1,
                MAX_STEPS,
                messages.len()
            );

            let t_build = std::time::Instant::now();
            let tools_value = tools_array_value(declared_w, declared_h, integration_tools.clone());
            let body = serde_json::json!({
                "model": "claude-haiku-4-5",
                "max_tokens": 1024,
                "stream": true,
                "system": system,
                "tools": tools_value.clone(),
                "messages": messages,
            });
            let body_size_kb = serde_json::to_vec(&body)
                .map(|v| v.len() / 1024)
                .unwrap_or(0);

            // Hash the cacheable prefix (system + tools) so we can spot when
            // it varies between turns. If the hash changes turn-to-turn,
            // prompt caching can't hit. Should be identical across an
            // entire session. If not, something is leaking into the
            // prefix that shouldn't be.
            let cache_prefix_hash = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                serde_json::to_vec(&system).unwrap_or_default().hash(&mut h);
                serde_json::to_vec(&tools_value).unwrap_or_default().hash(&mut h);
                h.finish()
            };
            eprintln!(
                "[agent-loop] step {} body built ({} KB, prefix_hash=0x{:x}) in {:?}",
                step + 1,
                body_size_kb,
                cache_prefix_hash,
                t_build.elapsed()
            );

            let t_send = std::time::Instant::now();
            let response = self
                .http
                .post(&self.endpoint)
                .header(&self.auth.0, &self.auth.1)
                .header("anthropic-version", "2023-06-01")
                .header("anthropic-beta", "computer-use-2025-01-24")
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await?;
            eprintln!(
                "[agent-loop] step {} upload + response headers → {:?}",
                step + 1,
                t_send.elapsed()
            );

            if !response.status().is_success() {
                let status = response.status();
                let body_text = response.text().await.unwrap_or_default();
                return Err(format!("Claude API error {}: {}", status, body_text).into());
            }

            // Parse streaming response. Collect tool_use blocks (with their
            // ids so we can pair them with tool_results next iteration) and
            // any free text. Fire on_action for each parsed tool the moment
            // its input JSON completes.
            let t_stream_start = std::time::Instant::now();
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut text_content = String::new();
            let mut tool_calls: Vec<(String, String, String)> = vec![]; // (id, name, input_json)
            let mut current_tool_name: Option<String> = None;
            let mut current_tool_id: Option<String> = None;
            let mut tool_json_buffer = String::new();
            let mut first_byte_logged = false;

            while let Some(chunk) = stream.next().await {
                if !first_byte_logged {
                    eprintln!(
                        "[agent-loop] step {} first SSE byte → {:?} (Claude TTFT)",
                        step + 1,
                        t_stream_start.elapsed()
                    );
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

                        match event["type"].as_str() {
                            Some("message_start") => {
                                eprintln!(
                                    "[sse-debug] step {} message_start: model={:?} usage={}",
                                    step + 1,
                                    event["message"]["model"].as_str().unwrap_or("?"),
                                    event["message"]["usage"]
                                );
                            }
                            Some("message_delta") => {
                                eprintln!(
                                    "[sse-debug] step {} message_delta: stop_reason={:?} stop_sequence={:?} usage={}",
                                    step + 1,
                                    event["delta"]["stop_reason"].as_str(),
                                    event["delta"]["stop_sequence"].as_str(),
                                    event["usage"]
                                );
                            }
                            Some("error") => {
                                eprintln!(
                                    "[sse-debug] step {} ERROR event: {}",
                                    step + 1,
                                    event
                                );
                            }
                            Some("content_block_start") => {
                                if event["content_block"]["type"].as_str() == Some("tool_use") {
                                    current_tool_name =
                                        event["content_block"]["name"].as_str().map(str::to_string);
                                    current_tool_id =
                                        event["content_block"]["id"].as_str().map(str::to_string);
                                    tool_json_buffer.clear();
                                } else {
                                    current_tool_name = None;
                                    current_tool_id = None;
                                }
                            }
                            Some("content_block_delta") => {
                                let delta_type = event["delta"]["type"].as_str();
                                if delta_type == Some("input_json_delta") {
                                    if let Some(j) = event["delta"]["partial_json"].as_str() {
                                        tool_json_buffer.push_str(j);
                                    }
                                } else if delta_type == Some("text_delta")
                                    && let Some(t) = event["delta"]["text"].as_str() {
                                        text_content.push_str(t);
                                        // The orchestrator pipes each delta
                                        // through StreamHelper for
                                        // sentence-boundary detection before
                                        // pushing to TTS.
                                        on_text_delta(t);
                                    }
                            }
                            Some("message_stop") | Some("ping") => {
                                // Expected events with no action required.
                            }
                            Some("content_block_stop") => {
                                if let (Some(name), Some(id)) =
                                    (current_tool_name.take(), current_tool_id.take())
                                {
                                    // No-arg tools (spotify_next, gmail_unread_count,
                                    // etc.) emit no input_json_delta events, so the
                                    // buffer is empty at stop. Treat that as "{}" so
                                    // they aren't silently dropped.
                                    let input_json = if tool_json_buffer.is_empty() {
                                        "{}".to_string()
                                    } else {
                                        tool_json_buffer.clone()
                                    };
                                    if let Ok(input) =
                                        serde_json::from_str::<serde_json::Value>(&input_json)
                                    {
                                        match parse_tool_call(
                                            &name,
                                            &input,
                                            declared_w,
                                            declared_h,
                                            window_x,
                                            window_y,
                                            window_width,
                                            window_height,
                                        ) {
                                            // Integration tools are dispatched post-stream
                                            // so their text results can feed back to Claude
                                            // as tool_result content. Skip on_action here.
                                            Some(Action::Integration) => {}
                                            Some(action) => on_action(action),
                                            None => {
                                                let action_field = input["action"]
                                                    .as_str()
                                                    .unwrap_or("(none)");
                                                eprintln!(
                                                    "[agent-loop] unhandled tool '{}' action='{}' input={}",
                                                    name, action_field, input_json
                                                );
                                            }
                                        }
                                        tool_calls.push((id, name, input_json));
                                    }
                                    tool_json_buffer.clear();
                                }
                            }
                            other => {
                                eprintln!(
                                    "[sse-debug] step {} unknown event type {:?}: {}",
                                    step + 1,
                                    other,
                                    event
                                );
                            }
                        }
                    }
                }
            }

            eprintln!(
                "[agent-loop] step {} stream complete → {:?} ({} tool(s), {} text chars)",
                step + 1,
                t_stream_start.elapsed(),
                tool_calls.len(),
                text_content.len()
            );

            final_text = text_content.clone();

            // Exit condition: no tool calls means Claude is done.
            if tool_calls.is_empty() {
                eprintln!(
                    "[agent-loop] step {} returned text-only ({} chars), exiting after {:?}",
                    step + 1,
                    final_text.len(),
                    t_step_start.elapsed()
                );
                break;
            }

            // Append the assistant turn that just streamed.
            let t_append = std::time::Instant::now();
            let mut assistant_content: Vec<serde_json::Value> = vec![];
            if !text_content.is_empty() {
                assistant_content.push(serde_json::json!({ "type": "text", "text": text_content }));
            }
            for (id, name, input_json) in &tool_calls {
                let input: serde_json::Value =
                    serde_json::from_str(input_json).unwrap_or_else(|_| serde_json::json!({}));
                assistant_content.push(serde_json::json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": input,
                }));
            }
            messages.push(serde_json::json!({ "role": "assistant", "content": assistant_content }));
            eprintln!(
                "[agent-loop] step {} assistant turn appended in {:?}",
                step + 1,
                t_append.elapsed()
            );

            // Dispatch integration tools first so we know whether the post-step
            // screenshot is even needed. Non-integration tools (computer,
            // open_url, etc.) already fired via on_action during streaming.
            let t_tail = std::time::Instant::now();
            let mut integration_results: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            for (id, name, input_json) in &tool_calls {
                let Ok(input) = serde_json::from_str::<serde_json::Value>(input_json) else {
                    continue;
                };
                let t_disp = std::time::Instant::now();
                eprintln!(
                    "[tool-call] step {} dispatch '{}' input={}",
                    step + 1,
                    name,
                    input_json
                );
                if let Some(result) = dispatch_integration(name, &input) {
                    let preview: String = result.chars().take(120).collect();
                    eprintln!(
                        "[tool-call] step {} '{}' returned {} chars in {:?} | preview: {}{}",
                        step + 1,
                        name,
                        result.len(),
                        t_disp.elapsed(),
                        preview,
                        if result.len() > 120 { "..." } else { "" }
                    );
                    integration_results.insert(id.clone(), result);
                } else {
                    eprintln!(
                        "[tool-call] step {} '{}' was NOT an integration (handled elsewhere) in {:?}",
                        step + 1,
                        name,
                        t_disp.elapsed()
                    );
                }
            }

            // If every tool this step was an integration tool, the screen
            // didn't change. Skip settle + screenshot capture and reuse the
            // last screenshot string for tool_result fallback (it won't
            // actually be referenced since all results are text).
            let all_integration = !tool_calls.is_empty()
                && tool_calls
                    .iter()
                    .all(|(id, _, _)| integration_results.contains_key(id));

            let new_screenshot: String = if all_integration {
                eprintln!(
                    "[agent-loop] step {} skipped settle + screenshot (all integration tools)",
                    step + 1
                );
                String::new()
            } else {
                let t_settle = std::time::Instant::now();
                tokio::time::sleep(Duration::from_millis(SETTLE_MS)).await;
                eprintln!(
                    "[agent-loop] step {} settle ({}ms) → {:?} actual",
                    step + 1,
                    SETTLE_MS,
                    t_settle.elapsed()
                );
                let t_shot = std::time::Instant::now();
                let shot = take_screenshot()?;
                eprintln!(
                    "[agent-loop] step {} screenshot captured ({} KB) in {:?}",
                    step + 1,
                    shot.len() / 1024,
                    t_shot.elapsed()
                );
                shot
            };

            // Append tool_results. Integration tools get their text result;
            // all other tools get the post-action screenshot so Claude can
            // see what changed on screen.
            let tool_results: Vec<serde_json::Value> = tool_calls
                .iter()
                .map(|(id, _, _)| {
                    if let Some(text) = integration_results.get(id) {
                        serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": id,
                            "content": text,
                        })
                    } else {
                        serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": id,
                            "content": [
                                { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": new_screenshot } }
                            ]
                        })
                    }
                })
                .collect();
            messages.push(serde_json::json!({ "role": "user", "content": tool_results }));

            // Linear token cost grows fast if we ship every screenshot
            // forever. Strip image data from older tool_results, keeping
            // only the N most recent. After an integration-only step the
            // visual context is dead weight (Claude is working with API
            // results, not pixels), so drop ALL prior screenshots in that
            // case. Cuts subsequent request size from ~270KB to a few KB.
            let keep = if all_integration { 0 } else { KEEP_RECENT_SCREENSHOTS };
            trim_old_screenshots(&mut messages, keep);

            eprintln!(
                "[agent-loop] step {} tool_result + trim in {:?}",
                step + 1,
                t_tail.elapsed()
            );

            eprintln!(
                "[agent-loop] step {} TOTAL {:?}",
                step + 1,
                t_step_start.elapsed()
            );
        }

        Ok(final_text)
    }
}
