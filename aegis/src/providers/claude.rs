use crate::screenshot::pick_declared_resolution;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;
use std::time::Duration;

/// A side-effecting action Claude requested via one of the tools in
/// `run_agent_loop`. The streaming parser surfaces these in real time so the
/// caller can fire them before the response is finished.
#[derive(Debug, Clone)]
pub enum Action {
    /// `computer` tool, `mouse_move`. Visual overlay moves to (x,y) but
    /// no real input is injected. Used for "where is X" / "show me X".
    Point { x: i64, y: i64 },
    /// `computer` tool, `left_click`. Visual overlay AND system mouse
    /// click at (x,y). Used for "click X" / "press X" / "select X".
    Click { x: i64, y: i64 },
    /// `computer` tool, `type`. Types `text` into the currently focused
    /// field. Used for "type X" / "search for X" / "write X". Embed a
    /// trailing \n if the result should be submitted (Enter).
    Type { text: String },
    /// `computer` tool, `key`. Press a key or key combination like
    /// "Return", "Tab", "Escape", "ctrl+a", "ctrl+f". The `key` string
    /// is whatever Claude emitted; parsing happens in the action handler.
    Key { key: String },
    /// `computer` tool, `scroll`. Direction is "up"/"down"/"left"/"right";
    /// amount is the number of "wheel clicks" Claude wants. Coordinate
    /// (if Claude provided one) is currently ignored — Wayland doesn't
    /// expose a clean "scroll at point" primitive without raw evdev.
    Scroll { direction: String, amount: u32 },
    /// `open_url` custom tool. URL is whatever Claude emitted; validation
    /// happens at execution time, not here.
    OpenUrl { url: String },
    /// `launch_app` custom tool. App is a desktop-file basename or a
    /// runnable binary name.
    LaunchApp { app: String },
    /// `switch_to_window` custom tool. Target is a window class or title.
    SwitchToWindow { target: String },
    /// An integration tool call (Spotify, etc.) we don't have a dedicated
    /// variant for. Dispatched at runtime via the integrations registry;
    /// the name + raw JSON payload are pulled from the outer SSE event, not
    /// from this variant. The variant exists purely so the on_action
    /// callback can tell integration tools apart from cursor-visible ones.
    Integration,
}

pub struct Claude {
    pub http: reqwest::Client,
    /// Full URL to POST messages requests to. Either the hosted proxy or
    /// api.anthropic.com depending on which mode we're in.
    pub endpoint: String,
    /// (header_name, header_value) for auth. Either ("x-aegis-device-id", uuid)
    /// when routed through the proxy, or ("x-api-key", anthropic_key) in
    /// direct mode.
    pub auth: (String, String),
}

impl Claude {
    /// Open the HTTPS connection to our endpoint so the first real voice
    /// turn doesn't pay TLS handshake cost. Fires a deliberately-malformed
    /// request that fast-fails on the server; the TCP+TLS handshake leaves
    /// a warm connection in reqwest's pool. Response is discarded.
    pub async fn warm(&self) {
        let _ = self
            .http
            .post(&self.endpoint)
            .header(&self.auth.0, &self.auth.1)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .body("{}")
            .send()
            .await;
    }
}

/// Default endpoint for the hosted proxy. Override at compile time by setting
/// `AEGIS_PROXY_URL` to a different worker URL if you deploy your own.
const PROXY_URL: &str = "https://aegis-proxy.danielbusnz.workers.dev/v1/anthropic/messages";
const DIRECT_URL: &str = "https://api.anthropic.com/v1/messages";

