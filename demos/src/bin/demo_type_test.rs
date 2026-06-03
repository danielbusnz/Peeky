//! Self-verifying test that macOS text injection actually enters text.
//!
//! Opens a fresh TextEdit document, types a known string via the aegis input
//! backend, reads the document back, and reports PASS/FAIL. Run with:
//!   cargo run -p aegis-demos --bin demo_type_test
//!
//! macOS only; the running terminal needs Accessibility permission.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("demo_type_test is macOS-only");
}

#[cfg(target_os = "macos")]
fn main() {
    use aegis::actions;
    use std::process::Command;
    use std::thread;
    use std::time::Duration;

    const EXPECTED: &str = "the quick brown fox 12345";

    // Run a one-line AppleScript and return its trimmed stdout.
    fn osa(script: &str) -> String {
        let out = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .expect("osascript failed to run");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    println!("=== aegis text-entry test ===");
    actions::init_input_executor();
    actions::check_input_injection_available();

    // Fresh TextEdit document, frontmost so the synthetic keystrokes land in it.
    osa("tell application \"TextEdit\" to activate");
    osa("tell application \"TextEdit\" to make new document");
    osa("tell application \"TextEdit\" to activate");
    thread::sleep(Duration::from_millis(1200));

    println!("typing: {EXPECTED:?}");
    actions::type_text(EXPECTED);
    thread::sleep(Duration::from_secs(1));

    let got = osa("tell application \"TextEdit\" to get text of front document");
    println!("read back: {got:?}");

    if got == EXPECTED {
        println!("\nPASS: text entered correctly");
    } else {
        println!("\nFAIL: expected {EXPECTED:?}, got {got:?}");
    }

    // Clean up the scratch document without saving.
    osa("tell application \"TextEdit\" to close front document saving no");
}
