use crate::audio;
use crate::cursor;
use crate::hotkey;
use crate::providers::claude::Claude;
use crate::providers::stt_deepgram::SttDeepgram;
use crate::providers::tts_cartesia::TtsCartesia;
use crate::screenshot;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[cfg(feature = "hyprland")]
fn set_cursor_idle() {
    crate::cursor::set_state(crate::cursor::CursorState::Idle);
}
#[cfg(not(feature = "hyprland"))]
fn set_cursor_idle() {}

/// Returns true if the user's query expects a spoken description from Claude.
/// False for "just do X" commands — pointing AND action verbs — where the
/// effect (cursor flying, browser opening, app launching) is the response
/// and TTS narration would just be noise.
///
/// Conservative: defaults to true on anything ambiguous, so we err on the
/// side of giving more info rather than less.
fn wants_description(transcript: &str) -> bool {
    let lower = transcript.trim().to_lowercase();
    // Strip leading conversational filler AND polite-request scaffolding
    // so phrases like "can you open up youtube please" resolve to a bare
    // "open up youtube" before the action-prefix match.
    let stripped = lower
        .trim_start_matches("um, ")
        .trim_start_matches("uh, ")
        .trim_start_matches("ok. ")
        .trim_start_matches("ok, ")
        .trim_start_matches("okay. ")
        .trim_start_matches("okay, ")
        .trim_start_matches("no. ")
        .trim_start_matches("no, ")
        .trim_start_matches("hey, ")
        .trim_start_matches("hey ")
        .trim_start_matches("can you ")
        .trim_start_matches("could you ")
        .trim_start_matches("would you ")
        .trim_start_matches("please ")
        .trim_start_matches("i want to ")
        .trim_start_matches("i'd like to ")
        .trim_start_matches("i want you to ")
        .trim_start_matches("let's ")
        .trim_start_matches("lets ")
        .trim_start_matches("just ");

    // Phrases that are unambiguously commands, not questions. Pointing verbs
    // ("where is X", "click X") and action verbs ("open X", "launch X",
    // "switch to X", "go to X") all dispatch through find_action's tools;
    // narrating them adds latency without adding info.
    let action_starts = [
        // Pointing
        "where is",
        "where's",
        "where are",
        "click",
        "click on",
        "point at",
        "point to",
        // Actions
        "open ",
        "launch ",
        "switch to ",
        "switch ",
        "go to ",
        "navigate to ",
        "focus ",
        "start ",
        // Media / integration commands — these route to tools like
        // spotify_play, never narration.
        "play ",
        "pause",
        "resume",
        "skip",
        "next track",
        "previous track",
    ];
    !action_starts.iter().any(|p| stripped.starts_with(p))
}

pub fn run_loop(mic: audio::Mic, stt: SttDeepgram, claude: Claude, cartesia: TtsCartesia) {
    // Tokio runtime owned by this thread. Streaming providers (Deepgram WS,
    // Claude SSE, Cartesia SSE) all run via `rt.block_on(...)`.
    let rt = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");

    // Warm up cpal once on this thread (cpal::Stream is !Send). The stream
    // runs forever; per-turn we just install a sender to start forwarding.
    let running_mic = mic.start();

    // Open the audio output sink ONCE at startup. Per-turn we just hand
    // out a fresh Player against this sink (~free).
    let audio_out = audio::AudioOutput::init().expect("could not open audio output");

    println!("aegis ready — hold SUPER+space to talk");
    loop {
        hotkey::wait_for_press();

        // Pre-capture the screenshot AND pre-resize for Computer Use, in
        // parallel with recording + streaming STT. The resize is CPU-heavy
        // (~2s for Lanczos3), so doing it here saves that time off the
        // critical path.
        // Capture + resize in parallel with recording + STT. The resize is
        // now ~41ms (fast_image_resize SIMD), so this thread usually finishes
        // before the user releases the hotkey. We return only the resized
        // image now — describe used to need the native-res version, but it
        // gets the same resized one and saves ~200ms of upload + ~1500 input
        // tokens per turn.
        let screenshot_handle =
            std::thread::spawn(|| -> Result<(i32, i32, u32, u32, String), String> {
                let (x, y, w, h) =
                    screenshot::active_workspace_geometry().map_err(|e| e.to_string())?;
                let (declared_w, declared_h) =
                    screenshot::pick_declared_resolution(w as i64, h as i64);
                // Single-pass capture+resize+encode. Skips the full-res JPEG
                // round-trip the old two-call path paid (~2-3s on 5K screens).
                let resized_b64 = screenshot::capture_resized_for_claude(
                    x, y, w as i32, h as i32, declared_w, declared_h,
                )
                .map_err(|e| e.to_string())?;
                Ok((x, y, w, h, resized_b64))
            });

        if let Err(e) = run_one_turn(
            &rt,
            &running_mic,
            &audio_out,
            &stt,
            &claude,
            &cartesia,
            screenshot_handle,
        ) {
            eprintln!("voice turn failed: {}", e);
        }
    }
}