impl Claude {
    /// Initialize from `.env`/environment. Default behavior is to route through
    /// the hosted aegis-proxy on Cloudflare, identified by a per-install UUID.
    /// No API key needed — that's the whole plug-and-play story.
    ///
    /// To bypass the proxy and talk to Anthropic directly (useful for local
    /// dev, debugging, or burning your own credit), set
    /// `AEGIS_ANTHROPIC_DIRECT=1` in the environment AND provide
    /// `ANTHROPIC_API_KEY`.
    ///
    /// `http` is the shared `reqwest::Client` so connection pools (TCP/TLS)
    /// are reused across calls. Saves the ~150ms handshake on every call
    /// after the first.
    pub fn from_env(http: reqwest::Client) -> Result<Self, Box<dyn std::error::Error>> {
        dotenvy::dotenv().ok();

        if std::env::var("AEGIS_ANTHROPIC_DIRECT").is_ok() {
            let api_key = std::env::var("ANTHROPIC_API_KEY")?;
            return Ok(Claude {
                http,
                endpoint: DIRECT_URL.to_string(),
                auth: ("x-api-key".to_string(), api_key),
            });
        }

        let device_id = super::device_id::load_or_create()?;
        Ok(Claude {
            http,
            endpoint: PROXY_URL.to_string(),
            auth: ("x-aegis-device-id".to_string(), device_id),
        })
    }
}

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
        const MAX_STEPS: usize = 10;
        const SETTLE_MS: u64 = 600;
        const KEEP_RECENT_SCREENSHOTS: usize = 3;

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
             needs multiple steps, call multiple tools across iterations — \
             you'll get a fresh screenshot after each batch. When the task \
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
        // When the previous step had only integration tool calls, we expect
        // this step to be a text-only summary (no tools). Stream text
        // tokens to the caller via on_text_delta so TTS can start speaking
        // before the full response is collected.
        let mut prev_step_was_integration_only = false;

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
            let body = serde_json::json!({
                "model": "claude-haiku-4-5",
                "max_tokens": 1024,
                "stream": true,
                "system": system,
                "tools": tools_array_value(declared_w, declared_h, integration_tools.clone()),
                "messages": messages,
            });
            let body_size_kb = serde_json::to_vec(&body)
                .map(|v| v.len() / 1024)
                .unwrap_or(0);
            eprintln!(
                "[agent-loop] step {} body built ({} KB) in {:?}",
                step + 1,
                body_size_kb,
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
                                } else if delta_type == Some("text_delta") {
                                    if let Some(t) = event["delta"]["text"].as_str() {
                                        text_content.push_str(t);
                                        if prev_step_was_integration_only {
                                            on_text_delta(t);
                                        }
                                    }
                                }
                            }
                            Some("message_stop") | Some("ping") => {
                                // Expected, no-op. Listed so the catch-all
                                // below can surface anything NEW we're not
                                // handling yet.
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

            // Heuristic for "next step is the text-only final answer":
            // this step had tools, and they were all integrations.
            prev_step_was_integration_only = all_integration;
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

/// System prompt for `run_agent_loop`. Kept as a function (not a const)
/// so it can be tweaked without breaking the const-eval rules around
/// multi-line raw strings.
fn system_prompt_for_actions() -> &'static str {
    "You are a desktop voice-assistant action dispatcher.\n\n\
     CRITICAL: NEVER hedge or apologize for limitations. If a tool exists \
     for what the user asked, just call it. SPECIFICALLY:\n\
     - NEVER say \"I don't have the ability to...\", \"I can't access...\", \
       \"You'd need to open...\", \"I can only see...\", or similar refusals \
       when a matching integration tool exists (gmail_*, spotify_*, etc.). \
       The presence of the tool in the tools array MEANS you have that \
       ability. Use it.\n\
     - NEVER narrate before calling a tool. Do not write 'I'm opening that \
       up for you' or 'Let me check that' as prefix text. Just call the \
       tool. The text you emit is spoken aloud by TTS; every word delays \
       the user's experience.\n\
     - The user's email is Gmail. ANY reference to email, mail, inbox, \
       messages from someone, sending a message to someone, or unread \
       count maps to Gmail. Never ask 'which email service?' or interpret \
       'email' as anything other than Gmail.\n\
     - For 'read my emails' / 'do I have mail' / 'send a message': call \
       gmail_search, gmail_read, gmail_unread_count, gmail_send directly. \
       Do not check the screen first.\n\
     - For 'play X' / 'pause music' / 'next song': call spotify_* directly.\n\
     - For 'show me my PRs' / 'do I have open issues' / 'is CI passing' / \
       'any GitHub notifications': call gh_my_prs, gh_my_issues, \
       gh_actions_status, gh_notifications directly. Do not browse to \
       github.com.\n\
     - Text content is ONLY for the FINAL answer back to the user (the \
       last step in the chain), after all tools have returned data.\n\
     - VOICE BREVITY: the final answer is spoken aloud, not read. Keep it \
       UNDER 100 words. For lists, give the top 3-5 items plus a 'you have \
       N total, want details on any?' summary — do NOT enumerate every \
       item. The user is listening; respect their time. If they want more, \
       they'll ask.\n\n\
     CRITICAL: NEVER call action=\"screenshot\" on the computer tool. \
     A fresh screenshot of the user's screen is ALREADY attached to \
     this message, and after every tool_result a new screenshot will be \
     attached automatically. Calling screenshot wastes a full Claude \
     turn (~6 seconds of user-perceived latency), produces no new \
     information, and visibly slows down multi-step chains. If you \
     think you need to \"look again,\" you don't — the next tool_result \
     will already contain the latest pixels. Just emit the next real \
     action (click, type, open_url, etc.) directly.\n\n\
     Pick the tool(s) needed for the user's request:\n\
     - `computer` mouse_move(coordinate=[x,y]): the user wants to SEE \
       where something is on screen, no click (\"where is the play \
       button\", \"show me X\", \"find X\", \"point at X\"). Cursor \
       visually moves but no input is injected.\n\
     - `computer` left_click(coordinate=[x,y]): the user wants to \
       actually CLICK something visible on screen (\"click the play \
       button\", \"press X\", \"select that\"). Cursor moves AND a \
       real click fires.\n\
     - `computer` type(text=\"...\"): type text into the currently \
       focused field. Prefer to end text with \\n to submit (search, \
       send) in one tool call rather than emitting a separate \
       key(\"Return\") afterward — fewer round trips. For multi-step \
       intents emit BOTH left_click on the input AND type(text=\"...\\n\") \
       in the same response.\n\
     - `computer` key(text=\"...\"): press a key or combo. Supported: \
       Return, Tab, Escape, Backspace, Delete, Home, End, PageUp, \
       PageDown, Up, Down, Left, Right, F1-F12, single letters/digits, \
       and combos like \"ctrl+a\", \"ctrl+f\", \"ctrl+enter\". Use this \
       for hotkeys (e.g. \"c\" toggles captions on YouTube, \"k\" \
       play/pause) or to submit forms when you didn't end a `type` with \
       \\n.\n\
     - `computer` scroll(scroll_direction=\"up\"|\"down\"|\"left\"|\"right\", \
       scroll_amount=N): scroll the focused area. amount is in approximate \
       wheel-clicks (1-10 is typical). Use scroll_amount=3 for short \
       scrolls, 5+ for longer pans. Coordinate is ignored — scrolling \
       happens on whatever element is focused.\n\
     - `open_url`: ALWAYS use this for web destinations — websites, web \
       apps, online docs. Phrases like \"open YouTube\", \"go to gmail\", \
       \"pull up github\", \"open the rust docs\", \"navigate to twitter\" \
       all map to open_url with the canonical URL (https://youtube.com, \
       https://gmail.com, https://github.com, https://doc.rust-lang.org, \
       etc.). NEVER use launch_app for these — do NOT call \
       launch_app(\"firefox\") or launch_app(\"chrome\") even if a browser \
       isn't visibly open; aegis handles which browser to use internally.\n\
       \n\
       PREFER DEEP-LINK URLS over UI navigation. If the request ends in \
       \"open page X\" or \"go to search results for X on site Y\", \
       construct the deep-link URL and call open_url ONCE — do NOT \
       open the homepage and then click the search bar and type. \
       Known search URL patterns:\n\
         - YouTube search: https://www.youtube.com/results?search_query=<URL-encoded query>\n\
         - Google search:  https://www.google.com/search?q=<URL-encoded query>\n\
         - GitHub search:  https://github.com/search?q=<URL-encoded query>\n\
         - Wikipedia:      https://en.wikipedia.org/wiki/<Title_With_Underscores>\n\
         - Amazon search:  https://www.amazon.com/s?k=<URL-encoded query>\n\
         - Spotify search: https://open.spotify.com/search/<URL-encoded query>\n\
         - Twitter/X:      https://twitter.com/search?q=<URL-encoded query>\n\
         - Reddit search:  https://www.reddit.com/search/?q=<URL-encoded query>\n\
         - DuckDuckGo:     https://duckduckgo.com/?q=<URL-encoded query>\n\
       URL-encode spaces as + (Google/Amazon/Twitter/Reddit/DDG) or %20 \
       (Spotify, GitHub also accept +). The user's intent \"open YouTube, \
       search for dogs\" should be a single open_url call with the search \
       results URL — NOT open_url(youtube.com) followed by a click + \
       type sequence. Use click+type only when no URL shortcut exists.\n\
     - `launch_app`: start a NON-browser desktop app that isn't running \
       yet (\"open spotify\", \"launch vs code\", \"open my terminal\", \
       \"open obsidian\"). Pass the lowercase common name. Do NOT use \
       for browsers or websites — those go through open_url.\n\
     - `switch_to_window`: focus an app the user already has open. \
       Pass a window class or title substring.\n\
     If the user's intent requires multiple ordered steps (\"open X, \
     then click Y, then type Z\"), emit only the tools needed for the \
     CURRENT step — you'll see a fresh screenshot after the tools run \
     and can pick the next step. When the whole task is done, respond \
     with plain text and no tool calls to end the chain. No preamble, \
     no explanation."
}

