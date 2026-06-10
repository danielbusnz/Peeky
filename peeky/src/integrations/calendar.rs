//! Calendar integration via AppleScript (`osascript`). macOS only. Creates
//! events and lists today's agenda. No install, no auth.
//!
//! Dates are built with AppleScript date arithmetic (`(current date) + n *
//! minutes`) instead of `date "..."` literals, which parse with the machine's
//! locale and break across regions. Event start times therefore arrive as
//! relative offsets in minutes; absolute times ("June 12 at 3pm") would need
//! the current datetime injected into the prompt, which we don't do yet.
//!
//! Known limitation: `calendar_list_today` filters on each event's master
//! start date, so a recurrence of an older repeating event does not show up.
//! Calendar can also take seconds to answer on large databases; integration
//! actions are not early-cancelled, so the reply still flows back to Claude.

use super::applescript;

/// True on macOS, where `osascript` can drive Calendar. No app install needed
/// (Calendar ships with macOS).
pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// JSON tool schemas Claude sees. Names are globally unique, prefixed `calendar_`.
pub fn tools() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "calendar_add_event",
            "description": "Add an event to the calendar, starting a given number \
                of minutes from now. Use for 'add a meeting in an hour', 'block \
                30 minutes for lunch'. Only relative times are supported.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "The event title, e.g. 'dentist'."
                    },
                    "offset_minutes": {
                        "type": "integer",
                        "description": "Minutes from now until the event starts, e.g. 60 \
                            for 'in an hour'."
                    },
                    "duration_minutes": {
                        "type": "integer",
                        "description": "Event length in minutes. Defaults to 60."
                    }
                },
                "required": ["title", "offset_minutes"]
            }
        }),
        serde_json::json!({
            "name": "calendar_list_today",
            "description": "List today's calendar events (title and start time) \
                across all calendars. Use for 'what's on my calendar', 'what do I \
                have today'.",
            "input_schema": { "type": "object", "properties": {} }
        }),
    ]
}

pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "calendar_add_event" => {
            let title = match input["title"].as_str() {
                Some(t) => t,
                None => return Some(err_body("calendar_add_event missing 'title' field")),
            };
            let offset = match input["offset_minutes"].as_i64() {
                Some(o) => o,
                None => {
                    return Some(err_body(
                        "calendar_add_event missing 'offset_minutes' field",
                    ));
                }
            };
            let duration = input["duration_minutes"].as_i64().unwrap_or(60);
            if duration <= 0 {
                return Some(err_body(
                    "calendar_add_event 'duration_minutes' must be positive",
                ));
            }
            Some(add_event(title, offset, duration))
        }
        "calendar_list_today" => Some(list_today()),
        _ => None,
    }
}

/// JSON-encoded `{"error": "..."}` so failures reach Claude as tool_result
/// content, matching the shape the other integrations use.
fn err_body(msg: &str) -> String {
    format!(
        r#"{{"error":{}}}"#,
        serde_json::Value::String(msg.to_string())
    )
}

/// Create an event in the first writable calendar (the default calendar can be
/// a read-only subscription). Offsets are integers straight into date
/// arithmetic, so only the title needs escaping. The doubled braces are
/// format!'s escape for the literal `{ }` of AppleScript's property record.
fn add_event(title: &str, offset_minutes: i64, duration_minutes: i64) -> String {
    let script = format!(
        "set startDate to (current date) + ({offset_minutes} * minutes)\n\
         set endDate to startDate + ({duration_minutes} * minutes)\n\
         tell application \"Calendar\"\n\
         tell (first calendar whose writable is true)\n\
         make new event with properties {{summary:\"{}\", start date:startDate, end date:endDate}}\n\
         end tell\n\
         end tell",
        applescript::escape(title)
    );
    match applescript::run(&script) {
        Ok(_) => "{}".to_string(),
        Err(e) => err_body(&format!("calendar_add_event failed: {e}")),
    }
}

