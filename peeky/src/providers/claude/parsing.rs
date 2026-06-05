//! Tool schema generation and incoming-tool-call parsing for `run_agent_loop`.
//! All functions are crate-private to the `claude` module.

use super::Action;

/// Shared tools array for the agent loop. Accepts extra tool schemas
/// (from integrations, etc.) to append so the function doesn't need to
/// import the integrations module directly.
pub(super) fn tools_array_value(
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
                screen (use the computer tool's left_click for that).",
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
                app (use switch_to_window for that).",
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

    // Anthropic prompt caching: a cache_control marker on the LAST tool
    // tells Anthropic to cache the whole prefix (system + tools). Subsequent
    // requests within the 5-minute TTL pay ~10% input-token cost on this
    // prefix and skip the preprocessing step → faster TTFT on every turn
    // after the first. The user transcript and screenshots in `messages`
    // are AFTER this breakpoint and remain uncached, so they ship fresh
    // each turn. Watch [sse-debug] message_start usage:
    //   cache_creation_input_tokens > 0 on first turn (write)
    //   cache_read_input_tokens > 0 on subsequent turns (hit)
    if let Some(last) = tools.last_mut()
        && let Some(obj) = last.as_object_mut()
    {
        obj.insert(
            "cache_control".to_string(),
            serde_json::json!({ "type": "ephemeral" }),
        );
    }

    serde_json::Value::Array(tools)
}

/// Trim image data from `tool_result` blocks older than the most recent
/// `keep_last_n` screenshots. Replaces the image with a text placeholder
/// so Claude knows there WAS a screenshot at that point, but the bytes
/// are gone. Keeps the conversation graph intact while controlling cost.
pub(super) fn trim_old_screenshots(messages: &mut [serde_json::Value], keep_last_n: usize) {
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
pub(super) fn parse_tool_call(
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
    // string like "[640, 47]"; Claude's malformed shape emits the latter.
    let (raw_x, raw_y) = extract_coordinate(&effective_input["coordinate"])?;
    let raw_x = raw_x.clamp(0, declared_w as i64 - 1);
    let raw_y = raw_y.clamp(0, declared_h as i64 - 1);
    let sx = window_x + (raw_x as f64 * window_width as f64 / declared_w as f64) as i64;
    let sy = window_y + (raw_y as f64 * window_height as f64 / declared_h as f64) as i64;
    let x = sx.clamp(window_x, window_x + window_width - 1);
    let y = sy.clamp(window_y, window_y + window_height - 1);

    match action {
        // Treat right/middle/double/triple clicks as left clicks for now.
        // Most apps treat them similarly for the "I want to interact with
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
    if let Some(arr) = value.as_array()
        && arr.len() == 2
    {
        return Some((arr[0].as_i64()?, arr[1].as_i64()?));
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

#[cfg(test)]
mod tests {
    use super::*;

    // ---- helpers ----

    /// Window geometry used as a neutral "1:1 mapping" baseline so
    /// raw Claude coordinates come out unchanged.
    fn unit_window() -> (i64, i64, i64, i64) {
        (0, 0, 1280, 800)
    }

    fn parse(tool: &str, input: serde_json::Value) -> Option<Action> {
        let (wx, wy, ww, wh) = unit_window();
        parse_tool_call(tool, &input, 1280, 800, wx, wy, ww, wh)
    }

    // ---- cache_control tests (pre-existing, kept) ----

    /// Anthropic prompt caching only kicks in if every request marks the
    /// SAME prefix boundary. If a future refactor strips the cache_control
    /// field off the last tool, requests still succeed but cost ~10x more
    /// and every TTFT jumps ~300-500ms (a silent performance regression).
    /// This test fails loudly if the marker disappears.
    #[test]
    fn last_tool_has_cache_control_marker() {
        let tools = tools_array_value(1280, 800, vec![]);
        let arr = tools
            .as_array()
            .expect("tools_array_value should return a JSON array");
        let last = arr.last().expect("tools array should be non-empty");
        let cache_control = last.get("cache_control").unwrap_or_else(|| {
            panic!(
                "last tool is missing the cache_control marker. prompt caching is OFF.\nlast tool was: {}",
                serde_json::to_string_pretty(last).unwrap_or_default()
            )
        });
        assert_eq!(
            cache_control,
            &serde_json::json!({ "type": "ephemeral" }),
            "cache_control shape changed; Anthropic expects {{ type: ephemeral }}"
        );
    }

    /// Extra defense: the marker should ALSO survive integration tools
    /// being added to the array. integrations::all_tools() is the real
    /// caller, so the marker needs to land on the last appended tool, not
    /// the last hardcoded one.
    #[test]
    fn cache_control_on_last_tool_with_extras() {
        let extra = vec![serde_json::json!({ "name": "fake_extra_tool", "input_schema": {} })];
        let tools = tools_array_value(1280, 800, extra);
        let arr = tools.as_array().expect("array");
        let last = arr.last().expect("non-empty");
        assert_eq!(
            last.get("name").and_then(|v| v.as_str()),
            Some("fake_extra_tool"),
            "extra tool should be appended last"
        );
        assert!(
            last.get("cache_control").is_some(),
            "cache_control must land on the LAST tool after extras are appended"
        );
    }

    // ---- parse_tool_call: click / point ----

    #[test]
    fn left_click_array_coord() {
        let action = parse(
            "computer",
            serde_json::json!({ "action": "left_click", "coordinate": [100, 200] }),
        );
        assert!(matches!(action, Some(Action::Click { x: 100, y: 200 })));
    }

    #[test]
    fn mouse_move_array_coord() {
        let action = parse(
            "computer",
            serde_json::json!({ "action": "mouse_move", "coordinate": [640, 400] }),
        );
        assert!(matches!(action, Some(Action::Point { x: 640, y: 400 })));
    }

    #[test]
    fn right_middle_double_triple_click_mapped_to_click() {
        for action_name in &[
            "right_click",
            "middle_click",
            "double_click",
            "triple_click",
        ] {
            let action = parse(
                "computer",
                serde_json::json!({ "action": action_name, "coordinate": [50, 50] }),
            );
            assert!(
                matches!(action, Some(Action::Click { .. })),
                "{} should map to Click",
                action_name
            );
        }
    }

    // ---- parse_tool_call: action-as-name fallback ----

    #[test]
    fn left_click_as_tool_name_fallback() {
        // Claude sometimes emits name:"left_click" instead of name:"computer"
        // with action:"left_click". Both shapes must produce a Click.
        let action = parse(
            "left_click",
            serde_json::json!({ "coordinate": [300, 150] }),
        );
        assert!(matches!(action, Some(Action::Click { x: 300, y: 150 })));
    }

    #[test]
    fn mouse_move_as_tool_name_fallback() {
        let action = parse("mouse_move", serde_json::json!({ "coordinate": [10, 20] }));
        assert!(matches!(action, Some(Action::Point { x: 10, y: 20 })));
    }

    #[test]
    fn type_as_tool_name_fallback() {
        let action = parse("type", serde_json::json!({ "text": "hello" }));
        assert!(matches!(action, Some(Action::Type { text }) if text == "hello"));
    }

    // ---- parse_tool_call: string-encoded coordinates ----

    #[test]
    fn string_coordinate_parses() {
        // Claude's malformed shape: coordinate arrives as the JSON string "[640, 47]"
        let action = parse(
            "computer",
            serde_json::json!({ "action": "left_click", "coordinate": "[640, 47]" }),
        );
        assert!(matches!(action, Some(Action::Click { x: 640, y: 47 })));
    }

    #[test]
    fn string_coordinate_with_spaces_parses() {
        let action = parse(
            "computer",
            serde_json::json!({ "action": "mouse_move", "coordinate": "[ 100 , 200 ]" }),
        );
        assert!(matches!(action, Some(Action::Point { x: 100, y: 200 })));
    }

    #[test]
    fn string_coordinate_malformed_returns_none() {
        let action = parse(
            "computer",
            serde_json::json!({ "action": "left_click", "coordinate": "not-a-coord" }),
        );
        assert!(action.is_none());
    }

    #[test]
    fn missing_coordinate_returns_none() {
        let action = parse("computer", serde_json::json!({ "action": "left_click" }));
        assert!(action.is_none());
    }

    // ---- parse_tool_call: coordinate clamping ----

    #[test]
    fn coordinate_clamped_to_declared_bounds() {
        // x=9999 is way outside 1280 wide; should clamp to 1279 before mapping.
        let action = parse_tool_call(
            "computer",
            &serde_json::json!({ "action": "left_click", "coordinate": [9999, 9999] }),
            1280,
            800,
            0,
            0,
            1280,
            800,
        );
        match action {
            Some(Action::Click { x, y }) => {
                assert!(x < 1280, "x must be clamped below declared width: {}", x);
                assert!(y < 800, "y must be clamped below declared height: {}", y);
            }
            other => panic!("expected Click, got {:?}", other),
        }
    }

    #[test]
    fn coordinate_zero_zero_is_valid() {
        let action = parse(
            "computer",
            serde_json::json!({ "action": "left_click", "coordinate": [0, 0] }),
        );
        assert!(matches!(action, Some(Action::Click { x: 0, y: 0 })));
    }

    // ---- parse_tool_call: window offset scaling ----

    #[test]
    fn window_offset_applied_to_coordinates() {
        // Window is offset by (100, 50) with same size as declared resolution.
        // A click at [0,0] in Claude's coordinate space should land at (100, 50).
        let action = parse_tool_call(
            "computer",
            &serde_json::json!({ "action": "left_click", "coordinate": [0, 0] }),
            1280,
            800,
            100,
            50,
            1280,
            800,
        );
        assert!(matches!(action, Some(Action::Click { x: 100, y: 50 })));
    }

    // ---- parse_tool_call: type ----

    #[test]
    fn type_action_happy_path() {
        let action = parse(
            "computer",
            serde_json::json!({ "action": "type", "text": "Hello world" }),
        );
        match action {
            Some(Action::Type { text }) => assert_eq!(text, "Hello world"),
            other => panic!("expected Type, got {:?}", other),
        }
    }

    #[test]
    fn type_action_missing_text_returns_none() {
        let action = parse("computer", serde_json::json!({ "action": "type" }));
        assert!(action.is_none());
    }

    #[test]
    fn type_with_trailing_newline_preserved() {
        let action = parse(
            "computer",
            serde_json::json!({ "action": "type", "text": "search term\n" }),
        );
        assert!(matches!(action, Some(Action::Type { text }) if text.ends_with('\n')));
    }

    // ---- parse_tool_call: key ----

    #[test]
    fn key_action_happy_path() {
        let action = parse(
            "computer",
            serde_json::json!({ "action": "key", "text": "ctrl+a" }),
        );
        match action {
            Some(Action::Key { key }) => assert_eq!(key, "ctrl+a"),
            other => panic!("expected Key, got {:?}", other),
        }
    }

    #[test]
    fn key_action_missing_text_returns_none() {
        let action = parse("computer", serde_json::json!({ "action": "key" }));
        assert!(action.is_none());
    }

    // ---- parse_tool_call: scroll ----

    #[test]
    fn scroll_down_happy_path() {
        let action = parse(
            "computer",
            serde_json::json!({ "action": "scroll", "scroll_direction": "down", "scroll_amount": 3 }),
        );
        match action {
            Some(Action::Scroll { direction, amount }) => {
                assert_eq!(direction, "down");
                assert_eq!(amount, 3);
            }
            other => panic!("expected Scroll, got {:?}", other),
        }
    }

    #[test]
    fn scroll_up_direction() {
        let action = parse(
            "computer",
            serde_json::json!({ "action": "scroll", "scroll_direction": "up", "scroll_amount": 5 }),
        );
        assert!(matches!(action, Some(Action::Scroll { direction, .. }) if direction == "up"));
    }

    #[test]
    fn scroll_amount_as_string() {
        // scroll_amount may arrive as a stringified integer from Claude.
        let action = parse(
            "computer",
            serde_json::json!({ "action": "scroll", "scroll_direction": "down", "scroll_amount": "7" }),
        );
        assert!(matches!(action, Some(Action::Scroll { amount: 7, .. })));
    }

    #[test]
    fn scroll_missing_direction_defaults_to_down() {
        let action = parse(
            "computer",
            serde_json::json!({ "action": "scroll", "scroll_amount": 2 }),
        );
        assert!(matches!(action, Some(Action::Scroll { direction, .. }) if direction == "down"));
    }

    #[test]
    fn scroll_missing_amount_defaults_to_three() {
        let action = parse(
            "computer",
            serde_json::json!({ "action": "scroll", "scroll_direction": "up" }),
        );
        assert!(matches!(action, Some(Action::Scroll { amount: 3, .. })));
    }

    // ---- parse_tool_call: unknown computer action ----

    #[test]
    fn unknown_computer_action_returns_none() {
        let action = parse("computer", serde_json::json!({ "action": "screenshot" }));
        assert!(
            action.is_none(),
            "screenshot action should not produce an Action"
        );
    }

    #[test]
    fn computer_with_no_action_field_returns_none() {
        let action = parse("computer", serde_json::json!({}));
        assert!(action.is_none());
    }

    // ---- parse_tool_call: custom tools ----

    #[test]
    fn open_url_happy_path() {
        let action = parse(
            "open_url",
            serde_json::json!({ "url": "https://example.com" }),
        );
        match action {
            Some(Action::OpenUrl { url }) => assert_eq!(url, "https://example.com"),
            other => panic!("expected OpenUrl, got {:?}", other),
        }
    }

    #[test]
    fn open_url_missing_url_returns_none() {
        let action = parse("open_url", serde_json::json!({}));
        assert!(action.is_none());
    }

    #[test]
    fn launch_app_happy_path() {
        let action = parse("launch_app", serde_json::json!({ "app": "spotify" }));
        match action {
            Some(Action::LaunchApp { app }) => assert_eq!(app, "spotify"),
            other => panic!("expected LaunchApp, got {:?}", other),
        }
    }

    #[test]
    fn launch_app_missing_app_returns_none() {
        let action = parse("launch_app", serde_json::json!({}));
        assert!(action.is_none());
    }

    #[test]
    fn switch_to_window_happy_path() {
        let action = parse(
            "switch_to_window",
            serde_json::json!({ "target": "firefox" }),
        );
        match action {
            Some(Action::SwitchToWindow { target }) => assert_eq!(target, "firefox"),
            other => panic!("expected SwitchToWindow, got {:?}", other),
        }
    }

    #[test]
    fn switch_to_window_missing_target_returns_none() {
        let action = parse("switch_to_window", serde_json::json!({}));
        assert!(action.is_none());
    }

    #[test]
    fn unknown_tool_name_returns_integration() {
        // Any name not in the built-in list routes to Action::Integration.
        let action = parse("spotify_play", serde_json::json!({ "track": "Despacito" }));
        assert!(matches!(action, Some(Action::Integration)));
    }

    // ---- trim_old_screenshots ----

    fn make_screenshot_msg(has_image: bool) -> serde_json::Value {
        if has_image {
            serde_json::json!({
                "role": "user",
                "content": [{ "type": "image", "source": { "type": "base64", "data": "AAAA" } }]
            })
        } else {
            serde_json::json!({
                "role": "user",
                "content": [{ "type": "text", "text": "some user text" }]
            })
        }
    }

    fn make_tool_result_msg(has_image: bool) -> serde_json::Value {
        let content = if has_image {
            serde_json::json!([
                { "type": "image", "source": { "type": "base64", "data": "BBBB" } }
            ])
        } else {
            serde_json::json!([{ "type": "text", "text": "ok" }])
        };
        serde_json::json!({
            "role": "user",
            "content": [{ "type": "tool_result", "content": content }]
        })
    }

    #[test]
    fn trim_keeps_last_n_direct_images() {
        // 5 user messages each carrying a direct image block.
        // keep_last_n=2 should preserve the 2 most recent, replace the 3 older ones.
        let mut msgs: Vec<serde_json::Value> = (0..5).map(|_| make_screenshot_msg(true)).collect();
        trim_old_screenshots(&mut msgs, 2);

        let mut remaining_images = 0usize;
        let mut placeholders = 0usize;
        for msg in &msgs {
            if msg["role"] != "user" {
                continue;
            }
            for block in msg["content"].as_array().unwrap() {
                if block["type"] == "image" {
                    remaining_images += 1;
                }
                if block["type"] == "text" && block["text"] == "[older screenshot omitted]" {
                    placeholders += 1;
                }
            }
        }
        assert_eq!(remaining_images, 2, "2 most recent images should survive");
        assert_eq!(
            placeholders, 3,
            "3 older images should become placeholder text"
        );
    }

    #[test]
    fn trim_keeps_last_n_tool_result_images() {
        // 4 tool_result messages each carrying an image.
        let mut msgs: Vec<serde_json::Value> = (0..4).map(|_| make_tool_result_msg(true)).collect();
        trim_old_screenshots(&mut msgs, 1);

        let mut remaining = 0usize;
        let mut placeholders = 0usize;
        for msg in &msgs {
            if let Some(content) = msg["content"].as_array() {
                for block in content {
                    if block["type"] != "tool_result" {
                        continue;
                    }
                    let Some(inner) = block["content"].as_array() else {
                        continue;
                    };
                    for item in inner {
                        if item["type"] == "image" {
                            remaining += 1;
                        }
                        if item["type"] == "text" && item["text"] == "[older screenshot omitted]" {
                            placeholders += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(remaining, 1);
        assert_eq!(placeholders, 3);
    }

    #[test]
    fn trim_keep_zero_replaces_all_images() {
        let mut msgs = vec![make_screenshot_msg(true), make_screenshot_msg(true)];
        trim_old_screenshots(&mut msgs, 0);
        for msg in &msgs {
            for block in msg["content"].as_array().unwrap() {
                assert_ne!(block["type"], "image", "all images should be replaced");
            }
        }
    }

    #[test]
    fn trim_non_user_messages_untouched() {
        // Assistant messages should never be modified.
        let mut msgs = vec![serde_json::json!({
            "role": "assistant",
            "content": [{ "type": "image", "source": {} }]
        })];
        trim_old_screenshots(&mut msgs, 0);
        assert_eq!(
            msgs[0]["content"][0]["type"], "image",
            "assistant image must not be touched"
        );
    }

    #[test]
    fn trim_no_op_when_fewer_than_keep_last_n() {
        let mut msgs = vec![make_screenshot_msg(true)];
        trim_old_screenshots(&mut msgs, 5);
        assert_eq!(
            msgs[0]["content"][0]["type"], "image",
            "image should survive when count <= keep_last_n"
        );
    }

    #[test]
    fn trim_mixed_image_and_text_blocks_preserves_text() {
        let mut msgs = vec![serde_json::json!({
            "role": "user",
            "content": [
                { "type": "text", "text": "user said something" },
                { "type": "image", "source": { "type": "base64", "data": "XX" } }
            ]
        })];
        trim_old_screenshots(&mut msgs, 0);
        let content = msgs[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text", "text block should be untouched");
        assert_ne!(
            content[1]["type"], "image",
            "image should have been replaced"
        );
    }
}