/// Shared tools array for the agent loop. Accepts extra tool schemas
/// (from integrations, etc.) to append so the function doesn't need to
/// import the integrations module directly.
fn tools_array_value(
    declared_w: u32,
    declared_h: u32,
    extra_tools: Vec<serde_json::Value>,
) -> serde_json::Value {
    let mut tools: Vec<serde_json::Value> = serde_json::json!([
        {
            "type": "computer_20250124",
            "name": "computer",
            "display_width_px": declared_w,
            "display_height_px": declared_h
        },
        {
            "name": "open_url",
            "description": "Open a URL in the user's default web browser. \
                Use ONLY for full https:// or http:// URLs the user explicitly \
                wants to navigate to. Do NOT use for clicking a link visible on \
                screen — use the computer tool's left_click for that.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Fully-qualified URL including scheme." }
                },
                "required": ["url"]
            }
        },
        {
            "name": "launch_app",
            "description": "Launch a desktop application by name. Use for queries \
                like 'open Spotify', 'launch Firefox'. The app argument is the \
                app's common name. Do NOT use for switching to an already-running \
                app — use switch_to_window for that.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "app": { "type": "string", "description": "App name or .desktop file basename, lowercase." }
                },
                "required": ["app"]
            }
        },
        {
            "name": "switch_to_window",
            "description": "Focus an already-running application window. Use for \
                'switch to Firefox' when the app is already open. Do NOT use to \
                launch a new app.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "Window class or title substring." }
                },
                "required": ["target"]
            }
        }
    ])
    .as_array()
    .expect("tools literal must be an array")
    .clone();

    tools.extend(extra_tools);
    serde_json::Value::Array(tools)
}

