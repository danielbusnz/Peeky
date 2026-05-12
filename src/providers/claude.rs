use super::Llm;
use crate::screenshot::capture_for_claude;

pub struct Claude {
    pub api_key: String,
}

/// Extract the first `[POINT:x,y]` tag from a Claude response.
///
/// Returns `Some((x, y))` if the tag is present and the coordinates parse,
/// otherwise `None`. Used by callers of [`Claude::ask_with_image`] to
/// detect when Claude wants the cursor to fly to a specific screen point.
pub fn parse_point_tag(response: &str) -> Option<(i32, i32)> {
    let start = response.find("[POINT:")?;
    let rest = &response[start + 7..];
    let end = rest.find(']')?;
    let inner = &rest[..end];
    let mut parts = inner.split(',');
    let x = parts.next()?.trim().parse().ok()?;
    let y = parts.next()?.trim().parse().ok()?;
    Some((x, y))
}

impl Claude {
    /// Loads the API key from `.env` or the environment.
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        dotenvy::dotenv().ok();
        let api_key = std::env::var("ANTHROPIC_API_KEY")?;
        Ok(Claude { api_key })
    }
}

impl Llm for Claude {
    fn complete(&self, prompt: &str) -> Result<String, Box<dyn std::error::Error>> {
        let body = serde_json::json!({
            "model": "claude-haiku-4-5",
            "max_tokens": 1024,
            "system": "You are aegis, a desktop voice assistant. Your responses will be spoken aloud. Respond conversationally in 1-2 sentences. Use only plain text — no markdown, no headers, no asterisks, no bullet points, no emojis. Write the way a person would speak.",
            "messages": [
                { "role": "user", "content": prompt }
            ]
        });

        let response = reqwest::blocking::Client::new()
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()?;

        if !response.status().is_success() {
            return Err(format!(
                "Anthropic API error {}: {}",
                response.status(),
                response.text().unwrap_or_default()
            )
            .into());
        }

        let json: serde_json::Value = response.json()?;
        Ok(json["content"][0]["text"]
            .as_str()
            .ok_or("no text in response")?
            .to_string())
    }
}

impl Claude {
    /// Send a prompt + image to Claude using **Tool Use** for guaranteed
    /// structured pointing. Returns `(text, Some((x, y)))` if Claude
    /// invoked the `point_at` tool, otherwise `(text, None)`.
    ///
    /// Prefer this over [`Claude::ask_with_image`] when you want a
    /// schema-enforced response shape instead of the brittle
    /// `[POINT:x,y]` text-tag approach.
    pub fn ask_with_image_tool(
        &self,
        prompt: &str,
        image_b64: &str,
    ) -> Result<(String, Option<(i32, i32)>), Box<dyn std::error::Error>> {
        let body = serde_json::json!({
            "model": "claude-opus-4-7",
            "max_tokens": 1024,
            "system": "You are aegis, a desktop voice assistant looking at the user's screen. Your responses will be spoken aloud. Respond conversationally in 1-2 sentences using only plain text — no markdown, no asterisks, no bullet points, no emojis. When the user asks WHERE something is on screen, invoke the point_at tool with the element's center coordinates. Skip the tool for general or non-spatial questions.",
            "tools": [{
                "name": "point_at",
                "description": "Point the cursor at a specific UI element on screen. Only call when the user is asking WHERE something is or asking you to indicate a screen location.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "x": { "type": "integer", "description": "x pixel coordinate (0 = left)" },
                        "y": { "type": "integer", "description": "y pixel coordinate (0 = top)" }
                    },
                    "required": ["x", "y"]
                }
            }],
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": image_b64 } },
                    { "type": "text", "text": prompt }
                ]
            }]
        });

        let response = reqwest::blocking::Client::new()
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()?;

        if !response.status().is_success() {
            return Err(format!(
                "Anthropic API error {}: {}",
                response.status(),
                response.text().unwrap_or_default()
            )
            .into());
        }

        let json: serde_json::Value = response.json()?;
        let blocks = json["content"]
            .as_array()
            .ok_or("no content array in response")?;

        let mut text = String::new();
        let mut point: Option<(i32, i32)> = None;
        for block in blocks {
            match block["type"].as_str() {
                Some("text") => {
                    if let Some(t) = block["text"].as_str() {
                        text.push_str(t);
                    }
                }
                Some("tool_use") if block["name"] == "point_at" => {
                    let x = block["input"]["x"].as_i64().map(|n| n as i32);
                    let y = block["input"]["y"].as_i64().map(|n| n as i32);
                    if let (Some(x), Some(y)) = (x, y) {
                        point = Some((x, y));
                    }
                }
                _ => {}
            }
        }

        Ok((text, point))
    }

    /// Send a prompt + image to Claude and return the text response.
    ///
    /// **Note:** This is the fallback "regex" approach — Claude appends
    /// a `[POINT:x,y]` tag in plain text when relevant, which the caller
    /// must parse with [`parse_point_tag`]. Prefer
    /// [`Claude::ask_with_image_tool`] for new code.
    pub fn ask_with_image(
        &self,
        prompt: &str,
        image_b64: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let body = serde_json::json!({
            "model": "claude-opus-4-7",
            "max_tokens": 1024,
            "system": "You are aegis, a desktop voice assistant looking at the user's screen. Your responses will be spoken aloud. Respond conversationally in 1-2 sentences using only plain text — no markdown, no asterisks, no bullet points, no emojis.\n\nIf the user asks WHERE something is on screen, end your response with the tag [POINT:x,y] using absolute pixel coordinates of that element's center. The screen is in standard pixel coordinates where (0,0) is the top-left. Only include the tag when the user is asking for a location; omit it for general questions.",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": image_b64 } },
                    { "type": "text", "text": prompt }
                ]
            }]
        });

        let response = reqwest::blocking::Client::new()
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()?;

        if !response.status().is_success() {
            return Err(format!(
                "Anthropic API error {}: {}",
                response.status(),
                response.text().unwrap_or_default()
            )
            .into());
        }

        let json: serde_json::Value = response.json()?;
        Ok(json["content"][0]["text"]
            .as_str()
            .ok_or("no text in response")?
            .to_string())
    }
}

