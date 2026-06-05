//! Stage 1 distillation data collection: turn each on-device classification
//! into a redacted sample, written to a capped local log and/or POSTed to the
//! proxy by a background uploader. Both paths are opt-in and off by default.

use std::io::Write;
use std::path::Path;
use std::sync::OnceLock;
use std::time::SystemTime;

use tokio::sync::mpsc::UnboundedSender;

use super::redact::{redact, redact_intent};
use crate::providers::claude::Intent;

/// Retention cap for the opt-in log: keep at most this many of the most recent
/// lines on disk so redacted history can't grow without bound.
const LOG_MAX_LINES: usize = 5000;

/// Append a single line to `path` (creating it if needed), then trim the file
/// to its most recent `max_lines` lines. Returns an IO error so the caller can
/// log and swallow it. The log is low-volume and capped, so reading it back per
/// write is cheap, and the file is only rewritten once it grows past the cap.
fn append_record(path: &Path, line: &str, max_lines: usize) -> std::io::Result<()> {
    {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(file, "{line}")?;
    }

    let contents = std::fs::read_to_string(path)?;
    let lines: Vec<&str> = contents.lines().collect();
    if lines.len() > max_lines {
        let tail = lines[lines.len() - max_lines..].join("\n");
        std::fs::write(path, format!("{tail}\n"))?;
    }
    Ok(())
}

/// Record one redacted classification sample for Stage 1 distillation. The
/// sample carries the routelet prediction and its confidence plus, when the
/// Claude fallback fired this turn, Claude's label as the free teacher signal.
///
/// Two independent opt-ins, both off by default:
///   * `AEGIS_ROUTELET_LOG=1` appends the sample to the on-device JSONL log.
///   * `AEGIS_ROUTELET_UPLOAD=1` enqueues it for the background proxy uploader
///     (only effective once `init_uploader` has run).
///
/// Does nothing unless at least one is set. On any error, emits a warning to
/// stderr and returns without panicking. Safe to call in the hot turn path:
/// disk writes are capped and the upload enqueue is a non-blocking send.
pub fn log_classification(
    text: &str,
    routelet_pred: Option<Intent>,
    routelet_conf: Option<f32>,
    claude_label: Option<Intent>,
) {
    if !logging_enabled() && !upload_enabled() {
        return;
    }

    let redacted = redact(text, redact_intent(routelet_pred, claude_label));
    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Field names mirror the proxy's RouteletSample (handlers/routelet.ts) so a
    // local log line and a pulled R2 object are the same shape downstream.
    let sample = serde_json::json!({
        "text": redacted,
        "routelet_pred": routelet_pred.map(Intent::as_str),
        "routelet_conf": routelet_conf,
        "claude_label": claude_label.map(Intent::as_str),
        "ts": ts,
    });

    if logging_enabled() {
        write_local(&sample);
    }
    if let Some(tx) = UPLOAD_TX.get() {
        // Non-blocking. A closed channel (uploader gone) just means we keep the
        // local copy, so the error is intentionally dropped.
        let _ = tx.send(sample);
    }
}

/// Append one serialized sample to the on-device log, resolving the path and
/// swallowing IO errors with a warning. Split out so the hot path stays a
/// straight line of intent.
fn write_local(sample: &serde_json::Value) {
    let line = match serde_json::to_string(sample) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[routelet-log] serialization error: {e}");
            return;
        }
    };
    let log_path = match build_log_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[routelet-log] could not resolve log path: {e}");
            return;
        }
    };
    if let Err(e) = append_record(&log_path, &line, LOG_MAX_LINES) {
        eprintln!("[routelet-log] write error ({}): {e}", log_path.display());
    }
}

/// Resolve `~/.config/aegis/routelet_log.jsonl`, creating the directory if
/// needed. Mirrors the resolution used by `MemoryStore::open_default`.
fn build_log_path() -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let mut path = dirs::config_dir().ok_or("could not locate config dir")?;
    path.push("aegis");
    std::fs::create_dir_all(&path)?;
    path.push("routelet_log.jsonl");
    Ok(path)
}

// ────── distillation sample uploader ──────

/// Proxy endpoint that stores one redacted sample per POST. Override with
/// `AEGIS_ROUTELET_SAMPLE_URL` to point at a local `wrangler dev` instance.
const SAMPLE_URL: &str = "https://aegis-proxy.danielbusnz.workers.dev/v1/routelet/sample";

/// Set once by `init_uploader` when uploading is enabled. `log_classification`
/// sends samples here; the background task drains and POSTs them.
static UPLOAD_TX: OnceLock<UnboundedSender<serde_json::Value>> = OnceLock::new();

