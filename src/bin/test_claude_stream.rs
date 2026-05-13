#[path = "../screenshot/mod.rs"]
mod screenshot;
#[path = "../providers/mod.rs"]
mod providers;

fn main() {
    let claude = providers::claude::Claude::from_env()
        .expect("missing ANTHROPIC_API_KEY");

    let rt = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");

    let prompt = "Count from one to ten slowly, with a brief comment between each number.";
    println!("prompt: {}\n\nresponse:", prompt);

    let start = std::time::Instant::now();
    let mut first_token_at: Option<std::time::Duration> = None;

    let result = rt.block_on(async {
        claude
            .complete_stream(prompt, |token| {
                if first_token_at.is_none() {
                    first_token_at = Some(start.elapsed());
                }
                print!("{}", token);
                use std::io::Write;
                std::io::stdout().flush().ok();
            })
            .await
    });

    println!("\n");

    match result {
        Ok(text) => {
            println!("=== stats ===");
            println!("first token: {:?}", first_token_at.unwrap_or(start.elapsed()));
            println!("total time:  {:?}", start.elapsed());
            println!("total chars: {}", text.len());
        }
        Err(e) => {
            eprintln!("stream failed: {}", e);
        }
    }
}
