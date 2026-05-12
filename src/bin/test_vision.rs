#[path = "../screenshot.rs"]
mod screenshot;

#[path = "../providers/mod.rs"]
mod providers;

fn main() {
    let claude = providers::claude::Claude::from_env().expect("missing ANTHROPIC_API_KEY");

    let (mx, my, mw, mh) = match screenshot::active_workspace_geometry() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("could not query monitor: {}", e);
            return;
        }
    };
    println!("active monitor: {}x{} at ({}, {})", mw, mh, mx, my);

    println!("asking Claude...");
    match claude.detect_element_location(
        "What's on this screen? Answer in one sentence.",
        mx as i64,
        my as i64,
        mw as i64,
        mh as i64,
    ) {
        Ok((text, _point)) => println!("Claude: {}", text),
        Err(e) => eprintln!("Claude error: {}", e),
    }
}
