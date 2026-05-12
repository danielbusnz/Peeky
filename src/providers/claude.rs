use super::Llm;
use crate::screenshot::capture_for_claude;

pub struct Claude {
    pub api_key: String,
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
    /// Send a prompt + image to Claude and return the text response.
    pub fn ask_with_image(
        &self,
        prompt: &str,
        image_b64: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let body = serde_json::json!({
            "model": "claude-opus-4-7",
            "max_tokens": 1024,
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
    /// Computer Use API call. Captures the given screen region, asks Claude where
    /// the user-described element is, and returns absolute screen coordinates (or
    /// None if Claude says there's no specific element to point at).
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
