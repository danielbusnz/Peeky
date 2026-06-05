//! GitHub integration via the `gh` CLI.
//!
//! Setup the user does once:
//! ```text
//! sudo pacman -S github-cli   # (or apt/dnf/brew equivalent)
//! gh auth login               # browser OAuth, stores token in ~/.config/gh
//! ```
//!
//! Aegis does not store GitHub credentials. `gh` handles auth, refresh, and
//! token storage. We just shell out.

use std::process::{Command, Stdio};

/// True iff `gh` is on PATH AND already authenticated. Both checks
/// because `gh` installed without auth would still surface tools to
/// Claude but every call would fail.
pub fn is_available() -> bool {
    let has_bin = Command::new("which")
        .arg("gh")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !has_bin {
        return false;
    }
    Command::new("gh")
        .args(["auth", "status"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Tool schemas this integration adds to Claude's tools array.
pub fn tools() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "gh_my_prs",
            "description": "List the user's pull requests across ALL of GitHub (not just one repo). \
                Use for 'show me my PRs', 'do I have open pull requests', 'what's pending review'. \
                Returns id/title/repo/state/url/createdAt for each match.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "state": {
                        "type": "string",
                        "enum": ["open", "closed", "merged"],
                        "description": "PR state filter. Default: open."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max results (default 10, cap 25).",
                        "minimum": 1, "maximum": 25
                    }
                }
            }
        }),
        serde_json::json!({
            "name": "gh_pr_view",
            "description": "Fetch detailed info about a specific pull request: body, state, status checks, reviews, diff size. \
                Use after gh_my_prs returns a hit, or when the user names a specific PR.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "Repository in 'owner/name' format." },
                    "number": { "type": "integer", "description": "PR number." }
                },
                "required": ["repo", "number"]
            }
        }),
        serde_json::json!({
            "name": "gh_my_issues",
            "description": "List the user's issues across ALL of GitHub. \
                Use for 'what issues do I have open', 'show me my GitHub issues'.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "state": {
                        "type": "string",
                        "enum": ["open", "closed"],
                        "description": "Issue state filter. Default: open."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max results (default 10, cap 25).",
                        "minimum": 1, "maximum": 25
                    }
                }
            }
        }),
        serde_json::json!({
            "name": "gh_issue_view",
            "description": "Fetch detailed info about a specific GitHub issue: body, state, labels, comment count.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "Repository in 'owner/name' format." },
                    "number": { "type": "integer", "description": "Issue number." }
                },
                "required": ["repo", "number"]
            }
        }),
        serde_json::json!({
            "name": "gh_actions_status",
            "description": "Get the last 5 GitHub Actions workflow runs for a repo. \
                Use for 'is CI passing for X', 'what's the build status of X'.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "Repository in 'owner/name' format." }
                },
                "required": ["repo"]
            }
        }),
        serde_json::json!({
            "name": "gh_notifications",
            "description": "Fetch the user's GitHub notification inbox: review requests, mentions, CI failures, etc. \
                Use for 'do I have GitHub notifications', 'any review requests'.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "integer",
                        "description": "Max notifications (default 10, cap 25).",
                        "minimum": 1, "maximum": 25
                    }
                }
            }
        }),
        serde_json::json!({
            "name": "gh_repo_view",
            "description": "Summary of a GitHub repository: name, description, stars, default branch, visibility.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "Repository in 'owner/name' format." }
                },
                "required": ["repo"]
            }
        }),
    ]
}

/// Returns `Some(json)` if this integration owned the tool, `None`
/// otherwise. `json` is either the gh CLI's `--json` output verbatim or
/// `{"error": "..."}` on failure.
pub fn dispatch(name: &str, input: &serde_json::Value) -> Option<String> {
    match name {
        "gh_my_prs" => Some(my_prs(input)),
        "gh_pr_view" => Some(pr_view(input)),
        "gh_my_issues" => Some(my_issues(input)),
        "gh_issue_view" => Some(issue_view(input)),
        "gh_actions_status" => Some(actions_status(input)),
        "gh_notifications" => Some(notifications(input)),
        "gh_repo_view" => Some(repo_view(input)),
        _ => None,
    }
}

