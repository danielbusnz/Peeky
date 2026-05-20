#![allow(dead_code)]

#[path = "../src/screenshot/mod.rs"]
mod screenshot;

#[path = "../src/tuning.rs"]
mod tuning;

#[path = "../src/providers/mod.rs"]
mod providers;

use tokio_util::sync::CancellationToken;

fn main() {
    let claude = providers::claude::Claude::from_env(reqwest::Client::new())
        .expect("missing ANTHROPIC_API_KEY");

    let (mx, my, mw, mh) = match screenshot::active_workspace_geometry() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("could not query monitor: {}", e);
            return;
        }
    };
    println!("active monitor: {}x{} at ({}, {})", mw, mh, mx, my);

    let (dw, dh) = screenshot::pick_declared_resolution(mw as i64, mh as i64);
    let b64 = match screenshot::capture_resized_for_claude(mx, my, mw as i32, mh as i32, dw, dh) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("capture failed: {}", e);
            return;
        }
    };

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    println!("asking Claude (run_agent_loop)...");
    print!("Claude: ");
    let result = rt.block_on(async {
        // test_vision is one-shot: if Claude tries to take more screenshots
        // (it shouldn't, given the prompt), surface that as an error.
        let take_screenshot = || -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
            Err("test_vision is one-shot; no follow-up screenshots".into())
        };
        claude
            .run_agent_loop(
                "Describe what's on this screen in one sentence. Do not call any tools — \
                 answer with plain text only.",
                &b64,
                &[],
                None,
                mx as i64,
                my as i64,
                mw as i64,
                mh as i64,
                vec![],
                CancellationToken::new(),
                take_screenshot,
                |_action| {},
                |_name, _input| None,
                |token| {
                    print!("{}", token);
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                },
            )
            .await
    });

    match result {
        Ok(text) => {
            // run_agent_loop only streams text mid-chain (post-integration);
            // on a clean one-shot it returns the final text in `text`.
            if !text.is_empty() {
                print!("{}", text);
            }
            println!();
        }
        Err(e) => {
            println!();
            eprintln!("Claude error: {}", e);
        }
    }
}
