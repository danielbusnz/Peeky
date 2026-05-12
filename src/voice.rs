use crate::audio;
use crate::cursor;
use crate::hotkey;
use crate::providers::Tts;
use crate::providers::claude::Claude;
use crate::providers::stt_deepgram::SttDeepgram;
use crate::providers::tts_cartesia::TtsCartesia;
use crate::screenshot;

pub fn run_loop(stt: SttDeepgram, claude: Claude, cartesia: TtsCartesia) {
    // Tokio runtime owned by this thread. Streaming providers (Deepgram WS,
    // Claude SSE, Cartesia SSE) all run via `rt.block_on(...)`.
    let rt = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");

    println!("aegis ready — hold SUPER+space to talk");
    loop {
        hotkey::wait_for_press();

        // Pre-capture the screenshot the moment the user presses, in parallel
        // with recording + streaming STT.
        let screenshot_handle =
            std::thread::spawn(|| -> Result<(i32, i32, u32, u32, String), String> {
                let (x, y, w, h) =
                    screenshot::active_workspace_geometry().map_err(|e| e.to_string())?;
                let (b64, _, _) = screenshot::capture_for_claude(x, y, w as i32, h as i32)
                    .map_err(|e| e.to_string())?;
                Ok((x, y, w, h, b64))
            });

        if let Err(e) = run_one_turn(&rt, &stt, &claude, &cartesia, screenshot_handle) {
            eprintln!("voice turn failed: {}", e);
        }
    }
}

fn run_one_turn(
    rt: &tokio::runtime::Runtime,
    stt: &SttDeepgram,
    claude: &Claude,
    cartesia: &TtsCartesia,
    screenshot_handle: std::thread::JoinHandle<Result<(i32, i32, u32, u32, String), String>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Set up audio streaming: cpal callback writes i16 chunks into a tokio
    // channel; Deepgram WS consumes them and returns the final transcript
    // when the audio sender is dropped.
    let (audio_tx, audio_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();
    let (sample_rate, channels) = audio::input_config();

    // Spawn the Deepgram transcription task immediately so the WS opens
    // while audio starts flowing. The handle returns the final transcript.
    let stt_handle = {
        let api_key = stt.api_key.clone();
        rt.spawn(async move {
            let stt = SttDeepgram { api_key };
            stt.transcribe_stream(sample_rate, channels, audio_rx).await
        })
    };

    // Block this thread on the cpal capture loop. Audio chunks flow through
    // audio_tx → audio_rx → Deepgram WS. Returns when hotkey released.
    audio::record_stream(audio_tx);

    // T = 0: user just released the hotkey. Measure everything from here.
    let release_t = std::time::Instant::now();
    eprintln!("[timing] release → 0ms");

    // Audio tx was dropped at the end of record_stream → Deepgram saw EOS
    // → final transcript should arrive ~100-300ms later via the WS.
    let transcript = rt
        .block_on(stt_handle)
        .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?
        .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;
    eprintln!("[timing] STT final transcript → {:?}", release_t.elapsed());
    println!("you said: {}", transcript);

    // Guard: if the user said nothing, skip the rest of the turn.
    if transcript.trim().is_empty() {
        eprintln!("[voice] empty transcript, skipping turn");
        // Still need to drain the screenshot thread.
        let _ = screenshot_handle.join();
        return Ok(());
    }

    // Pull the pre-captured screenshot. By now it's been ready for seconds.
    let (x, y, w, h, b64) = screenshot_handle
        .join()
        .map_err(|_| "screenshot thread panicked")?
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    eprintln!("[timing] screenshot ready → {:?}", release_t.elapsed());

    // Fire two Claude calls in parallel:
    //   - find_point: minimal Computer Use call, fires cursor instantly
    //   - describe_with_image: streaming text response for voice
    print!("claude: ");
    let text = rt
        .block_on(async {
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
                    cursor::point_at(px as i32, py as i32);
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
        })
        .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;
    println!();
    eprintln!("[timing] claude full text done → {:?}", release_t.elapsed());

    if !text.is_empty() {
        let handle = rodio::DeviceSinkBuilder::open_default_sink()?;
        let player = rodio::Player::connect_new(handle.mixer());

        let chan_nz = std::num::NonZeroU16::new(crate::providers::tts_cartesia::STREAM_CHANNELS)
            .unwrap();
        let sr_nz =
            std::num::NonZeroU32::new(crate::providers::tts_cartesia::STREAM_SAMPLE_RATE).unwrap();

        let mut first_chunk_logged = false;
        rt.block_on(async {
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
                    player.append(rodio::buffer::SamplesBuffer::new(chan_nz, sr_nz, samples));
                })
                .await
        })
        .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;

        eprintln!(
            "[timing] cartesia stream complete → {:?}",
            release_t.elapsed()
        );
        player.sleep_until_end();
        eprintln!("[timing] speech done → {:?}", release_t.elapsed());
    }

    // Cartesia and Tts trait imports are still used elsewhere; silence warnings.
    let _ = std::convert::identity::<&dyn Tts>(cartesia);

    Ok(())
}
