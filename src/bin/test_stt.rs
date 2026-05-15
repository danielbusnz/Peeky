// Isolated test for the mic → Deepgram pipeline with full timing.
//
// Run with `cargo run --bin test_stt`.
//
// What gets measured per turn:
//   * Press → release hold duration
//   * Press → first audio chunk forwarded (catches pre-roll flush)
//   * Press → first Deepgram interim
//   * Release → STT final transcript returned
//   * Total chunks forwarded + total samples + audio duration
//   * Every Deepgram event (interim / final) with elapsed time
//
// Use this to diagnose:
//   * Truncated transcripts (compare audio sent vs hold duration)
//   * Slow startup (first chunk delay should be ~0ms with pre-roll)
//   * Deepgram lag (first interim should arrive 100-300ms after first chunk)

#[path = "../audio.rs"]
mod audio;
#[path = "../hotkey/mod.rs"]
mod hotkey;
#[path = "../screenshot/mod.rs"]
mod screenshot;
#[path = "../providers/mod.rs"]
mod providers;

use providers::stt_deepgram::SttDeepgram;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

fn main() {
    let http = reqwest::Client::new();
    let stt = SttDeepgram::from_env(http).expect("STT init failed");
    let mic = audio::Mic::init();
    hotkey::init().expect("signal handler setup");

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let running_mic = mic.start();

    println!();
    println!("=================================");
    println!("STT pipeline test");
    println!("=================================");
    println!("config: {}Hz, {}ch", running_mic.sample_rate, running_mic.channels);
    println!();
    println!("hold SUPER+space, talk, release. Ctrl+C to quit.");
    println!();

    let mut turn = 1;
    loop {
        hotkey::wait_for_press();
        let press_t = Instant::now();

        eprintln!();
        eprintln!("════════════════════════ turn {} ════════════════════════", turn);
        log_t(&press_t, "press detected");

        // Two-channel setup so we can intercept chunks between cpal and Deepgram.
        //   cpal callback → interceptor_tx → interceptor_rx → counter → deepgram_tx → deepgram_rx
        let (interceptor_tx, mut interceptor_rx) =
            tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();
        let (deepgram_tx, deepgram_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();

        let chunk_count = Arc::new(AtomicUsize::new(0));
        let total_samples = Arc::new(AtomicUsize::new(0));
        let first_chunk_at = Arc::new(std::sync::Mutex::new(None::<std::time::Duration>));

        // Interceptor task: counts and timestamps each chunk, then forwards.
        let interceptor_handle = {
            let chunk_count = chunk_count.clone();
            let total_samples = total_samples.clone();
            let first_chunk_at = first_chunk_at.clone();
            let press_t = press_t;
            rt.spawn(async move {
                while let Some(chunk) = interceptor_rx.recv().await {
                    let n = chunk.len();
                    let count = chunk_count.fetch_add(1, Ordering::Relaxed) + 1;
                    total_samples.fetch_add(n, Ordering::Relaxed);
                    {
                        let mut slot = first_chunk_at.lock().unwrap();
                        if slot.is_none() {
                            let elapsed = press_t.elapsed();
                            *slot = Some(elapsed);
                            eprintln!(
                                "[t={:>8.1?}] first chunk forwarded (#{}: {} samples)",
                                elapsed, count, n
                            );
                        }
                    }
                    if deepgram_tx.send(chunk).is_err() {
                        break;
                    }
                }
                // interceptor_rx exhausted → deepgram_tx drops → Deepgram sees EOS
            })
        };

        // Deepgram WebSocket task
        let stt_handle = {
            let stt = stt.clone();
            let sample_rate = running_mic.sample_rate;
            let channels = running_mic.channels;
            rt.spawn(async move {
                stt.transcribe_stream(sample_rate, channels, deepgram_rx, None)
                    .await
            })
        };

        // Install our interceptor_tx as the cpal forwarding target.
        // This blocks until the hotkey is released.
        running_mic.capture_until_release(interceptor_tx);
        let release_t = Instant::now();
        log_t(&press_t, "release detected");

        // Drain interceptor (it'll exit once interceptor_rx is exhausted)
        let _ = rt.block_on(interceptor_handle);

        // Wait for Deepgram's final transcript (Strategy B returns ~instantly)
        let transcript = rt
            .block_on(stt_handle)
            .expect("stt task panicked")
            .expect("stt error");
        let stt_done_t = Instant::now();
        log_t(&press_t, "STT transcript returned");

        // Per-turn summary
        let chunks = chunk_count.load(Ordering::Relaxed);
        let samples = total_samples.load(Ordering::Relaxed);
        let audio_ms = (samples as f64
            / (running_mic.sample_rate as f64 * running_mic.channels as f64))
            * 1000.0;

        eprintln!();
        eprintln!("─── summary ───");
        eprintln!("  transcript            : {:?}", transcript);
        eprintln!("  hold duration         : {:?}", release_t.duration_since(press_t));
        eprintln!("  STT return after rel  : {:?}", stt_done_t.duration_since(release_t));
        if let Some(first) = *first_chunk_at.lock().unwrap() {
            eprintln!("  first chunk after pre : {:?}", first);
        }
        eprintln!("  chunks forwarded      : {}", chunks);
        eprintln!("  samples forwarded     : {} ({:.1}ms of audio)", samples, audio_ms);
        eprintln!();

        turn += 1;
    }
}

fn log_t(press_t: &Instant, msg: &str) {
    eprintln!("[t={:>8.1?}] {}", press_t.elapsed(), msg);
}
