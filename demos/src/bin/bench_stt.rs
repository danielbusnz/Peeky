// Test bins import the whole providers/audio/screenshot module tree but only
// touch a slice. Silences false-positive dead-code lints from the rest.
#![allow(dead_code)]

//! Deepgram STT benchmark over a fixed audio file.
//!
//! Loops the same WAV through the real `transcribe_stream` path N times,
//! reports mean/median/min/max of the post-stream-end latency (the "tail"
//! that production users see between releasing the hotkey and the
//! transcript landing), plus how many runs matched the expected text.
//!
//! Real-time playback: chunks are pushed at the same wall-clock rate the
//! mic would feed them. This matches Deepgram's live-streaming behavior;
//! bursting all the audio at once would give artificially low numbers.
//!
//! Usage:
//!   cargo run --release --example test_stt_bench -- <wav_path> <expected_text> [iterations]
//!
//! Example:
//!   cargo run --release --example test_stt_bench -- peeky/fixtures/sample.wav "hi my name is daniel" 5
//!
//! Recording a sample (24kHz mono, 16-bit signed little-endian):
//!   pw-record --rate 24000 --channels 1 --format s16 peeky/fixtures/sample.wav

use peeky::providers;

use providers::stt_deepgram::SttDeepgram;
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "usage: {} <wav_path> <expected_text> [iterations=5]",
            args[0]
        );
        std::process::exit(1);
    }
    let wav_path = &args[1];
    let expected = &args[2];
    let iterations: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(5);

    let (samples, sample_rate, channels) = load_wav(wav_path);
    let audio_duration_ms =
        (samples.len() as f64 / (sample_rate as f64 * channels as f64)) * 1000.0;
    println!(
        "loaded {wav_path}: {} samples, {}Hz, {}ch ({:.0}ms of audio)",
        samples.len(),
        sample_rate,
        channels,
        audio_duration_ms
    );
    println!("expected: {:?}", expected);
    println!("iterations: {iterations}");
    println!();

    let http = reqwest::Client::new();
    let stt = SttDeepgram::from_env(http).expect("STT init failed");
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");

    let mut tails: Vec<Duration> = Vec::with_capacity(iterations);
    let mut matches: usize = 0;

    for i in 0..iterations {
        print!("turn {}/{}: ", i + 1, iterations);
        let result = run_once(&rt, &stt, &samples, sample_rate, channels);
        let matched = normalize(&result.transcript) == normalize(expected);
        if matched {
            matches += 1;
        }
        println!(
            "tail={:>6.1?}  match={}  transcript={:?}",
            result.tail,
            if matched { "✓" } else { "✗" },
            result.transcript
        );
        tails.push(result.tail);
    }

    println!();
    let stats = compute_stats(&tails);
    println!("─── summary ({} iterations) ───", iterations);
    println!("  matches    : {}/{}", matches, iterations);
    println!("  tail mean  : {:?}", stats.mean);
    println!("  tail median: {:?}", stats.median);
    println!("  tail min   : {:?}", stats.min);
    println!("  tail max   : {:?}", stats.max);
}

struct RunResult {
    transcript: String,
    /// Time from end-of-stream to final transcript returned. Same metric
    /// the live tool calls "STT return after rel".
    tail: Duration,
}

fn run_once(
    rt: &tokio::runtime::Runtime,
    stt: &SttDeepgram,
    samples: &[i16],
    sample_rate: u32,
    channels: u16,
) -> RunResult {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();

    // 50ms chunks at 50ms intervals. Realistic mic feed.
    let samples_per_chunk = (sample_rate as usize / 20) * channels as usize;
    let chunks: Vec<Vec<i16>> = samples
        .chunks(samples_per_chunk)
        .map(|c| c.to_vec())
        .collect();

    let stream_end_marker = std::sync::Arc::new(std::sync::Mutex::new(None::<Instant>));
    let marker_for_replay = stream_end_marker.clone();
    let replay_task = rt.spawn(async move {
        for chunk in chunks {
            if tx.send(chunk).is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // tx dropping is what closes the channel → Deepgram sees EOS.
        // Mark the moment we stop sending so we can measure the tail.
        *marker_for_replay.lock().unwrap() = Some(Instant::now());
        drop(tx);
    });

    let stt = stt.clone();
    let transcribe_task =
        rt.spawn(async move { stt.transcribe_stream(sample_rate, channels, rx, None).await });

    rt.block_on(replay_task).expect("replay task panicked");
    let transcript = rt
        .block_on(transcribe_task)
        .expect("transcribe task panicked")
        .expect("transcribe returned an error");

    let stream_end_t = stream_end_marker
        .lock()
        .unwrap()
        .expect("replay task should have set the marker");
    let tail = stream_end_t.elapsed();

    RunResult { transcript, tail }
}

fn load_wav(path: &str) -> (Vec<i16>, u32, u16) {
    let mut reader =
        hound::WavReader::open(path).unwrap_or_else(|e| panic!("could not open {path}: {e}"));
    let spec = reader.spec();
    assert_eq!(
        spec.sample_format,
        hound::SampleFormat::Int,
        "expected integer PCM WAV (got float)"
    );
    assert_eq!(spec.bits_per_sample, 16, "expected 16-bit PCM");
    let samples: Vec<i16> = reader
        .samples::<i16>()
        .collect::<Result<Vec<_>, _>>()
        .expect("WAV decode failed");
    (samples, spec.sample_rate, spec.channels)
}

fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

struct Stats {
    mean: Duration,
    median: Duration,
    min: Duration,
    max: Duration,
}

fn compute_stats(times: &[Duration]) -> Stats {
    let mut sorted: Vec<u128> = times.iter().map(|d| d.as_micros()).collect();
    sorted.sort_unstable();
    let n = sorted.len();
    let mean_us = sorted.iter().sum::<u128>() / n as u128;
    let median_us = if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2
    };
    Stats {
        mean: Duration::from_micros(mean_us as u64),
        median: Duration::from_micros(median_us as u64),
        min: Duration::from_micros(sorted[0] as u64),
        max: Duration::from_micros(sorted[n - 1] as u64),
    }
}
