use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

#[path = "../screenshot/mod.rs"]
mod screenshot;

fn main() {
    let (b64, w, h) = match screenshot::capture_active_workspace() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("capture failed: {}", e);
            return;
        }
    };

    let bytes = BASE64.decode(b64.as_bytes()).expect("base64 decode failed");
    std::fs::write("/tmp/aegis-test.jpg", bytes).expect("write failed");
    println!(
        "captured active workspace {}x{} (saved /tmp/aegis-test.jpg)",
        w, h
    );
}
