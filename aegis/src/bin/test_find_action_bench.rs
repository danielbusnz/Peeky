// Full-pipeline benchmark: WAV → STT → classifier → find_action.
// Captures the current screen once, replays a pre-recorded WAV through
// Deepgram N times, classifies the transcript, and runs find_action.
// Reports per-iteration timing for every stage so you can see exactly
// where the latency budget goes.
//
// Usage:
//   cargo run --release --bin test_find_action_bench -- ./sample.wav 5
//
// Record a sample if you don't have one yet (24kHz mono PCM 16-bit):
//   arecord -f S16_LE -c 1 -r 24000 -d 4 sample.wav
//   # or with pipewire:
//   pw-record --rate 24000 --channels 1 --format s16 sample.wav
//
// Reported stages per iteration:
//   stt:         release → final transcript (Deepgram tail)
//   classify:    transcript → Intent (one Claude call, forced tool)
//   find_action: transcript + screenshot → action (one Claude call, forced tool)
//   total:       sum of the three (what the user would feel)
//
// Iterations where the classifier doesn't return FindAction are noted
// but excluded from the find_action / total stats.

#![allow(dead_code)]

#[path = "../screenshot/mod.rs"]
mod screenshot;
#[path = "../tuning.rs"]
mod tuning;
#[path = "../providers/mod.rs"]
mod providers;
#[path = "../intent.rs"]
mod intent;

use providers::claude::{Claude, Intent};
use providers::stt_deepgram::SttDeepgram;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: {} <wav_path> [iterations=5]", args[0]);
        std::process::exit(1);
    }
    let wav_path = &args[1];
    let iterations: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);

    let http = reqwest::Client::new();
    let stt = SttDeepgram::from_env(http.clone()).expect("STT init failed");
    let claude = Claude::from_env(http).expect("Claude init failed");
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");

    // Warm both HTTP pools in parallel so iteration 1 doesn't pay TLS
    // handshake cost on top of everything else we're trying to measure.
    let t_warm = Instant::now();
    rt.block_on(async {
        let _ = tokio::join!(stt.warm(), claude.warm());
    });
    println!("[warm] http pools → {:?}", t_warm.elapsed());

    // Load the WAV once. Each iteration replays the same bytes through
    // a fresh Deepgram WS so latency is comparable across runs.
    let (samples, sample_rate, channels) = load_wav(wav_path);
    let audio_ms =
        (samples.len() as f64 / (sample_rate as f64 * channels as f64)) * 1000.0;
    println!(
        "[setup] WAV: {} samples, {}Hz, {}ch ({:.0}ms of audio)",
        samples.len(),
        sample_rate,
        channels,
        audio_ms
    );

    // Capture the screen once so every iteration sees identical pixels.
    // find_action variance from per-iteration pixel changes would muddy
    // the timing data.
    let t_shot = Instant::now();
    let (x, y, w, h) = screenshot::active_workspace_geometry()
        .expect("could not get workspace geometry");
    let (declared_w, declared_h) = screenshot::pick_declared_resolution(w as i64, h as i64);
    let image_b64 = screenshot::capture_resized_for_claude(
        x, y, w as i32, h as i32, declared_w, declared_h,
    )
    .expect("could not capture screenshot");
    println!(
        "[setup] screenshot: {}×{} → {}×{} ({} KB) in {:?}",
        w,
        h,
        declared_w,
        declared_h,
        image_b64.len() / 1024,
        t_shot.elapsed()
    );
    println!("[setup] iterations: {}", iterations);
    println!();

    let mut results: Vec<RunResult> = Vec::with_capacity(iterations);
    for i in 0..iterations {
        println!("─── turn {}/{} ───", i + 1, iterations);
        let r = run_once(&rt, &stt, &claude, &samples, sample_rate, channels, &image_b64, x, y, w, h);
        println!(
            "  transcript    : {:?}",
            r.transcript.as_deref().unwrap_or("(none)")
        );
        println!(
            "  intent        : {:?}",
            r.intent.map(|i| format!("{:?}", i)).unwrap_or_else(|| "(none)".to_string())
        );
        println!("  stt tail      : {}", fmt_opt(r.stt));
        println!("  classify      : {}", fmt_opt(r.classify));
        println!("  find_action   : {}", fmt_opt(r.find_action));
        println!("  total         : {}", fmt_opt(r.total));
        println!("  action fired  : {}", if r.fired { "✓" } else { "✗" });
        println!();
        results.push(r);
    }

    println!("─── summary ({} iterations) ───", iterations);
    let fa_eligible: Vec<&RunResult> = results
        .iter()
        .filter(|r| matches!(r.intent, Some(Intent::FindAction)))
        .collect();
    println!(
        "  intents picked: {:?}",
        results
            .iter()
            .filter_map(|r| r.intent.map(|i| format!("{:?}", i)))
            .collect::<Vec<_>>()
    );
    println!(
        "  find_action runs: {} (out of {})",
        fa_eligible.len(),
        iterations
    );
    print_stats("stt tail        ", results.iter().filter_map(|r| r.stt));
    print_stats("classify        ", results.iter().filter_map(|r| r.classify));
    print_stats(
        "find_action     ",
        fa_eligible.iter().filter_map(|r| r.find_action),
    );
    print_stats("total (full pipe)", fa_eligible.iter().filter_map(|r| r.total));
}

struct RunResult {
    transcript: Option<String>,
    intent: Option<Intent>,
    stt: Option<Duration>,
    classify: Option<Duration>,
    find_action: Option<Duration>,
    total: Option<Duration>,
    fired: bool,
}