fn run_one_turn(
    rt: &tokio::runtime::Runtime,
    mic: &audio::LiveMic,
    audio_out: &audio::AudioOutput,
    stt: &SttDeepgram,
    claude: &Claude,
    cartesia: &TtsCartesia,
    screenshot_handle: std::thread::JoinHandle<
        Result<(i32, i32, u32, u32, String), String>,
    >,
) -> Result<(), Box<dyn std::error::Error>> {
    // Set up audio streaming: cpal callback writes i16 chunks into a tokio
    // channel; Deepgram WS consumes them and returns the final transcript
    // when the audio sender is dropped.
    let (audio_tx, audio_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();
    let sample_rate = mic.sample_rate;
    let channels = mic.channels;

    // Spawn the Deepgram transcription task immediately so the WS opens
    // while audio starts flowing. The handle returns the final transcript.
    let stt_handle = {
        let stt = stt.clone();
        rt.spawn(async move {
            // TODO(#22): pass Some(interim_tx) here once speculative Claude
            // orchestration is wired up. transcribe_stream already supports
            // broadcasting interims for stability detection.
            stt.transcribe_stream(sample_rate, channels, audio_rx, None)
                .await
        })
    };

    // Install audio_tx as the active forwarding target. The cpal stream
    // is already running; this just flips the switch. Returns when the
    // hotkey is released and uninstalls the sender, which closes the
    // channel and triggers Deepgram's Strategy B return.
    mic.capture_until_release(audio_tx);

    // T = 0: user just released the hotkey. Measure everything from here.
    let release_t = std::time::Instant::now();
    eprintln!("[timing] release → 0ms");

    // Audio tx was dropped at the end of record_stream → Deepgram saw EOS
    // → final transcript should arrive ~100-300ms later via the WS.
    let transcript = rt
        .block_on(stt_handle)
        .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?
        .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;
    println!("you said: {}", transcript);

    // Guard: if the user said nothing, skip the rest of the turn.
    if transcript.trim().is_empty() {
        eprintln!("[voice] empty transcript, skipping turn");
        set_cursor_idle();
        // Still need to drain the screenshot thread.
        let _ = screenshot_handle.join();
        return Ok(());
    }

    // Pull the pre-captured + pre-resized screenshot. If the user held
    // the hotkey for longer than the screenshot took, this returns
    // instantly. For very short turns, this can block 1-2s while the
    // Lanczos3 resize finishes — watch the timing log to see.
    let t_ss_join = std::time::Instant::now();
    let (x, y, w, h, resized_b64) = screenshot_handle
        .join()
        .map_err(|_| "screenshot thread panicked")?
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    let ss_wait = t_ss_join.elapsed();
    if ss_wait.as_millis() > 50 {
        eprintln!(
            "[timing] screenshot join BLOCKED for {:?} (short turn / slow resize)",
            ss_wait
        );
    } else {
        eprintln!("[timing] screenshot ready → {:?}", release_t.elapsed());
    }

    // Barge-in detection: from this point on, watch for the user pressing
    // the hotkey AGAIN (new press during processing/playback). If detected,
    // abort all in-flight work and return immediately so the next loop
    // iteration starts a fresh turn.
    let barge_in = BargeIn::start();

    // Sentence channel: voice_task pushes complete sentences as Claude streams;
    // tts_task drains them and fires Cartesia per sentence.
    let (sentence_tx, mut sentence_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Spawn TTS as a fully independent task on the runtime so it gets its own
    // worker thread. Inside `tokio::join!`, all sub-futures share ONE task and
    // tts_task would be starved by voice_task's rapid token processing.
    // Spawning gives Cartesia a real chance to fire mid-stream.
    let cartesia_for_tts = cartesia.clone();
    let barge_in_flag_tts = barge_in.flag.clone();
    // Hand out a fresh Player from the cached sink (cheap). The expensive
    // open_default_sink() happens once at app startup.
    let player = audio_out.new_player();
    let chan_nz = audio_out.channels;
    let sr_nz = audio_out.sample_rate;
    let tts_handle = rt.spawn(async move {

        let mut first_chunk_logged = false;
        loop {
            tokio::select! {
                biased;
                _ = wait_for_barge_in(&barge_in_flag_tts) => {
                    eprintln!("[barge-in] tts aborted at {:?}", release_t.elapsed());
                    player.stop();
                    return Ok(());
                }
                recv = sentence_rx.recv() => {
                    let Some(sentence) = recv else { break };
                    tokio::select! {
                        biased;
                        _ = wait_for_barge_in(&barge_in_flag_tts) => {
                            eprintln!("[barge-in] tts aborted at {:?}", release_t.elapsed());
                            player.stop();
                            return Ok(());
                        }
                        result = cartesia_for_tts.synthesize_stream(&sentence, |pcm_bytes| {
                            if !first_chunk_logged {
                                eprintln!(
                                    "[timing] SPEECH STARTS (first PCM chunk) → {:?}",
                                    release_t.elapsed()
                                );
                                first_chunk_logged = true;
                                set_cursor_idle();
                            }
                            let samples: Vec<f32> = pcm_bytes
                                .chunks_exact(2)
                                .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / i16::MAX as f32)
                                .collect();
                            player.append(rodio::buffer::SamplesBuffer::new(chan_nz, sr_nz, samples));
                        }) => {
                            result.map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                                e.to_string().into()
                            })?;
                        }
                    }
                }
            }
        }
        eprintln!(
            "[timing] cartesia stream complete → {:?}",
            release_t.elapsed()
        );
        // Wait for playback to finish, but bail immediately on barge-in.
        while !player.empty() {
            tokio::select! {
                biased;
                _ = wait_for_barge_in(&barge_in_flag_tts) => {
                    eprintln!("[barge-in] tts aborted during playback at {:?}", release_t.elapsed());
                    player.stop();
                    return Ok(());
                }
                _ = tokio::time::sleep(Duration::from_millis(20)) => {}
            }
        }
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });

    // `voice_task` is `async move` so it owns and drops `sentence_tx` at its
    // end, signalling the TTS task to wind down. That move conflicts with
    // `cursor_task`'s borrow of `transcript`, so clone the small string.
    let transcript_for_voice = transcript.clone();

    // Decide whether to spend a second Claude call on a spoken description.
    // Unambiguous "where is X?" / "click X" style queries don't need one —
    // the cursor pointing IS the answer. Skipping cuts ~1s off perceived
    // latency for those queries.
    let want_desc = wants_description(&transcript);
    eprintln!(
        "[query] {} → describe={}",
        transcript.trim(),
        want_desc
    );

    // Both Claude calls now share the same resized image. find_action still
    // borrows resized_b64 directly; voice_task is `async move` so it needs
    // its own copy. Clone is ~226KB, happens once per turn.
    let resized_b64_for_voice = resized_b64.clone();

    let barge_in_flag_claude = barge_in.flag.clone();
    print!("claude: ");
    rt.block_on(async {
        // Per-iteration screenshot capture. Re-queries the active workspace
        // geometry every call so the screenshot follows `switch_to_window`
        // and workspace switches mid-chain. Falls back to the initial
        // (x, y, w, h) if hyprctl fails. Uses the fast single-pass
        // capture+resize path.
        let take_screenshot = move || -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
            let (cx, cy, cw, ch) = screenshot::active_workspace_geometry()
                .map(|g| (g.0, g.1, g.2 as i32, g.3 as i32))
                .unwrap_or((x, y, w as i32, h as i32));
            let (dw, dh) = screenshot::pick_declared_resolution(cw as i64, ch as i64);
            screenshot::capture_resized_for_claude(cx, cy, cw, ch, dw, dh)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.to_string().into() })
        };

        let running_apps = crate::actions::list_running_apps();
        eprintln!(
            "[agent-loop] running apps detected: {}",
            if running_apps.is_empty() { "(none)".to_string() } else { running_apps.join(", ") }
        );

        let cursor_task = claude.run_agent_loop(
            &transcript,
            &resized_b64,
            &running_apps,
            x as i64,
            y as i64,
            w as i64,
            h as i64,
            take_screenshot,
            |action| {
                use crate::providers::claude::Action;
                eprintln!(
                    "\n[timing] ACTION FIRES → {:?}: {:?}",
                    release_t.elapsed(),
                    action
                );
                match action {
                    Action::Point { x: px, y: py } => {
                        set_cursor_idle();
                        cursor::point_at(px as i32, py as i32);
                    }
                    Action::Click { x: px, y: py } => {
                        set_cursor_idle();
                        cursor::point_at(px as i32, py as i32);
                        crate::actions::click_at(px, py);
                    }
                    Action::Type { text } => {
                        crate::actions::type_text(&text);
                    }
                    Action::Key { key } => {
                        crate::actions::press_key(&key);
                    }
                    Action::OpenUrl { url } => {
                        set_cursor_idle();
                        crate::actions::open_url(&url);
                    }
                    Action::LaunchApp { app } => {
                        set_cursor_idle();
                        crate::actions::launch_app(&app);
                    }
                    Action::SwitchToWindow { target } => {
                        set_cursor_idle();
                        crate::actions::switch_to_window(&target);
                    }
                    Action::Integration { name, input } => {
                        set_cursor_idle();
                        if !crate::integrations::dispatch(&name, &input) {
                            eprintln!(
                                "[integration] no handler for tool '{}' input={}",
                                name, input
                            );
                        }
                    }
                }
            },
        );

        let voice_task = async move {
            if !want_desc {
                eprintln!("[timing] skipping describe (point-only query)");
                // Drop sentence_tx so tts_task's recv() returns None and it
                // winds down without speaking anything.
                drop(sentence_tx);
                return Ok::<String, Box<dyn std::error::Error + Send + Sync>>(String::new());
            }
            let mut sentence_buf = String::new();
            let mut first_token_logged = false;
            let result = claude
                .describe_with_image(&transcript_for_voice, &resized_b64_for_voice, |token| {
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

                    sentence_buf.push_str(token);
                    while let Some(end) = find_sentence_end(&sentence_buf) {
                        let sentence: String = sentence_buf.drain(..=end).collect();
                        let _ = sentence_tx.send(sentence);
                    }
                })
                .await;
            // Flush any tail that didn't hit a sentence boundary.
            let tail = sentence_buf.trim();
            if !tail.is_empty() {
                let _ = sentence_tx.send(tail.to_string());
            }
            eprintln!("[timing] claude full text done → {:?}", release_t.elapsed());
            // sentence_tx drops here → tts_task's recv() returns None.
            result
        };

        // Race the Claude work against a barge-in signal. If a new press
        // arrives, drop both futures (which cancels their HTTP streams).
        tokio::select! {
            biased;
            _ = wait_for_barge_in(&barge_in_flag_claude) => {
                eprintln!("[barge-in] claude aborted at {:?}", release_t.elapsed());
            }
            joined = async {
                tokio::join!(cursor_task, voice_task)
            } => {
                let (_, voice_res) = joined;
                voice_res?;
            }
        }
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    })
    .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;
    println!();

    // Wait for tts_task to finish speaking everything before returning, but
    // also bail immediately on barge-in. (tts_task is also checking the
    // barge-in flag internally so it will stop the player on its own.)
    // We use an AbortHandle (cloneable) so the select! branch can abort
    // the task without consuming the JoinHandle.
    let tts_abort = tts_handle.abort_handle();
    let barge_in_flag_outer = barge_in.flag.clone();
    rt.block_on(async {
        tokio::select! {
            biased;
            _ = wait_for_barge_in(&barge_in_flag_outer) => {
                tts_abort.abort();
                Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
            }
            result = tts_handle => {
                match result {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(e),
                    Err(e) => Err(e.to_string().into()),
                }
            }
        }
    })
    .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;

    // Safety net: return to Idle if neither the cursor callback nor the
    // first-PCM-chunk path fired (errors, no-op turns). EXCEPTION: if the
    // user is currently pressing the hotkey, they've already queued a
    // Listening message via on_press for the next turn — firing Idle here
    // would clobber it on the cursor's drain loop ("latest wins"), and
    // the soundwave would never render.
    if !hotkey::is_recording() {
        set_cursor_idle();
    }

    Ok(())
}

