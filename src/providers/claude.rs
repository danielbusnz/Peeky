use crate::screenshot::pick_declared_resolution;
use futures_util::StreamExt;

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
    /// Computer Use call optimized for SPEED — returns only the click
    /// coordinates, no text. The prompt forces the model to invoke the
    /// `computer` tool immediately and stop. Designed to be fired in
    /// parallel with [`Claude::describe_with_image`] so the cursor flies
    /// before the spoken response is ready.
    ///
    /// `image_b64` is a base64-encoded JPEG captured at native resolution.
    /// This function resizes it to the aspect-matched declared resolution
    /// internally so coords can be scaled back accurately.
    ///
    /// The `on_point` callback fires the instant Claude emits the tool's
    /// coordinates, so the caller can move the cursor mid-stream.
    pub async fn find_point<F>(
        &self,
        prompt: &str,
        image_b64: &str,
        window_x: i64,
        window_y: i64,
        window_width: i64,
        window_height: i64,
        mut on_point: F,
    ) -> Result<Option<(i64, i64)>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnMut(i64, i64),
    {
        // `image_b64` is expected to be PRE-RESIZED to one of the Computer Use
        // declared resolutions. We re-derive (declared_w, declared_h) from the
        // window dimensions so the coord-scaling math stays consistent.
        let (declared_w, declared_h) = pick_declared_resolution(window_width, window_height);
        eprintln!(
            "[timing-claude:find_point] image size ({} KB b64)",
            image_b64.len() / 1024
        );

        let user_prompt = format!(
            "The user said: \"{}\". Find the most relevant UI element on screen — \
             button, link, menu item, sidebar entry, anything visible they could be referring to. \
             ALWAYS invoke the computer tool with a left_click on the center of that element. \
             Even if the user phrased it as a question (like 'where is X'), interpret it as a \
             request to point at X. The user wants to SEE where it is, not just hear about it. \
             Do not output text — only call the tool.",
            prompt
        );

        let body = serde_json::json!({
            "model": "claude-haiku-4-5",
            // 500 gives ample headroom for any preamble Claude might emit
            // before the tool call. Empirically the model uses ~60 tokens
            // on "I'll click on..." text before the actual tool block.
            "max_tokens": 500,
            "stream": true,
            "system": "You are a UI coordinate finder. A screenshot has ALREADY been \
                       provided to you in this message — do NOT call action=\"screenshot\", \
                       it is forbidden and will be discarded. Your ONLY valid action is \
                       left_click with x and y coordinates pointing to the center of \
                       the UI element the user asked about. Do NOT explain. Do NOT \
                       describe what you see. Do NOT say what you're about to do. \
                       Skip directly to the tool call. Your response must contain \
                       only the tool_use block with action=\"left_click\" and \
                       coordinate=[x, y]. If you cannot find the target in the provided \
                       screenshot, return plain text saying why — do NOT call any tool.",
            "tools": [{
                "type": "computer_20250124",
                "name": "computer",
                "display_width_px": declared_w,
                "display_height_px": declared_h
            }],
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
            "[timing-claude:find_point] upload + headers received → {:?}",
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
        let mut current_block_is_tool = false;
        let mut point: Option<(i64, i64)> = None;
        let mut first_byte_logged = false;
        let mut stop_reason: Option<String> = None;

        while let Some(chunk) = stream.next().await {
            if !first_byte_logged {
                eprintln!(
                    "[timing-claude:find_point] first SSE byte → {:?}",
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
                            current_block_is_tool =
                                event["content_block"]["type"].as_str() == Some("tool_use");
                            if current_block_is_tool {
                                tool_json_buffer.clear();
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
                            if current_block_is_tool && !tool_json_buffer.is_empty() {
                                if let Ok(input) =
                                    serde_json::from_str::<serde_json::Value>(&tool_json_buffer)
                                {
                                    if input["action"] == "left_click" {
                                        if let Some(coord) =
                                            input["coordinate"].as_array().filter(|c| c.len() == 2)
                                        {
                                            let raw_x = coord[0]
                                                .as_i64()
                                                .unwrap_or(0)
                                                .clamp(0, declared_w as i64 - 1);
                                            let raw_y = coord[1]
                                                .as_i64()
                                                .unwrap_or(0)
                                                .clamp(0, declared_h as i64 - 1);
                                            let sx = window_x
                                                + (raw_x as f64 * window_width as f64
                                                    / declared_w as f64)
                                                    as i64;
                                            let sy = window_y
                                                + (raw_y as f64 * window_height as f64
                                                    / declared_h as f64)
                                                    as i64;
                                            let cx =
                                                sx.clamp(window_x, window_x + window_width - 1);
                                            let cy =
                                                sy.clamp(window_y, window_y + window_height - 1);
                                            on_point(cx, cy);
                                            point = Some((cx, cy));
                                        } else {
                                            eprintln!(
                                                "[claude:find_point] tool fired action={:?} but no valid coordinate: {}",
                                                input["action"], tool_json_buffer
                                            );
                                        }
                                    } else {
                                        eprintln!(
                                            "[claude:find_point] tool fired with unexpected action: {}",
                                            tool_json_buffer
                                        );
                                    }
                                } else {
                                    eprintln!(
                                        "[claude:find_point] tool_json_buffer didn't parse: {}",
                                        tool_json_buffer
                                    );
                                }
                            }
                            current_block_is_tool = false;
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

        // Diagnostics: log why we didn't get a point if we didn't.
        if point.is_none() {
            eprintln!(
                "[claude:find_point] NO POINT returned. stop_reason={:?}, text_emitted={:?}",
                stop_reason.as_deref().unwrap_or("(none)"),
                if text_buffer.is_empty() {
                    "(empty)".to_string()
                } else {
                    text_buffer.clone()
                }
            );
        }

        Ok(point)
    }

    /// Vision call optimized for the SPOKEN RESPONSE — Claude looks at the
    /// screenshot and answers in plain text, streaming tokens as they arrive.
    /// No tools, no Computer Use overhead. Designed to be fired in parallel
    /// with [`Claude::find_point`].
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
            "system": "You are aegis, a desktop voice assistant looking at the user's screen. Your responses will be spoken aloud. Respond conversationally in 1-2 sentences using only plain text — no markdown, no asterisks, no bullet points, no emojis.",
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
}
