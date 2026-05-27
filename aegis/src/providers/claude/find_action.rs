//! Single-shot visual action dispatcher. Used when the classifier returns
//! Intent::FindAction (the user wants the cursor to move, a click to
//! fire, text to be typed, an app to launch, or a URL to open).
//!
//! Differences from `run_agent_loop`:
//!   * Forces a tool call via `tool_choice: { type: any }` so Claude can
//!     never respond with text-only ("the button is at 645,719"). This
//!     was the bug in the unified path: same query, sometimes Point
//!     action, sometimes text. Forced tool use eliminates the variance.
//!   * Smaller tool set (no gmail/spotify/github/youtube). The classifier
//!     already routed those to the Integration path; including them here
//!     would just tempt Claude away from the cursor tool.
//!   * Short, decisive system prompt: "pick one tool, no preamble".
//!   * No agent loop. Single Claude call, action fires, we're done.
//!
//! `on_action` fires the moment Claude finishes streaming the tool's
//! input JSON, so the cursor can move while we're still receiving bytes.

use super::parsing::parse_tool_call;
use super::{Action, Claude};
use crate::screenshot::pick_declared_resolution;
use futures_util::StreamExt;

impl Claude {
    /// Pick a single cursor action for a visual query.
    ///
    /// Returns `Some(Action)` if Claude picked a tool we know how to
    /// dispatch, `None` if Claude emitted a tool we couldn't parse
    /// (shouldn't happen with the tool list we send). Network or API
    /// errors return Err.
    #[allow(clippy::too_many_arguments)]
    pub async fn find_action<F>(
        &self,
        prompt: &str,
        image_b64: &str,
        window_x: i64,
        window_y: i64,
        window_width: i64,
        window_height: i64,
        mut on_action: F,
    ) -> Result<Option<Action>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnMut(Action),
    {
        let (declared_w, declared_h) = pick_declared_resolution(window_width, window_height);

        let user_prompt = format!(
            "The user said: \"{}\". Pick the single best tool for this request \
             and call it. Skip directly to the tool call. No preamble, no \
             description.",
            prompt
        );

        let body = serde_json::json!({
            "model": "claude-haiku-4-5",
            "max_tokens": 500,
            "stream": true,
            "system": [
                {
                    "type": "text",
                    "text": find_action_system_prompt(),
                    "cache_control": { "type": "ephemeral" }
                }
            ],
            "tools": find_action_tools(declared_w, declared_h),
            // Force Claude to call SOME tool (any of the ones we provided).
            // No text-only responses. That's the whole point of this path.
            "tool_choice": { "type": "any" },
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/jpeg",
                            "data": image_b64
                        }
                    },
                    { "type": "text", "text": user_prompt }
                ]
            }]
        });

        let t_send = std::time::Instant::now();
        let response = self
            .apply_auth(self.http.post(&self.endpoint))
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "computer-use-2025-01-24")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;
        eprintln!(
            "[find_action] upload + response headers → {:?}",
            t_send.elapsed()
        );

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            return Err(format!("find_action API error {}: {}", status, body_text).into());
        }

        // Stream parse: extract the single tool_use that Claude was forced
        // to emit. We only care about the first one. The prompt says
        // "single best tool," so multiple calls in one response would be a
        // model misbehavior we'd want to see in logs but not act on twice.
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut current_tool_name: Option<String> = None;
        let mut tool_json_buffer = String::new();
        let mut emitted: Option<Action> = None;
        let mut first_byte_logged = false;
        let t_stream_start = std::time::Instant::now();

        while let Some(chunk) = stream.next().await {
            if !first_byte_logged {
                eprintln!(
                    "[find_action] first SSE byte → {:?}",
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
                        Some("content_block_start") => {
                            if event["content_block"]["type"].as_str() == Some("tool_use") {
                                current_tool_name =
                                    event["content_block"]["name"].as_str().map(str::to_string);
                                tool_json_buffer.clear();
                            } else {
                                current_tool_name = None;
                            }
                        }
                        Some("content_block_delta") => {
                            if event["delta"]["type"].as_str() == Some("input_json_delta")
                                && let Some(j) = event["delta"]["partial_json"].as_str()
                            {
                                tool_json_buffer.push_str(j);
                            }
                        }
                        Some("content_block_stop") => {
                            if emitted.is_none()
                                && let Some(name) = current_tool_name.take()
                            {
                                let input_json = if tool_json_buffer.is_empty() {
                                    "{}".to_string()
                                } else {
                                    tool_json_buffer.clone()
                                };
                                if let Ok(input) =
                                    serde_json::from_str::<serde_json::Value>(&input_json)
                                {
                                    if let Some(action) = parse_tool_call(
                                        &name,
                                        &input,
                                        declared_w,
                                        declared_h,
                                        window_x,
                                        window_y,
                                        window_width,
                                        window_height,
                                    ) {
                                        on_action(action.clone());
                                        emitted = Some(action);
                                    } else {
                                        // Log why the action failed
                                        if name == "computer" && input["action"].as_str() == Some("screenshot") {
                                            eprintln!(
                                                "[find_action] ERROR: Claude called screenshot action (FORBIDDEN). \
                                                User asked: {:?}. Claude should have used mouse_move or left_click instead.",
                                                prompt
                                            );
                                        } else {
                                            eprintln!(
                                                "[find_action] unknown/invalid tool '{}' input={}. User asked: {:?}",
                                                name, input_json, prompt
                                            );
                                        }
                                    }
                                }
                                tool_json_buffer.clear();
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        eprintln!(
            "[find_action] stream complete → {:?}, emitted={:?}",
            t_stream_start.elapsed(),
            emitted
        );
        Ok(emitted)
    }
}

/// The find_action system prompt. Decisive, terse, instructs the model
/// that any descriptive text response is wrong. Cached.
fn find_action_system_prompt() -> &'static str {
    "You are a desktop voice-assistant action dispatcher. A screenshot of \
the user's screen is attached. You MUST emit exactly one tool call. \
Never respond with descriptive text. The user wants the cursor to \
MOVE or an action to FIRE, not to read coordinates or a description.\n\
\n\
Tool selection:\n\
- `computer` mouse_move(coordinate=[x,y]): user wants to SEE where \
something is on screen, NO click (\"where is X\", \"show me X\", \
\"find X\", \"point at X\"). Cursor moves visually, no input fires.\n\
- `computer` left_click(coordinate=[x,y]): user wants to actually CLICK \
something visible (\"click X\", \"press X\", \"select X\"). Cursor \
moves AND a real click fires.\n\
- `computer` type(text=\"...\"): type into the focused field. End with \
\\n if the user wants it submitted. For multi-step \"search for X\" \
queries, emit BOTH a left_click on the input AND a type with \\n.\n\
- `computer` key(text=\"...\"): press a key or combo (Return, Tab, \
Escape, ctrl+a, ctrl+f, etc.). Use for hotkeys.\n\
- `computer` scroll(scroll_direction=\"up\"|\"down\"|\"left\"|\"right\", \
scroll_amount=N): scroll the focused area.\n\
- `open_url`: navigate to a fully-qualified https:// URL.\n\
- `launch_app`: start an app that isn't running.\n\
- `switch_to_window`: focus an already-running app by window class.\n\
\n\
FORBIDDEN: action=\"screenshot\" on the computer tool. You already have \
the screenshot. Calling screenshot wastes ~6s of latency.\n\
\n\
Emit the tool call directly. No preamble, no description, no narration."
}

/// Tool definitions for find_action. Only the cursor + launch tools;
/// no integration tools (those go through the Integration path).
fn find_action_tools(declared_w: u32, declared_h: u32) -> serde_json::Value {
    serde_json::json!([
        {
            "type": "computer_20250124",
            "name": "computer",
            "display_width_px": declared_w,
            "display_height_px": declared_h
        },
        {
            "name": "open_url",
            "description": "Open a fully-qualified https:// URL in the default browser.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string" }
                },
                "required": ["url"]
            }
        },
        {
            "name": "launch_app",
            "description": "Launch a desktop application by name (e.g. 'spotify').",
            "input_schema": {
                "type": "object",
                "properties": {
                    "app": { "type": "string" }
                },
                "required": ["app"]
            }
        },
        {
            "name": "switch_to_window",
            "description": "Focus an already-running app by window class or title substring.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "target": { "type": "string" }
                },
                "required": ["target"]
            }
        }
    ])
}
