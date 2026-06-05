#![allow(dead_code)]

// Isolated first-run OAuth + Gmail integration smoke test.
//
// Run: cargo run --bin demo_gmail
//
// First invocation opens the system browser to Google's consent page,
// captures the loopback redirect, and writes ~/.config/peeky/gmail_token.json.
// Subsequent invocations reuse the cached refresh token.

use peeky::integrations::gmail;

fn main() {
    if !gmail::is_available() {
        eprintln!(
            "gmail::is_available() == false. Did you export PEEKY_GMAIL_CLIENT_ID and \
             PEEKY_GMAIL_CLIENT_SECRET (or put them in .env)?"
        );
        std::process::exit(1);
    }
    println!("[test_gmail] env vars present, dispatching gmail_unread_count...");
    println!("[test_gmail] first run will open your browser. Click Advanced > Continue on the");
    println!("[test_gmail] unverified app warning, then accept the four Gmail scopes.\n");

    let input = serde_json::json!({});
    let out = gmail::dispatch("gmail_unread_count", &input)
        .unwrap_or_else(|| "<dispatch returned None>".to_string());
    println!("\n[test_gmail] result: {out}");
}