/// Trim image data from `tool_result` blocks older than the most recent
/// `keep_last_n` screenshots. Replaces the image with a text placeholder
/// so Claude knows there WAS a screenshot at that point, but the bytes
/// are gone. Keeps the conversation graph intact while controlling cost.
fn trim_old_screenshots(messages: &mut [serde_json::Value], keep_last_n: usize) {
    let placeholder = || {
        serde_json::json!({
            "type": "text",
            "text": "[older screenshot omitted]"
        })
    };
    let mut seen = 0usize;
    for msg in messages.iter_mut().rev() {
        if msg["role"] != "user" {
            continue;
        }
        let Some(content) = msg["content"].as_array_mut() else {
            continue;
        };
        for block in content.iter_mut() {
            // Two image shapes show up in user messages:
            //   1. Direct image block on the initial transcript turn:
            //      {"type": "image", "source": {...}}
            //   2. Image inside a tool_result content array on each loop
            //      iteration:
            //      {"type": "tool_result", "content": [{"type": "image", ...}]}
            // Both count toward the "N most recent screenshots" budget so
            // the initial transcript image doesn't live forever and bloat
            // the body indefinitely.
            if block["type"] == "image" {
                if seen < keep_last_n {
                    seen += 1;
                } else {
                    *block = placeholder();
                }
                continue;
            }
            if block["type"] != "tool_result" {
                continue;
            }
            let Some(inner) = block["content"].as_array_mut() else {
                continue;
            };
            for item in inner.iter_mut() {
                if item["type"] == "image" {
                    if seen < keep_last_n {
                        seen += 1;
                    } else {
                        *item = placeholder();
                    }
                }
            }
        }
    }
}

