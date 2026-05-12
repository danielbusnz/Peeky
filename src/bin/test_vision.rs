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

    let (b64, _, _) = match screenshot::capture_for_claude(mx, my, mw as i32, mh as i32) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("capture failed: {}", e);
            return;
        }
    };

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    println!("asking Claude (describe_with_image, streaming)...");
    print!("Claude: ");
    let result = rt.block_on(async {
        claude
            .describe_with_image(
                "What's on this screen? Answer in one sentence.",
                &b64,
                |token| {
                    print!("{}", token);
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                },
            )
            .await
    });
    println!();

    if let Err(e) = result {
        eprintln!("Claude error: {}", e);
    }
}
