//! Quick test of macOS-specific functionality

fn main() {
    println!("=== macOS Functionality Test ===\n");

    // Test 1: Screenshot via xcap
    println!("1. Testing screenshot capture (xcap)...");
    match xcap::Monitor::all() {
        Ok(monitors) => {
            println!("   Found {} monitor(s)", monitors.len());
            for (i, m) in monitors.iter().enumerate() {
                if let (Ok(w), Ok(h), Ok(primary)) = (m.width(), m.height(), m.is_primary()) {
                    println!("   Monitor {}: {}x{} (primary: {})", i, w, h, primary);
                }
            }
            if let Some(m) = monitors
                .into_iter()
                .find(|m| m.is_primary().unwrap_or(false))
            {
                match m.capture_image() {
                    Ok(img) => println!(
                        "   SUCCESS: Captured {}x{} screenshot",
                        img.width(),
                        img.height()
                    ),
                    Err(e) => println!("   FAILED: {}", e),
                }
            }
        }
        Err(e) => println!("   FAILED: {}", e),
    }

    // Test 2: Mouse position
    println!("\n2. Testing mouse position...");
    match mouse_position::mouse_position::Mouse::get_mouse_position() {
        mouse_position::mouse_position::Mouse::Position { x, y } => {
            println!("   SUCCESS: Mouse at ({}, {})", x, y);
        }
        mouse_position::mouse_position::Mouse::Error => {
            println!("   FAILED: Could not get mouse position");
        }
    }

    // Test 3: Check available input tools
    println!("\n3. Checking input injection tools...");
    let tools = [
        ("ydotool", "Linux only - expected to fail"),
        ("cliclick", "brew install cliclick"),
        ("osascript", "built-in macOS"),
    ];
    for (tool, note) in tools {
        let exists = std::process::Command::new("which")
            .arg(tool)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        println!(
            "   {}: {} ({})",
            tool,
            if exists { "AVAILABLE" } else { "missing" },
            note
        );
    }

    // Test 4: Check if we can use osascript for clicks
    println!("\n4. Testing osascript click capability...");
    let test_script =
        r#"tell application "System Events" to get name of first process whose frontmost is true"#;
    match std::process::Command::new("osascript")
        .args(["-e", test_script])
        .output()
    {
        Ok(out) if out.status.success() => {
            let app = String::from_utf8_lossy(&out.stdout).trim().to_string();
            println!("   SUCCESS: Frontmost app is '{}'", app);
            println!("   (osascript can be used for input injection)");
        }
        Ok(out) => {
            println!("   FAILED: {}", String::from_utf8_lossy(&out.stderr));
            println!("   (May need Accessibility permissions)");
        }
        Err(e) => println!("   FAILED: {}", e),
    }

    println!("\n=== Summary ===");
    println!("Screenshot: xcap should work (needs Screen Recording permission)");
    println!("Mouse pos:  mouse_position crate works");
    println!("Clicks:     Need to implement using osascript or cliclick");
    println!("\n=== Test Complete ===");
}
