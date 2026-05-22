//! Test macOS native input injection (click, type, key)
//!
//! Run with: cargo run --no-default-features --features winit-window --bin demo_macos_input

#[path = "../src/actions.rs"]
mod actions;

use std::thread;
use std::time::Duration;

fn main() {
    println!("=== macOS Input Injection Test ===\n");
    println!("This will test click, type, and key press functionality.");
    println!("Make sure you have Accessibility permissions enabled.\n");

    // Initialize the input executor (required before any input commands)
    actions::init_input_executor();

    // Check availability
    println!("Checking input injection availability...");
    actions::check_input_injection_available();

    println!("\nStarting tests in 3 seconds...");
    println!("Open TextEdit or Notes and click in the text area!");
    thread::sleep(Duration::from_secs(3));

    // Test 1: Click at current mouse position (roughly center screen)
    println!("\n1. Testing click at (500, 500)...");
    actions::click_at(500, 500);
    thread::sleep(Duration::from_millis(500));

    // Test 2: Type some text
    println!("2. Testing type_text...");
    actions::type_text("Hello from Aegis! ");
    thread::sleep(Duration::from_millis(500));

    // Test 3: Press key combo (Cmd+A to select all)
    println!("3. Testing press_key (cmd+a)...");
    actions::press_key("cmd+a");
    thread::sleep(Duration::from_millis(500));

    // Test 4: Type more text (replacing selection)
    println!("4. Testing type_text with newline...");
    actions::type_text("Replaced text!\n");
    thread::sleep(Duration::from_millis(500));

    // Test 5: Scroll
    println!("5. Testing scroll down...");
    actions::scroll("down", 3);

    // Wait for executor to process all commands
    thread::sleep(Duration::from_secs(1));

    println!("\n=== Test Complete ===");
    println!("Check if the text was typed and commands executed properly.");
}
