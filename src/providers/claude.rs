use crate::screenshot::{
    capture_for_claude, pick_declared_resolution, resize_jpeg_for_computer_use,
};
use futures_util::StreamExt;

pub struct Claude {
    pub api_key: String,
}

impl Claude {
    /// Streaming version of `complete`. Posts to Anthropic with `stream: true`,
    /// parses the SSE response, and invokes `on_token` for each `text_delta`
    /// chunk as it arrives. Returns the fully-accumulated text when the stream
    /// completes.
    ///
    /// Designed to be called from a sync thread via `tokio::runtime::Runtime::block_on`,
    /// or directly from an async context (e.g., inside a tokio task).
    pub async fn complete_stream<F>(
        &self,
        prompt: &str,
        mut on_token: F,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnMut(&str),
    {
        let body = serde_json::json!({
            "model": "claude-haiku-4-5",
            "max_tokens": 1024,
            "stream": true,
            "system": "You are aegis, a desktop voice assistant. Your responses will be spoken aloud. Respond conversationally in 1-2 sentences. Use only plain text — no markdown, no headers, no asterisks, no bullet points, no emojis. Write the way a person would speak.",
            "messages": [
                { "role": "user", "content": prompt }
            ]
        });

        let response = reqwest::Client::new()
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Anthropic API error {}: {}", status, text).into());
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut accumulated = String::new();

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
                    if event["type"] == "content_block_delta" {
                        if let Some(text) = event["delta"]["text"].as_str() {
                            accumulated.push_str(text);
                            on_token(text);
                        }
                    }
                }
            }
        }

        Ok(accumulated)
    }
}

impl Claude {
    /// Loads the API key from `.env` or the environment.
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        dotenvy::dotenv().ok();
        let api_key = std::env::var("ANTHROPIC_API_KEY")?;
        Ok(Claude { api_key })
    }
}

impl Claude {
    /// Anthropic Computer Use call on Haiku 4.5. Captures the given screen
    /// region, resizes it to one of three aspect-matched declared resolutions
    /// (1024×768 / 1280×800 / 1366×768), POSTs to the Messages API with the
    /// `computer_20250124` tool definition, and parses the response.
    ///
    /// Returns `(text, Some((x, y)))` if Claude invoked the computer tool
    /// with a `left_click` action — coordinates are scaled back to absolute
    /// screen pixels. Returns `(text, None)` if Claude only spoke and didn't
    /// click. The text is always populated when Claude responds.
    pub fn detect_element_location(
        &self,
        prompt: &str,
        window_x: i64,
        window_y: i64,
        window_width: i64,
        window_height: i64,
    ) -> Result<(String, Option<(i64, i64)>), Box<dyn std::error::Error>> {
        // Capture at native resolution
        let (raw_b64, _, _) = capture_for_claude(
            window_x as i32,
            window_y as i32,
            window_width as i32,
            window_height as i32,
        )?;

        // Pick the aspect-matched declared resolution + resize so coords come
        // back in a known space we can scale precisely.
        let (declared_w, declared_h) = pick_declared_resolution(window_width, window_height);
        let image_b64 = resize_jpeg_for_computer_use(&raw_b64, declared_w, declared_h)?;

        let user_prompt = format!(
            "Look at the screenshot. The user asked: \"{}\". \
             Respond conversationally in 1-2 sentences (plain text, no markdown, no emojis). \
             If there is a specific UI element they're asking about, ALSO use the computer tool to left_click on it. \
             If the question is conceptual, just respond with text and skip the click.",
            prompt
        );

        let body = serde_json::json!({
            "model": "claude-haiku-4-5",
            "max_tokens": 512,
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

        let response = reqwest::blocking::Client::new()
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "computer-use-2025-01-24")
            .json(&body)
            .send()?;

        if !response.status().is_success() {
            return Err(format!(
                "Computer Use API error {}: {}",
                response.status(),
                response.text().unwrap_or_default()
            )
            .into());
        }

        let json: serde_json::Value = response.json()?;
        let content = json["content"].as_array().ok_or("no content array")?;

        let mut text = String::new();
        let mut point: Option<(i64, i64)> = None;

        for block in content {
            match block["type"].as_str() {
                Some("text") => {
                    if let Some(t) = block["text"].as_str() {
                        text.push_str(t);
                    }
                }
                Some("tool_use") if block["input"]["action"] == "left_click" => {
                    let coord = match block["input"]["coordinate"].as_array() {
                        Some(c) if c.len() == 2 => c,
                        _ => continue,
                    };

                    // Layer 1: clamp raw declared-resolution coords from Claude
                    // to [0, declared_w/h). Catches negatives and any
                    // out-of-bounds hallucinations before scaling.
                    let raw_x = coord[0]
                        .as_i64()
                        .unwrap_or(0)
                        .clamp(0, declared_w as i64 - 1);
                    let raw_y = coord[1]
                        .as_i64()
                        .unwrap_or(0)
                        .clamp(0, declared_h as i64 - 1);

                    // Scale declared-space coords back to actual screen pixels.
                    let screen_x = window_x
                        + (raw_x as f64 * window_width as f64 / declared_w as f64) as i64;
                    let screen_y = window_y
                        + (raw_y as f64 * window_height as f64 / declared_h as f64) as i64;

                    // Layer 2: clamp to the actual screen window. Belt + suspenders
                    // for float rounding at the edges.
                    let clamped_x = screen_x.clamp(window_x, window_x + window_width - 1);
                    let clamped_y = screen_y.clamp(window_y, window_y + window_height - 1);

                    point = Some((clamped_x, clamped_y));
                }
                _ => {}
            }
        }

        Ok((text, point))
    }
}