/// Shell out to `gh` and return its stdout as a String. On failure
/// returns `{"error": "..."}` so Claude sees a consistent shape across
/// all integration tools.
fn run_gh(args: &[&str]) -> String {
    let t = std::time::Instant::now();
    eprintln!("[gh] gh {}", args.join(" "));
    let out = match Command::new("gh").args(args).output() {
        Ok(o) if o.status.success() => {
            let out = String::from_utf8_lossy(&o.stdout);
            let trimmed = out.trim();
            if trimmed.is_empty() {
                "{}".to_string()
            } else {
                trimmed.to_string()
            }
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            eprintln!("[gh] non-zero exit: {}", err.trim());
            format!(
                r#"{{"error":{}}}"#,
                serde_json::Value::String(err.trim().to_string())
            )
        }
        Err(e) => {
            eprintln!("[gh] spawn failed: {e}");
            format!(
                r#"{{"error":{}}}"#,
                serde_json::Value::String(e.to_string())
            )
        }
    };
    eprintln!("[gh] done in {:?} ({} chars out)", t.elapsed(), out.len());
    out
}

fn require_str<'a>(input: &'a serde_json::Value, field: &str) -> Result<&'a str, String> {
    input[field]
        .as_str()
        .ok_or_else(|| format!(r#"{{"error":"missing required string field '{field}'"}}"#))
}

fn require_int(input: &serde_json::Value, field: &str) -> Result<i64, String> {
    input[field]
        .as_i64()
        .ok_or_else(|| format!(r#"{{"error":"missing required integer field '{field}'"}}"#))
}

fn my_prs(input: &serde_json::Value) -> String {
    let state = input["state"].as_str().unwrap_or("open");
    let limit = input["limit"]
        .as_u64()
        .unwrap_or(5)
        .clamp(1, 25)
        .to_string();
    run_gh(&[
        "search",
        "prs",
        "--author",
        "@me",
        "--state",
        state,
        "--limit",
        &limit,
        "--json",
        "number,title,repository,state,url,createdAt",
    ])
}

fn pr_view(input: &serde_json::Value) -> String {
    let repo = match require_str(input, "repo") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let number = match require_int(input, "number") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let number_s = number.to_string();
    run_gh(&[
        "pr",
        "view",
        &number_s,
        "--repo",
        repo,
        "--json",
        "number,title,body,state,isDraft,author,headRefName,baseRefName,url,additions,deletions,reviewDecision,statusCheckRollup,createdAt",
    ])
}

fn my_issues(input: &serde_json::Value) -> String {
    let state = input["state"].as_str().unwrap_or("open");
    let limit = input["limit"]
        .as_u64()
        .unwrap_or(5)
        .clamp(1, 25)
        .to_string();
    run_gh(&[
        "search",
        "issues",
        "--author",
        "@me",
        "--state",
        state,
        "--limit",
        &limit,
        "--json",
        "number,title,repository,state,url,createdAt",
    ])
}

fn issue_view(input: &serde_json::Value) -> String {
    let repo = match require_str(input, "repo") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let number = match require_int(input, "number") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let number_s = number.to_string();
    run_gh(&[
        "issue",
        "view",
        &number_s,
        "--repo",
        repo,
        "--json",
        "number,title,body,state,labels,author,url,comments,createdAt",
    ])
}

fn actions_status(input: &serde_json::Value) -> String {
    let repo = match require_str(input, "repo") {
        Ok(v) => v,
        Err(e) => return e,
    };
    run_gh(&[
        "run",
        "list",
        "--repo",
        repo,
        "--limit",
        "5",
        "--json",
        "status,conclusion,name,event,headBranch,createdAt,url,displayTitle",
    ])
}

fn notifications(input: &serde_json::Value) -> String {
    let limit = input["limit"].as_u64().unwrap_or(5).clamp(1, 25);
    let endpoint = format!("notifications?per_page={limit}");
    run_gh(&[
        "api",
        &endpoint,
        "--jq",
        "[.[] | {subject: .subject.title, type: .subject.type, repo: .repository.full_name, reason, updated_at}]",
    ])
}

fn repo_view(input: &serde_json::Value) -> String {
    let repo = match require_str(input, "repo") {
        Ok(v) => v,
        Err(e) => return e,
    };
    run_gh(&[
        "repo",
        "view",
        repo,
        "--json",
        "name,owner,description,stargazerCount,defaultBranchRef,visibility,url,updatedAt",
    ])
}