/// Dispatch a completed `tool_use` block to the corresponding `Action`
/// variant. Returns None if the tool name is unknown or the input shape
/// doesn't match (e.g. `computer` with an action other than `left_click`,
/// or a custom tool missing its required field). Each Some(_) is ready to
/// hand to the caller's `on_action` callback.
#[allow(clippy::too_many_arguments)]
fn parse_tool_call(
    tool_name: &str,
    input: &serde_json::Value,
    declared_w: u32,
    declared_h: u32,
    window_x: i64,
    window_y: i64,
    window_width: i64,
    window_height: i64,
) -> Option<Action> {
    // Claude sometimes emits malformed tool calls where the tool NAME is
    // the action (e.g. `name: "left_click"`) instead of the proper
    // `name: "computer", input.action: "left_click"`. Detect both shapes
    // and normalize to a single (action_name, input) pair before matching.
    let (effective_action, effective_input) = match tool_name {
        "computer" => (input["action"].as_str()?.to_string(), input),
        // The action-as-name fallback. Coordinate / text comes straight
        // from input without an `action` field.
        "left_click" | "right_click" | "middle_click" | "double_click" | "triple_click"
        | "mouse_move" | "type" | "key" | "scroll" | "screenshot" | "wait" | "cursor_position" => {
            (tool_name.to_string(), input)
        }
        // Custom tools handled below.
        _ => return parse_custom_tool(tool_name, input),
    };

    let action = effective_action.as_str();

    // Text-only actions (no coordinate).
    if action == "type" {
        let text = effective_input["text"].as_str()?;
        return Some(Action::Type {
            text: text.to_string(),
        });
    }
    if action == "key" {
        let key = effective_input["text"].as_str()?;
        return Some(Action::Key {
            key: key.to_string(),
        });
    }
    if action == "scroll" {
        let direction = effective_input["scroll_direction"]
            .as_str()
            .unwrap_or("down")
            .to_string();
        // scroll_amount may arrive as integer or stringified integer.
        let amount = effective_input["scroll_amount"]
            .as_u64()
            .or_else(|| {
                effective_input["scroll_amount"]
                    .as_str()
                    .and_then(|s| s.parse::<u64>().ok())
            })
            .unwrap_or(3) as u32;
        return Some(Action::Scroll { direction, amount });
    }

    // Coordinate actions. Accept either a JSON array [x, y] OR a JSON
    // string like "[640, 47]" — Claude's malformed shape emits the latter.
    let (raw_x, raw_y) = extract_coordinate(&effective_input["coordinate"])?;
    let raw_x = raw_x.clamp(0, declared_w as i64 - 1);
    let raw_y = raw_y.clamp(0, declared_h as i64 - 1);
    let sx = window_x + (raw_x as f64 * window_width as f64 / declared_w as f64) as i64;
    let sy = window_y + (raw_y as f64 * window_height as f64 / declared_h as f64) as i64;
    let x = sx.clamp(window_x, window_x + window_width - 1);
    let y = sy.clamp(window_y, window_y + window_height - 1);

    match action {
        // Treat right/middle/double/triple clicks as left clicks for now —
        // most apps treat them similarly for the "I want to interact with
        // THIS element" case. We can add separate Action variants if a
        // real use case appears.
        "left_click" | "right_click" | "middle_click" | "double_click" | "triple_click" => {
            Some(Action::Click { x, y })
        }
        "mouse_move" => Some(Action::Point { x, y }),
        _ => None,
    }
}

/// Parse a coordinate field that may be either a JSON array `[x, y]` or a
/// JSON string `"[x, y]"`. Returns (x, y) as i64.
fn extract_coordinate(value: &serde_json::Value) -> Option<(i64, i64)> {
    if let Some(arr) = value.as_array() {
        if arr.len() == 2 {
            return Some((arr[0].as_i64()?, arr[1].as_i64()?));
        }
    }
    if let Some(s) = value.as_str() {
        // Strip brackets/whitespace, split on comma.
        let trimmed = s.trim().trim_start_matches('[').trim_end_matches(']');
        let parts: Vec<&str> = trimmed.split(',').map(|p| p.trim()).collect();
        if parts.len() == 2 {
            let x = parts[0].parse::<i64>().ok()?;
            let y = parts[1].parse::<i64>().ok()?;
            return Some((x, y));
        }
    }
    None
}

/// Custom tools (open_url, launch_app, switch_to_window) PLUS the
/// integration fallback. Any tool name not in the built-in list is
/// returned as `Action::Integration` for runtime dispatch to whichever
/// integration owns it (Spotify, etc.). If no integration owns it, the
/// dispatcher logs the unknown name.
fn parse_custom_tool(tool_name: &str, input: &serde_json::Value) -> Option<Action> {
    match tool_name {
        "open_url" => input["url"]
            .as_str()
            .map(|s| Action::OpenUrl { url: s.to_string() }),
        "launch_app" => input["app"]
            .as_str()
            .map(|s| Action::LaunchApp { app: s.to_string() }),
        "switch_to_window" => input["target"].as_str().map(|s| Action::SwitchToWindow {
            target: s.to_string(),
        }),
        _ => Some(Action::Integration),
    }
}
