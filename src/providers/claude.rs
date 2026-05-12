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
        let (declared_w, declared_h) = pick_declared_resolution(window_width, window_height);
        let resized = resize_jpeg_for_computer_use(image_b64, declared_w, declared_h)
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                e.to_string().into()
            })?;

        let user_prompt = format!(
            "User asks: \"{}\". Look at the screenshot and find the UI element they're asking about. \
             Invoke the computer tool to left_click on its center. Do not say anything — just call the tool.",
            prompt
        );

        let body = serde_json::json!({
            "model": "claude-haiku-4-5",
            "max_tokens": 100,
            "stream": true,
            "tools": [{
                "type": "computer_20250124",
                "name": "computer",
                "display_width_px": declared_w,
                "display_height_px": declared_h
            }],
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": resized } },
                    { "type": "text", "text": user_prompt }
                ]
            }]
        });

        let response = reqwest::Client::new()
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "computer-use-2025-01-24")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Computer Use API error {}: {}", status, text).into());
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut tool_json_buffer = String::new();
        let mut current_block_is_tool = false;
        let mut point: Option<(i64, i64)> = None;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let s = std::str::from_utf8(&chunk)?;
            buffer.push_str(s);

            while let Some(idx) = buffer.find("\n\n") {
                let frame: String = buffer.drain(..idx + 2).collect();
                for line in frame.lines() {
                    let Some(data) = line.strip_prefix("data: ") else { continue };
                    let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else { continue };

                    match event["type"].as_str() {
                        Some("content_block_start") => {
                            current_block_is_tool =
                                event["content_block"]["type"].as_str() == Some("tool_use");
                            if current_block_is_tool {
                                tool_json_buffer.clear();
                            }
                        }
                        Some("content_block_delta") => {
                            if event["delta"]["type"].as_str() == Some("input_json_delta") {
                                if let Some(j) = event["delta"]["partial_json"].as_str() {
                                    tool_json_buffer.push_str(j);
                                }
                            }
                        }
                        Some("content_block_stop") => {
                            if current_block_is_tool && !tool_json_buffer.is_empty() {
                                if let Ok(input) =
                                    serde_json::from_str::<serde_json::Value>(&tool_json_buffer)
                                {
                                    if input["action"] == "left_click" {
                                        if let Some(coord) = input["coordinate"]
                                            .as_array()
                                            .filter(|c| c.len() == 2)
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
                                        }
                                    }
                                }
                            }
                            current_block_is_tool = false;
                        }
                        _ => {}
                    }
                }
            }
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
                    let Some(data) = line.strip_prefix("data: ") else { continue };
                    let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else { continue };
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
