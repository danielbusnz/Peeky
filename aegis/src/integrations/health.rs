//! Startup health checks for every integration. Probes each one in
//! parallel (so total wait time = the slowest check, not their sum) and
//! prints a single table to stderr. Pure diagnostics, never panics, never
//! prevents the app from booting.

use std::process::{Command, Stdio};
use std::thread;
use std::time::Instant;

use super::{github, gmail, spotify, youtube};

/// Per-integration result. `Ok` carries a short status string ("playing on
/// archbox", "12 unread"). `Err` carries the reason for failure. `Skip` is
/// "not configured / not installed", which is distinct from broken.
enum Status {
    Ok(String),
    Err(String),
    Skip(String),
}

struct Report {
    name: &'static str,
    elapsed_ms: u128,
    status: Status,
}

/// Run every integration's health probe in parallel and print a table.
/// Call once at startup; takes ~the slowest probe's wall time.
pub fn check_and_print() {
    type ProbeFn = fn() -> Status;
    let probes: Vec<(&'static str, ProbeFn)> = vec![
        ("github", probe_github),
        ("gmail", probe_gmail),
        ("spotify", probe_spotify),
        ("youtube", probe_youtube),
    ];

    let t0 = Instant::now();
    let handles: Vec<_> = probes
        .into_iter()
        .map(|(name, probe)| {
            thread::spawn(move || {
                let t = Instant::now();
                let status = probe();
                Report {
                    name,
                    elapsed_ms: t.elapsed().as_millis(),
                    status,
                }
            })
        })
        .collect();

    let mut reports: Vec<Report> = handles
        .into_iter()
        .map(|h| h.join().expect("health probe thread panicked"))
        .collect();
    reports.sort_by_key(|r| r.name);

    eprintln!(
        "[health] integration checks ({}ms total):",
        t0.elapsed().as_millis()
    );
    for r in &reports {
        let (tag, detail) = match &r.status {
            Status::Ok(d) => ("OK  ", d.as_str()),
            Status::Err(d) => ("FAIL", d.as_str()),
            Status::Skip(d) => ("SKIP", d.as_str()),
        };
        eprintln!(
            "[health]   {:<7}  {}  ({}ms)  {}",
            r.name, tag, r.elapsed_ms, detail
        );
    }
}

// ── github ────────────────────────────────────────────────────────────────
fn probe_github() -> Status {
    if !github::is_available() {
        return Status::Skip("gh not installed or not logged in".to_string());
    }
    // gh api user --jq .login → just the username, single string, fast.
    match Command::new("gh")
        .args(["api", "user", "--jq", ".login"])
        .stderr(Stdio::piped())
        .output()
    {
        Ok(o) if o.status.success() => {
            let login = String::from_utf8_lossy(&o.stdout).trim().to_string();
            Status::Ok(format!("authenticated as {login}"))
        }
        Ok(o) => Status::Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => Status::Err(format!("spawn failed: {e}")),
    }
}

// ── gmail ─────────────────────────────────────────────────────────────────
fn probe_gmail() -> Status {
    if !gmail::is_available() {
        return Status::Skip("AEGIS_GMAIL_CLIENT_ID/SECRET not set".to_string());
    }
    // user_email() hits /profile, which exercises OAuth refresh, the token
    // cache file, and an HTTP round trip. Success means "auth works AND we
    // know who the user is". That result is also cached for the agent
    // loop to inject into Claude's system prompt.
    match gmail::user_email() {
        Some(email) => Status::Ok(format!("authenticated as {email}")),
        None => Status::Err("profile fetch failed (see [gmail] log lines)".to_string()),
    }
}

// ── spotify ───────────────────────────────────────────────────────────────
fn probe_spotify() -> Status {
    if !spotify::is_available() {
        return Status::Skip("spotify_player not installed".to_string());
    }
    // `get key playback` returns a JSON blob describing the active device.
    // If auth is broken it fails with the same auth error the dispatch path
    // would hit, so this is a real end-to-end probe.
    match Command::new("spotify_player")
        .args(["get", "key", "playback"])
        .stderr(Stdio::piped())
        .output()
    {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            // The blob is huge; pull just the device name + playing flag for
            // a one-line summary. Soft-fail to "responding" if the shape
            // isn't what we expect rather than blowing up the probe.
            let summary = serde_json::from_str::<serde_json::Value>(&stdout)
                .ok()
                .and_then(|v| {
                    let device = v
                        .get("device")
                        .and_then(|d| d.get("name"))
                        .and_then(|n| n.as_str())
                        .map(str::to_string);
                    let playing = v.get("is_playing").and_then(|p| p.as_bool());
                    match (device, playing) {
                        (Some(d), Some(true)) => Some(format!("playing on {d}")),
                        (Some(d), Some(false)) => Some(format!("paused on {d}")),
                        (Some(d), None) => Some(format!("connected to {d}")),
                        _ => None,
                    }
                })
                .unwrap_or_else(|| "responding".to_string());
            Status::Ok(summary)
        }
        Ok(o) => Status::Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => Status::Err(format!("spawn failed: {e}")),
    }
}

// ── youtube ───────────────────────────────────────────────────────────────
fn probe_youtube() -> Status {
    if !youtube::is_available() {
        return Status::Skip("yt-dlp not installed".to_string());
    }
    match Command::new("yt-dlp")
        .arg("--version")
        .stderr(Stdio::piped())
        .output()
    {
        Ok(o) if o.status.success() => {
            let v = String::from_utf8_lossy(&o.stdout).trim().to_string();
            Status::Ok(format!("yt-dlp {v}"))
        }
        Ok(o) => Status::Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => Status::Err(format!("spawn failed: {e}")),
    }
}
