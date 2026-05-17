use crate::screenshot::pick_declared_resolution;
use futures_util::StreamExt;
use std::time::Duration;

/// A side-effecting action Claude requested via one of the tools in
/// `find_action`. The streaming parser surfaces these in real time so the
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
    /// `open_url` custom tool. URL is whatever Claude emitted; validation
    /// happens at execution time, not here.
    OpenUrl { url: String },
    /// `launch_app` custom tool. App is a desktop-file basename or a
    /// runnable binary name.
    LaunchApp { app: String },
    /// `switch_to_window` custom tool. Target is a window class or title.
    SwitchToWindow { target: String },
    /// An integration tool call (Spotify, etc.) we don't have a dedicated
    /// variant for. Dispatched at runtime by name via the integrations
    /// registry. `input` is the raw JSON payload Claude emitted.
    Integration {
        name: String,
        input: serde_json::Value,
    },
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
    /// Action-dispatch call optimized for SPEED. Claude looks at the
    /// screenshot, picks ONE tool (click, open_url, launch_app, or
    /// switch_to_window), and invokes it. The prompt forces the model
    /// to skip preamble and go straight to the tool call. Designed to
    /// fire in parallel with [`Claude::describe_with_image`] so the
    /// action lands before the spoken response is ready.
    ///
    /// `image_b64` is a base64-encoded JPEG captured at native resolution.
    /// This function resizes it to the aspect-matched declared resolution
    /// internally so click coords can be scaled back accurately.
    ///
    /// `on_action` fires the instant Claude finishes streaming the tool's
    /// input JSON, so the caller can dispatch the side effect (cursor
    /// move, browser open, app launch, etc.) mid-stream.
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
        // `image_b64` is expected to be PRE-RESIZED to one of the Computer Use
        // declared resolutions. We re-derive (declared_w, declared_h) from the
        // window dimensions so the coord-scaling math stays consistent.
        let (declared_w, declared_h) = pick_declared_resolution(window_width, window_height);
        eprintln!(
            "[timing-claude:find_action] image size ({} KB b64)",
            image_b64.len() / 1024
        );

        let user_prompt = format!(
            "The user said: \"{}\". Pick the single best action and invoke its tool. \
             Skip directly to the tool call — no text, no preamble.",
            prompt
        );

        let body = serde_json::json!({
            "model": "claude-haiku-4-5",
            // 500 gives ample headroom for any preamble Claude might emit
            // before the tool call. Empirically the model uses ~60 tokens
            // on "I'll click on..." text before the actual tool block.
            "max_tokens": 500,
            "stream": true,
            "system": "You are a desktop voice-assistant action dispatcher. A screenshot \
                       of the user's screen is in this message — do NOT call \
                       action=\"screenshot\" on the computer tool, it is forbidden. \
                       Pick exactly ONE tool based on the user's request:\n\
                       - `computer` mouse_move(coordinate=[x,y]): the user wants to SEE \
                         where something is on screen, no click (\"where is the play \
                         button\", \"show me X\", \"find X\", \"point at X\"). Cursor \
                         visually moves but no input is injected.\n\
                       - `computer` left_click(coordinate=[x,y]): the user wants to \
                         actually CLICK something visible on screen (\"click the play \
                         button\", \"press X\", \"select that\"). Cursor moves AND a \
                         real click fires.\n\
                       - `computer` type(text=\"...\"): type text into the currently \
                         focused field (\"type hello\", \"search for X\"). If the user \
                         clearly wants the text submitted (e.g. \"search for X\", \"send \
                         the message X\"), end `text` with \\n so Enter fires. For multi-\
                         step intents like \"search YouTube for cats\" emit BOTH a \
                         left_click on the search bar AND a type(text=\"cats\\n\") in the \
                         same response — aegis will run them in order.\n\
                       - `open_url`: to navigate to a URL (\"open the rust docs for map\", \
                         \"pull up youtube.com\"). Use https:// URLs only.\n\
                       - `launch_app`: to start an app that may not be running yet \
                         (\"open spotify\", \"launch vs code\", \"open my terminal\"). \
                         Pass the lowercase common name.\n\
                       - `switch_to_window`: to focus an app the user already has open \
                         (\"switch to firefox\", \"focus my terminal\"). Pass a window \
                         class or title substring.\n\
                       No preamble, no description, no explanation. Skip directly to \
                       the tool call. If none fits, return plain text saying why.",
            "tools": [
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
                            "url": {
                                "type": "string",
                                "description": "Fully-qualified URL including scheme."
                            }
                        },
                        "required": ["url"]
                    }
                },
                {
                    "name": "launch_app",
                    "description": "Launch a desktop application by name. Use for queries \
                        like 'open Spotify', 'launch Firefox', 'open my terminal'. The app \
                        argument is the app's common name (e.g. 'spotify', 'firefox', \
                        'code', 'kitty'). Do NOT use for switching to an already-running \
                        app — use switch_to_window for that.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "app": {
                                "type": "string",
                                "description": "App name or .desktop file basename, lowercase."
                            }
                        },
                        "required": ["app"]
                    }
                },
                {
                    "name": "switch_to_window",
                    "description": "Focus an already-running application window. Use for \
                        'switch to Firefox', 'focus my terminal' when the app is already \
                        open. Do NOT use to launch a new app — use launch_app for that. \
                        The target is a window class or title substring.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "target": {
                                "type": "string",
                                "description": "Window class (e.g. 'firefox') or title substring."
                            }
                        },
                        "required": ["target"]
                    }
                }
            ],
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": image_b64 } },
                    { "type": "text", "text": user_prompt }
                ]
            }]
        });

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
            "[timing-claude:find_action] upload + headers received → {:?}",
            t_send.elapsed()
        );

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Computer Use API error {}: {}", status, text).into());
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut tool_json_buffer = String::new();
        let mut text_buffer = String::new();
        let mut current_tool_name: Option<String> = None;
        let mut last_action: Option<Action> = None;
        let mut first_byte_logged = false;
        let mut stop_reason: Option<String> = None;

        while let Some(chunk) = stream.next().await {
            if !first_byte_logged {
                eprintln!(
                    "[timing-claude:find_action] first SSE byte → {:?}",
                    t_send.elapsed()
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
                                current_tool_name = event["content_block"]["name"]
                                    .as_str()
                                    .map(str::to_string);
                                tool_json_buffer.clear();
                            } else {
                                current_tool_name = None;
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
                                    text_buffer.push_str(t);
                                }
                            }
                        }
                        Some("content_block_stop") => {
                            if let Some(name) = current_tool_name.take() {
                                if !tool_json_buffer.is_empty() {
                                    match serde_json::from_str::<serde_json::Value>(
                                        &tool_json_buffer,
                                    ) {
                                        Ok(input) => {
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
                                                last_action = Some(action);
                                            } else {
                                                eprintln!(
                                                    "[claude:find_action] tool '{}' input didn't match any handler: {}",
                                                    name, tool_json_buffer
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            eprintln!(
                                                "[claude:find_action] tool '{}' JSON didn't parse ({}): {}",
                                                name, e, tool_json_buffer
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        Some("message_delta") => {
                            if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                                stop_reason = Some(reason.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if last_action.is_none() {
            eprintln!(
                "[claude:find_action] NO ACTION returned. stop_reason={:?}, text_emitted={:?}",
                stop_reason.as_deref().unwrap_or("(none)"),
                if text_buffer.is_empty() {
                    "(empty)".to_string()
                } else {
                    text_buffer.clone()
                }
            );
        }

        Ok(last_action)
    }

    /// Vision call optimized for the SPOKEN RESPONSE — Claude looks at the
    /// screenshot and answers in plain text, streaming tokens as they arrive.
    /// No tools, no Computer Use overhead. Designed to be fired in parallel
    /// with [`Claude::find_action`].
    ///
    /// The `on_token` callback fires for each text delta so callers can pipe
    /// partial text to a streaming TTS.
    pub async fn describe_with_image<F>(
        &self,
        prompt: &str,
        image_b64: &str,
        mut on_token: F,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnMut(&str),
    {
        eprintln!(
            "[timing-claude:describe] image size ({} KB b64)",
            image_b64.len() / 1024
        );

        let body = serde_json::json!({
            "model": "claude-haiku-4-5",
            "max_tokens": 200,
            "stream": true,
            "system": "You are aegis, a desktop voice assistant looking at the user's screen. Your responses will be spoken aloud. Respond conversationally in 1-2 sentences using only plain text — no markdown, no asterisks, no bullet points, no emojis.\n\nIMPORTANT: A parallel dispatcher already handles opening URLs, launching apps, switching windows, and clicking UI elements for the user. If the user is asking you to do one of those things, do NOT say you can't — assume it's being handled, and either acknowledge briefly (\"opening it now\") or just answer any non-action part of their question. Never tell the user you can't open apps, navigate to URLs, switch windows, or click things.",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": image_b64 } },
                    { "type": "text", "text": prompt }
                ]
            }]
        });

        let t_send = std::time::Instant::now();
        let response = self
            .http
            .post(&self.endpoint)
            .header(&self.auth.0, &self.auth.1)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;
        eprintln!(
            "[timing-claude:describe] upload + headers received → {:?}",
            t_send.elapsed()
        );

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Anthropic API error {}: {}", status, text).into());
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut accumulated = String::new();
        let mut first_byte_logged = false;

        while let Some(chunk) = stream.next().await {
            if !first_byte_logged {
                eprintln!(
                    "[timing-claude:describe] first SSE byte → {:?}",
                    t_send.elapsed()
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
                    if event["type"] == "content_block_delta"
                        && event["delta"]["type"] == "text_delta"
                    {
                        if let Some(t) = event["delta"]["text"].as_str() {
                            accumulated.push_str(t);
                            on_token(t);
                        }
                    }
                }
            }
        }

        Ok(accumulated)
    }

    /// Multi-step agent loop. Each iteration calls Claude with the running
    /// message history, executes any tool_use blocks via `on_action`, waits
    /// SETTLE_MS for the UI to settle, captures a fresh screenshot via
    /// `take_screenshot`, appends the assistant turn + a user tool_result
    /// turn (containing the new screenshot), and recurses. Exits when
    /// Claude returns a response with no tool calls (text-only answer),
    /// when MAX_STEPS is reached, or when the future is dropped by an
    /// outer barge-in select.
    ///
    /// Returns the final text content from the last iteration — useful as
    /// a spoken summary if you want to drop the parallel `describe_with_image`
    /// later. For now voice.rs still runs describe in parallel and the
    /// returned text is informational only.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_agent_loop<F, S>(
        &self,
        prompt: &str,
        initial_screenshot_b64: &str,
        running_apps: &[String],
        window_x: i64,
        window_y: i64,
        window_width: i64,
        window_height: i64,
        mut take_screenshot: S,
        mut on_action: F,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnMut(Action),
        S: FnMut() -> Result<String, Box<dyn std::error::Error + Send + Sync>>,
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
                spotify_pause). These are dramatically faster than visual \
                automation — one tool call vs. 5-10 steps of click+type. \
                Use them whenever the user's intent matches.\n\
             2. If no integration tool exists and the target app IS in the \
                running list above, prefer switch_to_window to focus it and \
                interact via click+type.\n\
             3. If no integration tool exists and the app is NOT running, \
                use launch_app to start it (or open_url for web services).\n\
             4. open_url is for pure web destinations — sites without a \
                desktop app, or when the user explicitly says \"in the browser\".\n\n\
             Pick the best action(s) and invoke their tools. If the request \
             needs multiple steps, call multiple tools across iterations — \
             you'll get a fresh screenshot after each batch. When the task \
             is fully done, respond with plain text and no tool calls to end \
             the chain.",
            prompt, running_list
        );
        let mut messages: Vec<serde_json::Value> = vec![serde_json::json!({
            "role": "user",
            "content": [
                { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": initial_screenshot_b64 } },
                { "type": "text", "text": user_prompt }
            ]
        })];

        let mut final_text = String::new();

        for step in 0..MAX_STEPS {
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
                "system": system_prompt_for_actions(),
                "tools": tools_array_value(declared_w, declared_h),
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
                        let Some(data) = line.strip_prefix("data: ") else { continue; };
                        let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else { continue; };

                        match event["type"].as_str() {
                            Some("content_block_start") => {
                                if event["content_block"]["type"].as_str() == Some("tool_use") {
                                    current_tool_name = event["content_block"]["name"].as_str().map(str::to_string);
                                    current_tool_id = event["content_block"]["id"].as_str().map(str::to_string);
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
                                    }
                                }
                            }
                            Some("content_block_stop") => {
                                if let (Some(name), Some(id)) =
                                    (current_tool_name.take(), current_tool_id.take())
                                {
                                    if !tool_json_buffer.is_empty() {
                                        let input_json = tool_json_buffer.clone();
                                        if let Ok(input) = serde_json::from_str::<serde_json::Value>(&input_json) {
                                            if let Some(action) = parse_tool_call(
                                                &name, &input, declared_w, declared_h,
                                                window_x, window_y, window_width, window_height,
                                            ) {
                                                on_action(action);
                                            } else {
                                                // No dispatch (e.g. computer.screenshot/key/scroll/wait/cursor_position).
                                                // Log so we can see what's eating the silent steps.
                                                let action_field = input["action"]
                                                    .as_str()
                                                    .unwrap_or("(none)");
                                                eprintln!(
                                                    "[agent-loop] unhandled tool '{}' action='{}' input={}",
                                                    name, action_field, input_json
                                                );
                                            }
                                            tool_calls.push((id, name, input_json));
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

            // Settle: let UI react to the actions before taking the next screenshot.
            let t_settle = std::time::Instant::now();
            tokio::time::sleep(Duration::from_millis(SETTLE_MS)).await;
            eprintln!(
                "[agent-loop] step {} settle ({}ms) → {:?} actual",
                step + 1,
                SETTLE_MS,
                t_settle.elapsed()
            );

            // Capture fresh screenshot for the next iteration's tool_result.
            let t_shot = std::time::Instant::now();
            let new_screenshot = take_screenshot()?;
            eprintln!(
                "[agent-loop] step {} screenshot captured ({} KB) in {:?}",
                step + 1,
                new_screenshot.len() / 1024,
                t_shot.elapsed()
            );

            // Append tool_results — one per tool_use_id, each with the same
            // post-action screenshot (Claude needs to see what changed).
            let t_tail = std::time::Instant::now();
            let tool_results: Vec<serde_json::Value> = tool_calls
                .iter()
                .map(|(id, _, _)| {
                    serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": id,
                        "content": [
                            { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": new_screenshot } }
                        ]
                    })
                })
                .collect();
            messages.push(serde_json::json!({ "role": "user", "content": tool_results }));

            // Linear token cost grows fast if we ship every screenshot
            // forever — strip image data from older tool_results, keeping
            // only the N most recent.
            trim_old_screenshots(&mut messages, KEEP_RECENT_SCREENSHOTS);
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

/// Shared system prompt used by `find_action` and `run_agent_loop`. Kept
/// as a function (not a const) so it can be tweaked without breaking the
/// const-eval rules around multi-line raw strings.
fn system_prompt_for_actions() -> &'static str {
    "You are a desktop voice-assistant action dispatcher.\n\n\
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

/// Shared tools array used by both single-call and loop entry points.
/// Appends any tools exposed by ready integrations (spotify, etc.) at
/// the end of the array so Claude can call them when the user's intent
/// matches.
fn tools_array_value(declared_w: u32, declared_h: u32) -> serde_json::Value {
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

    tools.extend(crate::integrations::all_tools());
    serde_json::Value::Array(tools)
}

/// Trim image data from `tool_result` blocks older than the most recent
/// `keep_last_n` screenshots. Replaces the image with a text placeholder
/// so Claude knows there WAS a screenshot at that point, but the bytes
/// are gone. Keeps the conversation graph intact while controlling cost.
fn trim_old_screenshots(messages: &mut [serde_json::Value], keep_last_n: usize) {
    let placeholder = || serde_json::json!({
        "type": "text",
        "text": "[older screenshot omitted]"
    });
    let mut seen = 0usize;
    for msg in messages.iter_mut().rev() {
        if msg["role"] != "user" {
            continue;
        }
        let Some(content) = msg["content"].as_array_mut() else { continue; };
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
            let Some(inner) = block["content"].as_array_mut() else { continue; };
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
        "left_click" | "right_click" | "middle_click" | "double_click"
        | "triple_click" | "mouse_move" | "type" | "key" | "scroll"
        | "screenshot" | "wait" | "cursor_position" => (tool_name.to_string(), input),
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
        "left_click" | "right_click" | "middle_click" | "double_click"
        | "triple_click" => Some(Action::Click { x, y }),
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
/// returned as `Action::Integration { name, input }` for runtime
/// dispatch to whichever integration owns it (Spotify, etc.). If no
/// integration owns it, the dispatcher logs the unknown name.
fn parse_custom_tool(tool_name: &str, input: &serde_json::Value) -> Option<Action> {
    match tool_name {
        "open_url" => input["url"]
            .as_str()
            .map(|s| Action::OpenUrl { url: s.to_string() }),
        "launch_app" => input["app"]
            .as_str()
            .map(|s| Action::LaunchApp { app: s.to_string() }),
        "switch_to_window" => input["target"]
            .as_str()
            .map(|s| Action::SwitchToWindow { target: s.to_string() }),
        _ => Some(Action::Integration {
            name: tool_name.to_string(),
            input: input.clone(),
        }),
    }
}
