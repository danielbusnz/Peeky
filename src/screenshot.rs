use std::process::Command;

pub fn take_area_screenshot(x: i32, y: i32, width: i32, height: i32, filename: &str) {
    let geometry = format!("{},{} {}x{}", x, y, width, height);

    let status = Command::new("grim")
        .args(["-g", &geometry, filename])
        .status()
        .expect("Failed to execute grim");

    if status.success() {
        println!("Screenshot saved to {}", filename);
    } else {
        eprintln!("Error taking screenshot");
    }
}
