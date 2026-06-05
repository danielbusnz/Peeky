//! Trial-wall upgrade prompt. When a proxy call comes back trial-exhausted, we
//! nudge the user to sign in: speak one line and open the console's sign-in
//! window (`AEGIS_SHOW_SIGNIN=1`), the same branded card the user can reach from
//! settings. That window runs the GitHub OAuth dance and writes the session
//! token to the file the agent reads per request (see
//! [`crate::providers::session_jwt`]), so the next voice turn is already on the
//! account tier. If the console binary can't be found (e.g. the agent was run
//! standalone in dev), we fall back to driving the browser OAuth ourselves so
//! sign-in is never a dead end.
//!
//! Fires once per process: a run of exhausted turns must not stack windows or
//! repeat the spoken line. Detection lives at the provider HTTP error boundary
//! ([`on_proxy_error`]); the orchestrator drains the spoken line via
//! [`take_announcement`] and routes it through the normal TTS pipeline.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The proxy's trial-exhausted 429 carries this error code in its JSON body.
/// Mirrors `trial_exhausted` in proxy/src/usage.ts.
const TRIAL_EXHAUSTED_MARKER: &str = "trial_exhausted";

/// Spoken once when the wall is first hit. Kept neutral about window vs browser
/// since either may front the sign-in depending on what's installed.
const UPGRADE_LINE: &str =
    "That's your free turns for today. I've opened sign-in, just sign in to keep going.";

/// True once the prompt has fired this process, so repeated exhausted turns
/// stay quiet and don't reopen the browser.
static PROMPTED: AtomicBool = AtomicBool::new(false);

/// Set when [`on_proxy_error`] fires and cleared by [`take_announcement`]. Lets
/// the deep call site hand the spoken line back to the orchestrator, which owns
/// TTS, without threading a channel through every provider.
static ANNOUNCEMENT_PENDING: AtomicBool = AtomicBool::new(false);

fn proxy_base() -> String {
    std::env::var("AEGIS_PROXY_BASE")
        .unwrap_or_else(|_| "https://aegis-proxy.danielbusnz.workers.dev".to_string())
}

/// Inspect a proxy response that already failed its status check. If it's the
/// trial-exhausted 429, fire the sign-in flow once: open the browser and arm
/// the spoken line. Safe to call from any provider error branch; it's a no-op
/// for every other error and after the first fire. Must run inside the tokio
/// runtime (it spawns the OAuth poll task).
pub fn on_proxy_error(status: u16, body: &str) {
    if status != 429 || !body.contains(TRIAL_EXHAUSTED_MARKER) {
        return;
    }
    // swap returns the prior value: only the first caller proceeds.
    if PROMPTED.swap(true, Ordering::SeqCst) {
        return;
    }
    ANNOUNCEMENT_PENDING.store(true, Ordering::SeqCst);
    tokio::spawn(launch_signin());
}

/// Open the console's sign-in window; if no console binary is found, drive the
/// browser OAuth ourselves. Spawned as a task so the turn can end.
async fn launch_signin() {
    if open_console_signin() {
        return;
    }
    eprintln!("[upgrade] no console binary found; opening browser sign-in directly");
    run_oauth_flow().await;
}

/// Spawn the console's sign-in window via `AEGIS_SHOW_SIGNIN=1`. The console
/// runs the OAuth dance and writes the session token file we read. Returns true
/// once a console binary is found and spawned.
fn open_console_signin() -> bool {
    for path in console_candidates() {
        if !path.exists() {
            continue;
        }
        let mut cmd = std::process::Command::new(&path);
        cmd.env("AEGIS_SHOW_SIGNIN", "1");
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        match cmd.spawn() {
            Ok(_) => {
                eprintln!(
                    "[upgrade] opened console sign-in window: {}",
                    path.display()
                );
                return true;
            }
            Err(e) => eprintln!("[upgrade] couldn't spawn console {}: {e}", path.display()),
        }
    }
    false
}

/// Where the console binary might live. The env var is set by the console when
/// it spawns the agent (the reliable production path); the rest cover shipped
/// bundles (sibling of the agent exe) and the dev workspace layout.
fn console_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(p) = std::env::var("AEGIS_CONSOLE_BIN") {
        candidates.push(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join("console"));
        candidates.push(dir.join("Aegis"));
        #[cfg(windows)]
        {
            candidates.push(dir.join("console.exe"));
            candidates.push(dir.join("Aegis.exe"));
        }
    }
    for p in [
        "../../target/debug/console",
        "../../target/release/console",
        "target/debug/console",
        "target/release/console",
    ] {
        candidates.push(PathBuf::from(p));
    }
    candidates
}

/// Take the pending upgrade line, if the wall was just hit. Returns it at most
/// once per fire; the orchestrator speaks it through the live TTS pipeline.
pub fn take_announcement() -> Option<&'static str> {
    if ANNOUNCEMENT_PENDING.swap(false, Ordering::SeqCst) {
        Some(UPGRADE_LINE)
    } else {
        None
    }
}

/// Open the browser to the proxy's GitHub OAuth start, then poll the session
/// endpoint until the proxy parks our JWT (same dance as the console). On
/// success, write the token to the file the agent reads, so the next turn is
/// the account tier. Best-effort: every failure just leaves the user on trial.
async fn run_oauth_flow() {
    let base = proxy_base();
    let state = uuid::Uuid::new_v4().to_string();

    if let Err(e) = open::that(format!("{base}/auth/github/start?state={state}")) {
        eprintln!("[upgrade] couldn't open browser for sign-in: {e}");
        return;
    }

    let client = reqwest::Client::new();
    let session_url = format!("{base}/auth/github/session?state={state}");

    // Poll for ~2 minutes while the user completes the browser flow.
    for _ in 0..80 {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let resp = match client.get(&session_url).send().await {
            Ok(r) => r,
            Err(_) => continue, // transient network blip; keep polling
        };
        if resp.status().as_u16() == 404 {
            eprintln!("[upgrade] sign-in link expired before completion");
            return;
        }
        let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        if body.get("status").and_then(|v| v.as_str()) != Some("done") {
            continue;
        }
        let token = body
            .get("token")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if token.is_empty() {
            eprintln!("[upgrade] sign-in returned an empty token");
            return;
        }
        match crate::providers::session_jwt::store(token) {
            Ok(()) => eprintln!("[upgrade] signed in; account tier active next turn"),
            Err(e) => eprintln!("[upgrade] couldn't store session token: {e}"),
        }
        return;
    }
    eprintln!("[upgrade] sign-in timed out");
}
