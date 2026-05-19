use crate::audio;
use crate::barge_in::BargeIn;
use crate::ai_cursor;
use crate::hotkey;
use crate::intent::{is_integration_intent, wants_description};
use crate::providers::claude::Claude;
use crate::providers::stt_deepgram::SttDeepgram;
use crate::providers::tts_cartesia::TtsCartesia;
use crate::screenshot;
use crate::voice_session::VoiceSession;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[cfg(feature = "hyprland")]
fn set_cursor_idle() {
    crate::ai_cursor::set_state(crate::ai_cursor::CursorState::Idle);
}
#[cfg(not(feature = "hyprland"))]
fn set_cursor_idle() {}

pub fn run_loop(mic: audio::Mic, stt: SttDeepgram, claude: Claude, cartesia: TtsCartesia) {
    let session = VoiceSession::start(mic, stt, claude, cartesia);

    println!("aegis ready — hold SUPER+space to talk");
    loop {
        hotkey::wait_for_press();

        // Pre-capture the screenshot AND pre-resize for Computer Use, in
        // parallel with recording + streaming STT. The resize is now ~41ms
        // (fast_image_resize SIMD), so this thread usually finishes before
        // the user releases the hotkey. Single-pass capture+resize+encode
        // skips the full-res JPEG round-trip the old two-call path paid
        // (~2-3s on 5K screens) and saves ~200ms of upload + ~1500 input
        // tokens per turn.
        let screenshot_task =
            session
                .rt
                .spawn_blocking(|| -> Result<(i32, i32, u32, u32, String), String> {
                    let (x, y, w, h) =
                        screenshot::active_workspace_geometry().map_err(|e| e.to_string())?;
                    let (declared_w, declared_h) =
                        screenshot::pick_declared_resolution(w as i64, h as i64);
                    let resized_b64 = screenshot::capture_resized_for_claude(
                        x, y, w as i32, h as i32, declared_w, declared_h,
                    )
                    .map_err(|e| e.to_string())?;
                    Ok((x, y, w, h, resized_b64))
                });

        if let Err(e) = run_one_turn(&session, screenshot_task) {
            eprintln!("voice turn failed: {}", e);
        }
    }
}