#[allow(clippy::too_many_arguments)]
fn run_once(
    rt: &tokio::runtime::Runtime,
    stt: &SttDeepgram,
    claude: &Claude,
    samples: &[i16],
    sample_rate: u32,
    channels: u16,
    image_b64: &str,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
) -> RunResult {
    // ── stage 1: STT ──
    let stt_done_at = Arc::new(Mutex::new(None::<Instant>));
    let stt_done_at_clone = stt_done_at.clone();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();
    let samples_per_chunk = (sample_rate as usize / 20) * channels as usize;
    let chunks: Vec<Vec<i16>> = samples
        .chunks(samples_per_chunk)
        .map(|c| c.to_vec())
        .collect();

    let stt_start = Instant::now();
    let replay = rt.spawn(async move {
        for chunk in chunks {
            if tx.send(chunk).is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        *stt_done_at_clone.lock().unwrap() = Some(Instant::now());
        drop(tx);
    });
    let stt = stt.clone();
    let stt_task = rt.spawn(async move {
        stt.transcribe_stream(sample_rate, channels, rx, None).await
    });
    let _ = rt.block_on(replay);
    let transcript = match rt.block_on(stt_task) {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => {
            eprintln!("[stt] error: {e}");
            return empty_result();
        }
        Err(e) => {
            eprintln!("[stt] task panicked: {e}");
            return empty_result();
        }
    };
    let stt_end = Instant::now();
    let stt_tail = stt_done_at
        .lock()
        .ok()
        .and_then(|g| *g)
        .map(|done| stt_end.saturating_duration_since(done));

    if transcript.trim().is_empty() {
        return RunResult {
            transcript: Some(transcript),
            intent: None,
            stt: stt_tail,
            classify: None,
            find_action: None,
            total: None,
            fired: false,
        };
    }

    // ── stage 2: classifier (hybrid — same path as orchestrator) ──
    let t_classify = Instant::now();
    let (intent, classifier_path) = match intent::keyword_classify(&transcript) {
        Some(i) => (Some(i), "keyword"),
        None => {
            let llm = match rt.block_on(claude.classify_intent(&transcript)) {
                Ok(i) => i,
                Err(e) => {
                    eprintln!("[classify] error: {e}");
                    return RunResult {
                        transcript: Some(transcript),
                        intent: None,
                        stt: stt_tail,
                        classify: None,
                        find_action: None,
                        total: None,
                        fired: false,
                    };
                }
            };
            (llm, "llm")
        }
    };
    let classify_dur = t_classify.elapsed();
    eprintln!(
        "[classifier] path={} → {:?} ({:?})",
        classifier_path, intent, classify_dur
    );

    // ── stage 3: find_action (only if intent says so) ──
    let (find_dur, fired) = match intent {
        Some(Intent::FindAction) => {
            let t_fa = Instant::now();
            let action_at = Arc::new(Mutex::new(None::<Duration>));
            let action_at_cb = action_at.clone();
            let cb = move |_action| {
                let mut slot = action_at_cb.lock().unwrap();
                if slot.is_none() {
                    *slot = Some(t_fa.elapsed());
                }
            };
            let result = rt.block_on(claude.find_action(
                &transcript,
                image_b64,
                x as i64,
                y as i64,
                w as i64,
                h as i64,
                cb,
            ));
            let dur = t_fa.elapsed();
            let fired = matches!(&result, Ok(Some(_)));
            (Some(dur), fired)
        }
        _ => (None, false),
    };

    // Total user-perceived: stt_tail + classify + find_action (if it ran).
    let total = match (stt_tail, find_dur) {
        (Some(s), Some(f)) => Some(s + classify_dur + f),
        _ => None,
    };

    RunResult {
        transcript: Some(transcript),
        intent,
        stt: stt_tail,
        classify: Some(classify_dur),
        find_action: find_dur,
        total,
        fired,
    }
}

fn empty_result() -> RunResult {
    RunResult {
        transcript: None,
        intent: None,
        stt: None,
        classify: None,
        find_action: None,
        total: None,
        fired: false,
    }
}

fn load_wav(path: &str) -> (Vec<i16>, u32, u16) {
    let mut reader = hound::WavReader::open(path)
        .unwrap_or_else(|e| panic!("could not open {path}: {e}"));
    let spec = reader.spec();
    assert_eq!(
        spec.sample_format,
        hound::SampleFormat::Int,
        "expected integer PCM WAV"
    );
    assert_eq!(spec.bits_per_sample, 16, "expected 16-bit PCM");
    let samples: Vec<i16> = reader
        .samples::<i16>()
        .collect::<Result<Vec<_>, _>>()
        .expect("WAV decode failed");
    (samples, spec.sample_rate, spec.channels)
}

fn print_stats<I: Iterator<Item = Duration>>(label: &str, iter: I) {
    let mut values: Vec<u128> = iter.map(|d| d.as_micros()).collect();
    if values.is_empty() {
        println!("  {}: (no samples)", label);
        return;
    }
    values.sort_unstable();
    let n = values.len();
    let mean_us = values.iter().sum::<u128>() / n as u128;
    let median_us = if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2
    };
    println!(
        "  {}: mean={:>8.1?}  median={:>8.1?}  min={:>8.1?}  max={:>8.1?}",
        label,
        Duration::from_micros(mean_us as u64),
        Duration::from_micros(median_us as u64),
        Duration::from_micros(values[0] as u64),
        Duration::from_micros(values[n - 1] as u64)
    );
}

fn fmt_opt(d: Option<Duration>) -> String {
    match d {
        Some(d) => {
            let ms = d.as_millis();
            if ms >= 1000 {
                format!("{:.2}s", d.as_secs_f64())
            } else {
                format!("{}ms", ms)
            }
        }
        None => "-".to_string(),
    }
}