impl Claude {
    /// **Currently unused.** Alternative pointing approach using Anthropic's
    /// Computer Use API.
    ///
    /// Kept as reference for issue #5 (cursor clicks). Our active approach
    /// is the `[POINT:x,y]` tag inside `ask_with_image`'s system prompt,
    /// which is cheaper and simpler. This function may be revived when we
    /// add real click/keypress capabilities.
    ///
    /// Captures the given screen region, asks Claude where the user-described
    /// element is, and returns absolute screen coordinates (or None if Claude
    /// says there's no specific element to point at).
    #[allow(dead_code)]
    pub fn detect_element_location(
        &self,
        prompt: &str,
        window_x: i64,
        window_y: i64,
        window_width: i64,
        window_height: i64,
    ) -> Result<Option<(i64, i64)>, Box<dyn std::error::Error>> {
        let (image_b64, declared_w, declared_h) = capture_for_claude(
            window_x as i32,
            window_y as i32,
            window_width as i32,
            window_height as i32,
        )?;

        let user_prompt = format!(
            "Look at the screenshot. The user asked: \"{}\". \
             If there is a specific UI element they should interact with, click on it. \
             If the question is conceptual, respond with text saying \"no specific element\".",
            prompt
        );

        let body = serde_json::json!({
            "model": "claude-opus-4-7",
            "max_tokens": 256,
            "tools": [{
                "type": "computer_20251124",
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
            .header("anthropic-beta", "computer-use-2025-11-24")
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

        for block in content {
            if block["type"] != "tool_use" {
                continue;
            }
            if block["input"]["action"] != "left_click" {
                continue;
            }
            let coord = match block["input"]["coordinate"].as_array() {
                Some(c) if c.len() == 2 => c,
                _ => continue,
            };
            let x = coord[0].as_i64().ok_or("coord[0] not an integer")?;
            let y = coord[1].as_i64().ok_or("coord[1] not an integer")?;

            let screen_x = window_x + (x as f64 * window_width as f64 / declared_w as f64) as i64;
            let screen_y = window_y + (y as f64 * window_height as f64 / declared_h as f64) as i64;
            return Ok(Some((screen_x, screen_y)));
        }

        Ok(None)
    }
}
