#![allow(dead_code)]

// Smoke test for the github integration tools.
// Run: cargo run --bin demo_github

use peeky::integrations::github;

use serde_json::Value;

fn step(n: u32, label: &str) {
    println!("\n=== step {n}: {label} ===");
}

fn dispatch(tool: &str, input: Value) -> Value {
    println!("[call] {tool} input={input}");
    let raw = github::dispatch(tool, &input).expect("dispatch returned None");
    match serde_json::from_str::<Value>(&raw) {
        Ok(v) => {
            let pretty = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
            let preview: String = pretty.chars().take(500).collect();
            println!(
                "[ret ] {preview}{}",
                if pretty.len() > 500 { "..." } else { "" }
            );
            v
        }
        Err(_) => {
            println!("[ret ] (non-json) {raw}");
            Value::String(raw)
        }
    }
}

fn main() {
    if !github::is_available() {
        eprintln!("github::is_available() == false. Is `gh` installed and `gh auth status` clean?");
        std::process::exit(1);
    }
    println!("github integration available, exercising tools...\n");

    step(1, "gh_my_prs (state=open)");
    dispatch(
        "gh_my_prs",
        serde_json::json!({ "state": "open", "limit": 3 }),
    );

    step(2, "gh_my_issues (state=open)");
    let issues = dispatch(
        "gh_my_issues",
        serde_json::json!({ "state": "open", "limit": 3 }),
    );

    // If we got at least one issue back, pick the first one and view it.
    if let Some(arr) = issues.as_array()
        && let Some(first) = arr.first()
    {
        let repo = first["repository"]["nameWithOwner"].as_str().unwrap_or("");
        let number = first["number"].as_i64().unwrap_or(0);
        if !repo.is_empty() && number > 0 {
            step(3, &format!("gh_issue_view ({repo}#{number})"));
            dispatch(
                "gh_issue_view",
                serde_json::json!({ "repo": repo, "number": number }),
            );
        }
    }

    step(4, "gh_notifications");
    dispatch("gh_notifications", serde_json::json!({ "limit": 3 }));

    step(5, "gh_repo_view (cli/cli as a known public repo)");
    dispatch("gh_repo_view", serde_json::json!({ "repo": "cli/cli" }));

    step(6, "gh_actions_status (cli/cli)");
    dispatch(
        "gh_actions_status",
        serde_json::json!({ "repo": "cli/cli" }),
    );

    println!("\n=== all gh integration tools exercised ===");
}
