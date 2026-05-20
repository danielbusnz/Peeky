// End-to-end exercise of every gmail integration tool.
//
// Sends one self-addressed test email, then runs the other six tools
// against that single message. Cleanup is "search subject:aegis-test
// in Gmail and delete".
//
// Run: cargo run --bin test_gmail_full

#[path = "../src/integrations/gmail.rs"]
mod gmail;

use serde_json::Value;
use std::thread::sleep;
use std::time::Duration;

const SELF_ADDR: &str = "danielbusnz+aegis@gmail.com";

fn step(n: u32, label: &str) {
    println!("\n=== step {n}: {label} ===");
}

fn dispatch_or_die(tool: &str, input: Value) -> Value {
    println!("[call] {tool} input={input}");
    let raw = gmail::dispatch(tool, &input).unwrap_or_else(|| {
        eprintln!("dispatch returned None for {tool}");
        std::process::exit(1);
    });
    match serde_json::from_str::<Value>(&raw) {
        Ok(v) => {
            println!("[ret ] {v}");
            v
        }
        Err(_) => {
            println!("[ret ] (non-json) {raw}");
            Value::String(raw)
        }
    }
}

fn main() {
    if !gmail::is_available() {
        eprintln!("gmail::is_available() == false. Check .env.");
        std::process::exit(1);
    }

    let tag = format!(
        "aegis-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );
    println!("using tag: {tag}");

    step(1, "gmail_unread_count baseline");
    let before = dispatch_or_die("gmail_unread_count", serde_json::json!({}));
    let unread_before = before["unread"].as_i64().unwrap_or(-1);
    println!("unread before send: {unread_before}");

    step(2, "gmail_send (self-addressed test email)");
    dispatch_or_die(
        "gmail_send",
        serde_json::json!({
            "to": SELF_ADDR,
            "subject": tag,
            "body": format!("This is an automated end-to-end test for the aegis Gmail \
                            integration. Tag: {tag}. Safe to delete."),
        }),
    );

    println!("waiting 6s for Gmail to ingest...");
    sleep(Duration::from_secs(6));

    step(3, "gmail_search (find the message we just sent)");
    let search = dispatch_or_die(
        "gmail_search",
        serde_json::json!({ "query": format!("subject:{tag}"), "max_results": 5 }),
    );
    let arr = search.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        eprintln!("search returned 0 results. Gmail may need a few more seconds, \
                   or the send silently failed.");
        std::process::exit(1);
    }
    let msg_id = arr[0]["id"].as_str().unwrap_or("").to_string();
    println!("found message id: {msg_id}");

    step(4, "gmail_read (full body of that message)");
    let read = dispatch_or_die("gmail_read", serde_json::json!({ "id": msg_id }));
    let subject = read["subject"].as_str().unwrap_or("");
    let body_preview = read["body"].as_str().unwrap_or("").chars().take(80).collect::<String>();
    println!("subject='{subject}' body_preview='{body_preview}'");

    step(5, "gmail_draft (compose a reply draft, do not send)");
    dispatch_or_die(
        "gmail_draft",
        serde_json::json!({
            "to": SELF_ADDR,
            "subject": format!("Re: {tag}"),
            "body": "draft created by test_gmail_full, safe to delete.",
        }),
    );

    step(6, "gmail_mark_read (remove UNREAD label from test message)");
    dispatch_or_die("gmail_mark_read", serde_json::json!({ "id": msg_id }));

    step(7, "gmail_archive (remove INBOX label from test message)");
    dispatch_or_die("gmail_archive", serde_json::json!({ "id": msg_id }));

    step(8, "gmail_unread_count delta check");
    let after = dispatch_or_die("gmail_unread_count", serde_json::json!({}));
    let unread_after = after["unread"].as_i64().unwrap_or(-1);
    println!("unread after: {unread_after} (was {unread_before})");

    println!("\n=== ALL STEPS COMPLETED ===");
    println!("Cleanup: in Gmail, search 'subject:{tag}' and delete the message + draft.");
}
