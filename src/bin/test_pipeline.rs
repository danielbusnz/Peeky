// Replays a fixed WAV file through the full Whisper → Claude → Cartesia → play
// pipeline. Use this to get reproducible timings without speech variance.
//
// Usage:
//   1. Record a baseline once (e.g., 3s of you saying "show me the close button"):
//        arecord -f S16_LE -r 16000 -c 1 -d 3 /tmp/aegis-baseline.wav
//   2. Re-run this bin to replay it through the pipeline:
//        cargo run --bin test_pipeline
//   3. Watch the [timing] lines.

#[path = "../audio.rs"]
mod audio;
#[path = "../hotkey/mod.rs"]
mod hotkey;
#[path = "../screenshot/mod.rs"]
mod screenshot;
#[path = "../providers/mod.rs"]
mod providers;

use providers::{Stt, Tts};
use std::path::Path;
use std::time::Instant;

const BASELINE_WAV: &str = "/tmp/aegis-baseline.wav";

fn main() {
    // Catch Hyprland's SIGUSR1/SIGUSR2 so the keybind doesn't kill us mid-test.
    let _ = hotkey::init();

    if !Path::new(BASELINE_WAV).exists() {
        eprintln!("baseline file missing: {}", BASELINE_WAV);
        eprintln!();
        eprintln!("create one by saying your test phrase into the mic:");
        eprintln!("  arecord -f S16_LE -r 16000 -c 1 -d 3 {}", BASELINE_WAV);
        eprintln!();
        eprintln!("then re-run this bin.");
        std::process::exit(1);
    }

    let (samples, sample_rate, channels) = match load_wav(BASELINE_WAV) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to load baseline wav: {}", e);
            return;
        }
    };
    println!(
        "loaded baseline: {} samples ({}Hz, {}ch) from {}",
        samples.len(),
        sample_rate,
        channels,
        BASELINE_WAV
    );

    let whisper = providers::whisper_openai::WhisperOpenAi::from_env()
        .expect("missing OPENAI_API_KEY");
    let claude = providers::claude::Claude::from_env().expect("missing ANTHROPIC_API_KEY");
    let cartesia =
        providers::tts_cartesia::TtsCartesia::from_env().expect("missing CARTESIA_API_KEY");

    println!();
    println!("=== pipeline timing ===");

    // Simulate the production flow: screenshot fires the moment the user
    // pressed the hotkey, in parallel with recording. By the time we mark
    // T=0 (release), the screenshot has already been running.
    let screenshot_handle =
        std::thread::spawn(|| -> Result<(i32, i32, u32, u32, String), String> {
            let (x, y, w, h) =
                screenshot::active_workspace_geometry().map_err(|e| e.to_string())?;
            let (b64, _, _) = screenshot::capture_for_claude(x, y, w as i32, h as i32)
                .map_err(|e| e.to_string())?;
            Ok((x, y, w, h, b64))
        });

    // Pretend the user just released the hotkey. T=0 starts here.
    let release_t = Instant::now();
    eprintln!("[timing] release → 0ms (simulated)");

    let transcript = match whisper.transcribe(&samples, sample_rate, channels) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("whisper failed: {}", e);
            return;
        }
    };
    eprintln!("[timing] whisper done → {:?}", release_t.elapsed());
    println!("you said: {}", transcript);

    let (x, y, w, h, b64) = match screenshot_handle.join() {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => {
            eprintln!("screenshot failed: {}", e);
            return;
        }
        Err(_) => {
            eprintln!("screenshot thread panicked");
            return;
        }
    };
    eprintln!("[timing] screenshot ready → {:?}", release_t.elapsed());

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    print!("claude: ");
    let result = rt.block_on(async {
        let cursor_task = claude.find_point(
            &transcript,
            &b64,
            x as i64,
            y as i64,
            w as i64,
            h as i64,
            |px, py| {
                eprintln!(
                    "\n[timing] CURSOR FIRES → {:?} (at {},{})",
                    release_t.elapsed(),
                    px,
                    py
                );
            },
        );

        let mut first_token_logged = false;
        let voice_task = claude.describe_with_image(&transcript, &b64, |token| {
            if !first_token_logged {
                eprintln!(
                    "[timing] first Claude text token → {:?}",
                    release_t.elapsed()
                );
                first_token_logged = true;
            }
            print!("{}", token);
            use std::io::Write;
            std::io::stdout().flush().ok();
        });

        let (_point_result, text_result) = tokio::join!(cursor_task, voice_task);
        text_result
    });
    println!();

    let text = match result {
        Ok(t) => t,
        Err(e) => {
            eprintln!("claude failed: {}", e);
            return;
        }
    };
    eprintln!("[timing] claude full text done → {:?}", release_t.elapsed());

    if !text.is_empty() {
        let handle = rodio::DeviceSinkBuilder::open_default_sink().expect("audio output");
        let player = rodio::Player::connect_new(handle.mixer());

        let channels =
            std::num::NonZeroU16::new(providers::tts_cartesia::STREAM_CHANNELS).unwrap();
        let sample_rate =
            std::num::NonZeroU32::new(providers::tts_cartesia::STREAM_SAMPLE_RATE).unwrap();

        let mut first_chunk_logged = false;
        let stream_result = rt.block_on(async {
            cartesia
                .synthesize_stream(&text, |pcm_bytes| {
                    if !first_chunk_logged {
                        eprintln!(
                            "[timing] SPEECH STARTS (first PCM chunk) → {:?}",
                            release_t.elapsed()
                        );
                        first_chunk_logged = true;
                    }
                    let samples: Vec<f32> = pcm_bytes
                        .chunks_exact(2)
                        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / i16::MAX as f32)
                        .collect();
                    player.append(rodio::buffer::SamplesBuffer::new(
                        channels,
                        sample_rate,
                        samples,
                    ));
                })
                .await
        });
        if let Err(e) = stream_result {
            eprintln!("cartesia stream failed: {}", e);
            return;
        }
        eprintln!(
            "[timing] cartesia stream complete → {:?}",
            release_t.elapsed()
        );
        player.sleep_until_end();
        eprintln!("[timing] speech done → {:?}", release_t.elapsed());
    }
}

fn load_wav(path: &str) -> Result<(Vec<f32>, u32, u16), Box<dyn std::error::Error>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    // Normalize i16 samples to f32 in [-1.0, 1.0] — what whisper.transcribe expects.
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| s.unwrap_or(0) as f32 / i16::MAX as f32)
        .collect();
    Ok((samples, spec.sample_rate, spec.channels))
}