fn logging_enabled() -> bool {
    std::env::var("AEGIS_ROUTELET_LOG").as_deref() == Ok("1")
}

fn upload_enabled() -> bool {
    std::env::var("AEGIS_ROUTELET_UPLOAD").as_deref() == Ok("1")
}

fn sample_url() -> String {
    std::env::var("AEGIS_ROUTELET_SAMPLE_URL").unwrap_or_else(|_| SAMPLE_URL.to_string())
}

/// Spawn the background sample uploader on the session runtime. Samples that
/// `log_classification` enqueues are drained in batches and POSTed to the proxy
/// off the hot turn path, so the voice loop never blocks on the network.
///
/// No-op unless `AEGIS_ROUTELET_UPLOAD=1`. Idempotent: a second call is ignored.
/// Reuses the shared `reqwest::Client` so uploads ride the warm connection pool.
pub fn init_uploader(rt: &tokio::runtime::Runtime, http: reqwest::Client) {
    if !upload_enabled() {
        return;
    }
    let device_id = match crate::providers::device_id::load_or_create() {
        Ok(id) => id,
        Err(e) => {
            eprintln!("[routelet-upload] no device id, uploads disabled: {e}");
            return;
        }
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
    if UPLOAD_TX.set(tx).is_err() {
        return; // already initialized
    }
    let url = sample_url();
    rt.spawn(async move {
        while let Some(first) = rx.recv().await {
            // Drain whatever else is already queued so a burst of turns flushes
            // in one wakeup instead of one POST per await.
            let mut batch = vec![first];
            while batch.len() < crate::tuning::ROUTELET_UPLOAD_BATCH_MAX {
                match rx.try_recv() {
                    Ok(s) => batch.push(s),
                    Err(_) => break,
                }
            }
            // The endpoint stores one sample per request, so the batch is sent
            // as sequential POSTs. They are off the hot path; latency here only
            // delays telemetry, never a voice turn.
            for sample in batch {
                let res = http
                    .post(&url)
                    .header(
                        crate::providers::proxy_contract::DEVICE_ID_HEADER,
                        &device_id,
                    )
                    .json(&sample)
                    .send()
                    .await;
                if let Err(e) = res {
                    eprintln!("[routelet-upload] POST failed: {e}");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;

    // append_record: creates file, writes a valid JSON line, second call appends
    #[test]
    fn append_record_creates_and_appends() {
        // Use a unique name in tmp so parallel test runs don't collide.
        let path = std::env::temp_dir().join(format!(
            "aegis_routelet_test_{}.jsonl",
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        // Remove any leftover from a previous run.
        let _ = std::fs::remove_file(&path);

        let line1 = r#"{"a":1}"#;
        let line2 = r#"{"b":2}"#;

        append_record(&path, line1, 10).expect("first write should succeed");
        append_record(&path, line2, 10).expect("second write should succeed");

        let file = std::fs::File::open(&path).unwrap();
        let lines: Vec<String> = std::io::BufReader::new(file)
            .lines()
            .map(|l| l.unwrap())
            .collect();

        // Clean up before assertions so a failure still removes the file.
        let _ = std::fs::remove_file(&path);

        assert_eq!(lines.len(), 2, "expected 2 lines");
        assert_eq!(lines[0], line1);
        assert_eq!(lines[1], line2);

        // Both lines must parse as valid JSON.
        serde_json::from_str::<serde_json::Value>(&lines[0]).expect("line 1 must be valid JSON");
        serde_json::from_str::<serde_json::Value>(&lines[1]).expect("line 2 must be valid JSON");
    }

    // append_record: trims the file to the most recent max_lines.
    #[test]
    fn append_record_caps_to_max_lines() {
        let path = std::env::temp_dir().join(format!(
            "aegis_routelet_cap_{}.jsonl",
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_file(&path);

        // Write 10 lines with a cap of 3; only the last 3 should remain.
        for i in 0..10 {
            append_record(&path, &format!(r#"{{"n":{i}}}"#), 3).expect("write should succeed");
        }
        let contents = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 3, "file should be capped to 3 lines");
        assert_eq!(lines, vec![r#"{"n":7}"#, r#"{"n":8}"#, r#"{"n":9}"#]);
    }

    // Intent::as_str round-trips with from_str for all five variants.
    // (Mirrors the test in classifier.rs but lives here with the log code,
    // which depends on as_str to serialize the sample.)
    #[test]
    fn as_str_round_trip_all_variants() {
        for intent in [
            Intent::FindAction,
            Intent::Integration,
            Intent::Chat,
            Intent::Memory,
            Intent::Agent,
        ] {
            assert_eq!(
                Intent::from_str(intent.as_str()),
                Some(intent),
                "round-trip failed for {:?}",
                intent
            );
        }
    }
}