fn run_one_turn(
    session: &VoiceSession,
    screenshot_task: tokio::task::JoinHandle<Result<(i32, i32, u32, u32, String), String>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Unpack session resources as locals so the rest of the function body
    // reads as if these were direct parameters. Pure shorthand; no copies.
    let rt = &session.rt;
    let mic = &session.mic;
    let audio_out = &session.audio_out;
    let stt = &session.stt;
    let claude = &session.claude;
    let cartesia = &session.cartesia;

    // ────── phase 1: record + transcribe ──────
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

    // ────── phase 2: await transcript & screenshot ──────
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
        // Still need to drain the screenshot task.
        let _ = rt.block_on(screenshot_task);
        return Ok(());
    }

    // Pull the pre-captured + pre-resized screenshot. If the user held
    // the hotkey for longer than the screenshot took, this returns
    // instantly. For very short turns, this can block 1-2s while the
    // Lanczos3 resize finishes — watch the timing log to see.
    let t_ss_join = std::time::Instant::now();
    let (x, y, w, h, resized_b64) = rt
        .block_on(screenshot_task)
        .map_err(|e| -> Box<dyn std::error::Error> {
            format!("screenshot task panicked: {e}").into()
        })?
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

    // ────── phase 3: set up state + spawn TTS streamer ──────
    // Barge-in detection: from this point on, watch for the user pressing
    // the hotkey AGAIN (new press during processing/playback). If detected,
    // abort all in-flight work and return immediately so the next loop
    // iteration starts a fresh turn.
    let barge_in = BargeIn::start();

    // First-feedback flag: cancelled when either TTS plays its first PCM
    // chunk or the cursor fires a visible action. Read by run_agent_loop
    // between steps to bail out once the user is already getting feedback,
    // so a chatty Claude can't keep iterating after the answer has started.
    let early_exit = CancellationToken::new();

    // Sentence channel: voice_task pushes complete sentences as Claude streams;
    // tts_task drains them and fires Cartesia per sentence.
    let (sentence_tx, mut sentence_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Spawn TTS as a fully independent task on the runtime so it gets its own
    // worker thread. Inside `tokio::join!`, all sub-futures share ONE task and
    // tts_task would be starved by voice_task's rapid token processing.
    // Spawning gives Cartesia a real chance to fire mid-stream.
    let cartesia_for_tts = cartesia.clone();
    let cancel_tts = barge_in.token();
    let early_exit_tts = early_exit.clone();
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
                _ = cancel_tts.cancelled() => {
                    eprintln!("[barge-in] tts aborted at {:?}", release_t.elapsed());
                    player.stop();
                    return Ok(());
                }
                recv = sentence_rx.recv() => {
                    let Some(sentence) = recv else { break };
                    tokio::select! {
                        biased;
                        _ = cancel_tts.cancelled() => {
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
                                early_exit_tts.cancel();
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
                _ = cancel_tts.cancelled() => {
                    eprintln!("[barge-in] tts aborted during playback at {:?}", release_t.elapsed());
                    player.stop();
                    return Ok(());
                }
                _ = tokio::time::sleep(Duration::from_millis(20)) => {}
            }
        }
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });

    // ────── phase 4: decide intent + run agent loop ──────
    // Whether the user's query expects a spoken answer at all. Point-and-click
    // style queries ("click X", "where is Y") get silent action; everything
    // else gets TTS of the agent loop's final text.
    let want_speech = wants_description(&transcript);
    // For clear integration-tool queries, skip uploading the screenshot on
    // step 1. Claude doesn't need pixels to decide on gmail_search /
    // gh_my_issues / spotify_play. Saves ~700ms of HTTP body upload.
    let skip_initial_screenshot = is_integration_intent(&transcript);
    eprintln!(
        "[query] {} → speak={} skip_screenshot={}",
        transcript.trim(),
        want_speech,
        skip_initial_screenshot
    );
    let initial_screenshot: &str = if skip_initial_screenshot {
        ""
    } else {
        resized_b64.as_str()
    };

    let cancel_claude = barge_in.token();
    print!("claude: ");
    rt.block_on(async {
        // Per-iteration screenshot capture. Re-queries the active workspace
        // geometry every call so the screenshot follows `switch_to_window`
        // and workspace switches mid-chain. Falls back to the initial
        // (x, y, w, h) if hyprctl fails. Uses the fast single-pass
        // capture+resize path.
        let take_screenshot =
            move || -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
                let (cx, cy, cw, ch) = screenshot::active_workspace_geometry()
                    .map(|g| (g.0, g.1, g.2 as i32, g.3 as i32))
                    .unwrap_or((x, y, w as i32, h as i32));
                let (dw, dh) = screenshot::pick_declared_resolution(cw as i64, ch as i64);
                screenshot::capture_resized_for_claude(cx, cy, cw, ch, dw, dh).map_err(
                    |e| -> Box<dyn std::error::Error + Send + Sync> { e.to_string().into() },
                )
            };

        let running_apps = crate::actions::list_running_apps();
        eprintln!(
            "[agent-loop] running apps detected: {}",
            if running_apps.is_empty() {
                "(none)".to_string()
            } else {
                running_apps.join(", ")
            }
        );

        // Streaming token-to-TTS state. When the agent loop fires
        // on_text_delta during a stream-safe step, we accumulate into
        // tts_buf and push complete sentences to sentence_tx as soon as a
        // boundary forms. did_stream tracks whether ANY tokens streamed,
        // so the post-await branch knows whether to also split final_text
        // or just flush the tail.
        let tts_buf: std::rc::Rc<std::cell::RefCell<String>> =
            std::rc::Rc::new(std::cell::RefCell::new(String::new()));
        let did_stream = std::rc::Rc::new(std::cell::Cell::new(false));
        let tts_buf_for_cb = tts_buf.clone();
        let did_stream_for_cb = did_stream.clone();
        let sentence_tx_for_cb = sentence_tx.clone();
        let stream_to_tts = want_speech;

        let early_exit_action = early_exit.clone();
        let user_email = crate::integrations::gmail::user_email();
        let cursor_task = claude.run_agent_loop(
            &transcript,
            initial_screenshot,
            &running_apps,
            user_email.as_deref(),
            x as i64,
            y as i64,
            w as i64,
            h as i64,
            crate::integrations::all_tools(),
            early_exit.clone(),
            take_screenshot,
            |action| {
                use crate::providers::claude::Action;
                eprintln!(
                    "\n[timing] ACTION FIRES → {:?}: {:?}",
                    release_t.elapsed(),
                    action
                );
                // Any user-visible action trips the first-feedback flag so
                // the agent loop bails out before its next round trip.
                // Integration tools don't qualify — they're research, not
                // user-visible work — so they fall through this match
                // without flipping the flag.
                if !matches!(action, Action::Integration) {
                    early_exit_action.cancel();
                }
                match action {
                    Action::Point { x: px, y: py } => {
                        set_cursor_idle();
                        ai_cursor::point_at(px as i32, py as i32);
                    }
                    Action::Click { x: px, y: py } => {
                        set_cursor_idle();
                        ai_cursor::point_at(px as i32, py as i32);
                        crate::actions::click_at(px, py);
                    }
                    Action::Type { text } => {
                        crate::actions::type_text(&text);
                    }
                    Action::Key { key } => {
                        crate::actions::press_key(&key);
                    }
                    Action::Scroll { direction, amount } => {
                        crate::actions::scroll(&direction, amount);
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
                    // Integration tools are NOT dispatched here; run_agent_loop
                    // handles them post-stream via dispatch_integration so their
                    // return values reach Claude as tool_result content.
                    Action::Integration => {}
                }
            },
            |name, input| {
                let result = crate::integrations::dispatch(name, input);
                if result.is_none() {
                    eprintln!("[integration] no handler for tool '{name}'");
                }
                result
            },
            move |delta: &str| {
                // Only stream to TTS if the user expects spoken output.
                if !stream_to_tts {
                    return;
                }
                did_stream_for_cb.set(true);
                let mut buf = tts_buf_for_cb.borrow_mut();
                buf.push_str(delta);
                while let Some(end) = find_sentence_end(&buf) {
                    let sentence: String = buf.drain(..=end).collect();
                    let _ = sentence_tx_for_cb.send(sentence);
                }
            },
        );

        // ────── phase 5: race against barge-in + flush final text ──────
        // Race the agent loop against a barge-in signal. If a new press
        // arrives, drop the future (cancels its HTTP streams).
        tokio::select! {
            biased;
            _ = cancel_claude.cancelled() => {
                eprintln!("[barge-in] claude aborted at {:?}", release_t.elapsed());
                drop(sentence_tx);
            }
            result = cursor_task => {
                match result {
                    Ok(final_text) => {
                        eprintln!(
                            "[timing] agent loop final text ready ({} chars, streamed={}) → {:?}",
                            final_text.len(),
                            did_stream.get(),
                            release_t.elapsed()
                        );
                        print!("{final_text}");
                        use std::io::Write;
                        std::io::stdout().flush().ok();
                        if want_speech && !final_text.trim().is_empty() {
                            if did_stream.get() {
                                // Streaming was active. Sentences already
                                // flowed to TTS as Claude produced them.
                                // Just flush the tail (partial sentence
                                // with no terminator).
                                let tail = tts_buf.borrow().trim().to_string();
                                if !tail.is_empty() {
                                    let _ = sentence_tx.send(tail);
                                }
                            } else {
                                // No streaming happened (e.g., first-step
                                // text-only answer like "what's on screen?").
                                // Fall back to post-hoc sentence split.
                                let mut buf = String::new();
                                for ch in final_text.chars() {
                                    buf.push(ch);
                                    while let Some(end) = find_sentence_end(&buf) {
                                        let sentence: String =
                                            buf.drain(..=end).collect();
                                        let _ = sentence_tx.send(sentence);
                                    }
                                }
                                let tail = buf.trim();
                                if !tail.is_empty() {
                                    let _ = sentence_tx.send(tail.to_string());
                                }
                            }
                        }
                        // Drop tx so tts_task's recv() returns None and winds
                        // down. If want_speech=false this drops with nothing
                        // queued and tts_task exits silently.
                        drop(sentence_tx);
                    }
                    Err(e) => {
                        eprintln!("[agent-loop] failed: {e}");
                        drop(sentence_tx);
                    }
                }
            }
        }
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    })
    .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;
    println!();

    // ────── phase 6: wait for TTS + cleanup ──────
    // Wait for tts_task to finish speaking everything before returning, but
    // also bail immediately on barge-in. (tts_task is also checking the
    // barge-in flag internally so it will stop the player on its own.)
    // We use an AbortHandle (cloneable) so the select! branch can abort
    // the task without consuming the JoinHandle.
    let tts_abort = tts_handle.abort_handle();
    let cancel_outer = barge_in.token();
    rt.block_on(async {
        tokio::select! {
            biased;
            _ = cancel_outer.cancelled() => {
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