/// Barge-in detector: spawns a background thread that watches for the user
/// pressing the hotkey AGAIN (after the current turn's release), and flips
/// a shared atomic flag when it happens. Async code racing against this
/// flag can abort their in-flight work.
///
/// On drop, signals the watchdog thread to exit. Construct AFTER the
/// hotkey has been released (RECORDING is false); the watchdog interprets
/// the next true→false→true cycle as a new press.
struct BargeIn {
    flag: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
}

impl BargeIn {
    fn start() -> Self {
        let flag = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let flag_w = flag.clone();
        let shutdown_w = shutdown.clone();
        std::thread::spawn(move || {
            // Watchdog: as soon as hotkey::is_recording() flips true again,
            // flip the barge-in flag. Exits when shutdown is signalled.
            while !shutdown_w.load(Ordering::Relaxed) {
                if hotkey::is_recording() {
                    flag_w.store(true, Ordering::Relaxed);
                    return;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        BargeIn { flag, shutdown }
    }
}

impl Drop for BargeIn {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

/// Async helper: yields until the barge-in flag flips true. Used inside
/// `tokio::select!` arms to race the flag against normal work.
async fn wait_for_barge_in(flag: &Arc<AtomicBool>) {
    while !flag.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

/// Find the byte index of the first sentence-ending punctuation followed by
/// whitespace or end-of-buffer. Returns None if no boundary is present.
/// Only matches ASCII '.', '!', '?' which are safe to slice on in UTF-8.
fn find_sentence_end(buf: &str) -> Option<usize> {
    let bytes = buf.as_bytes();
    for i in 0..bytes.len() {
        if matches!(bytes[i], b'.' | b'!' | b'?') {
            if i + 1 == bytes.len() || matches!(bytes[i + 1], b' ' | b'\n' | b'\t') {
                return Some(i);
            }
        }
    }
    None
}