/// Data-returning: today's events as `{"events":[{"title","time"}]}`. The
/// script emits one `title<tab>time` line per event using AppleScript's `tab`
/// and `linefeed` constants, which we split back apart here. `time string`
/// formats per the machine's locale, which is fine for speech.
fn list_today() -> String {
    let script = r#"set dayStart to current date
set time of dayStart to 0
set dayEnd to dayStart + (1 * days)
set out to ""
tell application "Calendar"
    repeat with c in calendars
        repeat with e in (every event of c whose start date is greater than or equal to dayStart and start date is less than dayEnd)
            set out to out & (summary of e) & tab & (time string of (start date of e)) & linefeed
        end repeat
    end repeat
end tell
return out"#;

    match applescript::run(script) {
        Ok(raw) => parse_event_lines(&raw),
        Err(e) => err_body(&format!("calendar_list_today failed: {e}")),
    }
}

/// Parse the `title<tab>time` lines `list_today` produces into
/// `{"events":[{"title","time"}]}`. Split out from `list_today` so it can be
/// unit-tested without running osascript.
fn parse_event_lines(raw: &str) -> String {
    let events: Vec<serde_json::Value> = raw
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut parts = line.splitn(2, '\t');
            let title = parts.next().unwrap_or("");
            let time = parts.next().unwrap_or("");
            serde_json::json!({ "title": title, "time": time })
        })
        .collect();

    serde_json::json!({ "events": events }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The osascript-backed functions need macOS and a populated Calendar, so
    // they are verified by hand. These cover the pure logic.

    #[test]
    fn tools_exposes_the_two_expected_names() {
        let schemas = tools();
        let names: Vec<&str> = schemas.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["calendar_add_event", "calendar_list_today"]);
    }

    #[test]
    fn dispatch_missing_title_returns_error_not_panic() {
        let out = dispatch(
            "calendar_add_event",
            &serde_json::json!({ "offset_minutes": 5 }),
        )
        .expect("dispatch owns calendar_add_event");
        assert!(out.contains("error"), "expected error body, got {out}");
        assert!(out.contains("missing 'title'"));
    }

    #[test]
    fn dispatch_missing_offset_returns_error_not_panic() {
        let out = dispatch("calendar_add_event", &serde_json::json!({ "title": "x" }))
            .expect("dispatch owns calendar_add_event");
        assert!(out.contains("missing 'offset_minutes'"));
    }

    #[test]
    fn dispatch_rejects_nonpositive_duration() {
        let out = dispatch(
            "calendar_add_event",
            &serde_json::json!({ "title": "x", "offset_minutes": 5, "duration_minutes": 0 }),
        )
        .expect("dispatch owns calendar_add_event");
        assert!(out.contains("must be positive"));
    }

    #[test]
    fn dispatch_unknown_tool_returns_none() {
        assert!(dispatch("not_a_calendar_tool", &serde_json::json!({})).is_none());
    }

    #[test]
    fn parse_event_lines_builds_one_entry_per_line() {
        let raw = "standup\t9:30:00 AM\ndentist\t3:00:00 PM\n";
        let parsed: serde_json::Value = serde_json::from_str(&parse_event_lines(raw)).unwrap();
        let events = parsed["events"].as_array().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["title"], "standup");
        assert_eq!(events[0]["time"], "9:30:00 AM");
        assert_eq!(events[1]["title"], "dentist");
    }

    #[test]
    fn parse_event_lines_handles_empty_and_timeless() {
        let empty: serde_json::Value = serde_json::from_str(&parse_event_lines("")).unwrap();
        assert_eq!(empty["events"].as_array().unwrap().len(), 0);

        let no_time: serde_json::Value =
            serde_json::from_str(&parse_event_lines("standup\n")).unwrap();
        assert_eq!(no_time["events"][0]["title"], "standup");
        assert_eq!(no_time["events"][0]["time"], "");
    }
}
