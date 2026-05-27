//! Voice-turn orchestrator. One iteration per hotkey press:
//!   1. Record + transcribe (Deepgram WS).
//!   2. Classify intent (small Claude call, in parallel with screenshot join).
//!   3. Set up shared per-turn infra (barge-in, TTS pipeline, sentence channel).
//!   4. Branch on intent: find_action / integration / chat / memory / agent.
//!   5. Race the branch against barge-in; flush tail text to TTS.
//!   6. Wait for TTS playback, then return.

use crate::ai_cursor;
use crate::audio;
use crate::barge_in::BargeIn;
use crate::hotkey;
use crate::intent::keyword_classify;
use crate::providers::claude::{Claude, Intent};
use crate::providers::stt_deepgram::SttDeepgram;
use crate::providers::tts_cartesia::TtsCartesia;
use crate::screenshot;
use crate::voice_session::VoiceSession;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Output of the pre-turn screenshot task: (x, y, w, h, base64 JPEG).
/// Named so the JoinHandle signature stops tripping clippy::type_complexity.
type ScreenshotResult = Result<(i32, i32, u32, u32, String), String>;

fn set_cursor_idle() {
    crate::ai_cursor::set_state(crate::ai_cursor::CursorState::Idle);
}

pub fn run_loop(mic: audio::Mic, stt: SttDeepgram, claude: Claude, cartesia: TtsCartesia) {
    let session = VoiceSession::start(mic, stt, claude, cartesia);

    println!("aegis ready. hold Ctrl+Space to talk");
    loop {
        hotkey::wait_for_press();
        let press_t = std::time::Instant::now();

        // Pre-capture the screenshot in parallel with recording + STT. Many
        // turns won't need it (chat, integration, memory) but we don't know
        // that until the classifier returns. Capturing eagerly keeps the
        // hot paths fast; the cost when unused is just thrown-away pixels.
        let screenshot_task = session.rt.spawn_blocking(|| -> ScreenshotResult {
            let (x, y, w, h) =
                screenshot::active_workspace_geometry().map_err(|e| e.to_string())?;
            let (declared_w, declared_h) = screenshot::pick_declared_resolution(w as i64, h as i64);
            let resized_b64 = screenshot::capture_resized_for_claude(
                x, y, w as i32, h as i32, declared_w, declared_h,
            )
            .map_err(|e| e.to_string())?;
            Ok((x, y, w, h, resized_b64))
        });

        if let Err(e) = run_one_turn(&session, press_t, screenshot_task) {
            eprintln!("voice turn failed: {}", e);
        }
    }
}

fn run_one_turn(
    session: &VoiceSession,
    press_t: std::time::Instant,
    screenshot_task: tokio::task::JoinHandle<ScreenshotResult>,
) -> Result<(), Box<dyn std::error::Error>> {
    let rt = &session.rt;
    let mic = &session.mic;
    let audio_out = &session.audio_out;
    let stt = &session.stt;
    let claude = &session.claude;
    let cartesia = &session.cartesia;
    let memory = &session.memory;

    // ────── phase 1: record + transcribe ──────
    let (audio_tx, audio_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();
    let sample_rate = mic.sample_rate;
    let channels = mic.channels;

    let stt_handle = {
        let stt = stt.clone();
        rt.spawn(async move {
            stt.transcribe_stream(sample_rate, channels, audio_rx, None)
                .await
        })
    };
    mic.capture_until_release(audio_tx);

    // ────── phase 2: await transcript, fire classifier ──────
    let release_t = std::time::Instant::now();
    eprintln!(
        "[timing] release → 0ms (hold={:?})",
        release_t.duration_since(press_t)
    );

    let transcript = rt
        .block_on(stt_handle)
        .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?
        .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;
    eprintln!("[timing] transcript ready → {:?}", release_t.elapsed());
    println!("you said: {}", transcript);

    if transcript.trim().is_empty() {
        eprintln!("[voice] empty transcript, skipping turn");
        set_cursor_idle();
        let _ = rt.block_on(screenshot_task);
        return Ok(());
    }

    // Hybrid classification: try the local keyword classifier first
    // (sub-millisecond). If it returns Some, we skip the LLM round-trip
    // entirely. If it returns None, fall through to the LLM classifier
    // (~700ms), spawned in parallel with the screenshot join so the
    // wait at least overlaps with the screenshot resize.
    let keyword_intent = keyword_classify(&transcript);
    let classifier_task = if keyword_intent.is_some() {
        None
    } else {
        let claude = claude.clone();
        let transcript_for_classifier = transcript.clone();
        Some(rt.spawn(async move { claude.classify_intent(&transcript_for_classifier).await }))
    };

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

    let intent_result = match (keyword_intent, classifier_task) {
        (Some(i), _) => {
            eprintln!(
                "[classifier] keyword match → {:?} at {:?} (LLM skipped)",
                i,
                release_t.elapsed()
            );
            Some(i)
        }
        (None, Some(task)) => {
            let llm_intent = rt
                .block_on(task)
                .map_err(|e| -> Box<dyn std::error::Error> {
                    format!("classifier task panicked: {e}").into()
                })?
                .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;
            eprintln!(
                "[classifier] LLM fallback → {:?} at {:?}",
                llm_intent,
                release_t.elapsed()
            );
            llm_intent
        }
        (None, None) => None, // unreachable in practice; guard anyway
    };
    eprintln!(
        "[timing] classifier ready → {:?}: {:?}",
        release_t.elapsed(),
        intent_result
    );

    // Fail loud if the classifier couldn't pick a category. The current
    // contract: speak a short error and stop. No silent fallback to
    // run_agent_loop. Surprises here mean the prompt drifted or the API
    // changed, and we want to see it.
    let intent = match intent_result {
        Some(i) => i,
        None => {
            return speak_error(
                rt,
                cartesia,
                audio_out,
                "I'm not sure how to handle that. Try rephrasing.",
            );
        }
    };

    // ────── phase 3: shared per-turn infra (barge-in, TTS pipeline) ──────
    let barge_in = BargeIn::start();
    let early_exit = CancellationToken::new();
    let (sentence_tx, mut sentence_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let cartesia_for_tts = cartesia.clone();
    let cancel_tts = barge_in.token();
    let early_exit_tts = early_exit.clone();
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

    // ────── phase 4: branch on intent ──────
    let cancel_claude = barge_in.token();
    print!("claude: ");
    rt.block_on(async {
        match intent {
            Intent::FindAction => {
                eprintln!("[intent] → FindAction for: {:?}", transcript);
                run_find_action(
                    claude,
                    &transcript,
                    &resized_b64,
                    x,
                    y,
                    w,
                    h,
                    release_t,
                    &early_exit,
                    &cancel_claude,
                    &sentence_tx,
                )
                .await
            }
            Intent::Chat => {
                eprintln!("[intent] → Chat for: {:?}", transcript);
                run_chat(
                    claude,
                    &transcript,
                    &resized_b64,
                    memory,
                    release_t,
                    &cancel_claude,
                    &sentence_tx,
                )
                .await
            }
            Intent::Integration => {
                run_integration(
                    claude,
                    &transcript,
                    memory,
                    release_t,
                    &cancel_claude,
                    &sentence_tx,
                )
                .await
            }
            Intent::Memory => {
                run_memory(
                    claude,
                    &transcript,
                    memory,
                    release_t,
                    &cancel_claude,
                    &sentence_tx,
                )
                .await
            }
            Intent::Agent => {
                run_agent(
                    claude,
                    &transcript,
                    &resized_b64,
                    x,
                    y,
                    w,
                    h,
                    release_t,
                    &early_exit,
                    &cancel_claude,
                    &sentence_tx,
                )
                .await
            }
        }
    });
    println!();
    drop(sentence_tx);

    // ────── phase 6: wait for TTS + cleanup ──────
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

    if !hotkey::is_recording() {
        set_cursor_idle();
    }

    Ok(())
}

// ────── per-intent dispatchers ──────

/// Dispatch the FindAction path. One Claude call with the screenshot;
/// the action callback fires when Claude's tool input finishes streaming,
/// kicking off the cursor/click/type/scroll immediately. `early_exit`
/// is set so we don't spend extra Claude turns after feedback already
/// reached the user.
#[allow(clippy::too_many_arguments)]
async fn run_find_action(
    claude: &Claude,
    transcript: &str,
    screenshot_b64: &str,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    release_t: std::time::Instant,
    early_exit: &CancellationToken,
    cancel_claude: &CancellationToken,
    sentence_tx: &tokio::sync::mpsc::UnboundedSender<String>,
) {
    let early_exit_action = early_exit.clone();
    let action_cb = move |action| {
        eprintln!("[timing] claude first response → {:?}", release_t.elapsed());
        eprintln!(
            "[timing] ACTION FIRES → {:?}: {:?}",
            release_t.elapsed(),
            action
        );
        dispatch_action(action, &early_exit_action);
    };

    tokio::select! {
        biased;
        _ = cancel_claude.cancelled() => {
            eprintln!("[barge-in] find_action aborted at {:?}", release_t.elapsed());
            let _ = sentence_tx;
        }
        result = claude.find_action(
            transcript, screenshot_b64,
            x as i64, y as i64, w as i64, h as i64,
            action_cb,
        ) => {
            if let Err(e) = result {
                eprintln!("[find_action] failed: {e}");
            }
        }
    }
}

/// Dispatch the Chat path. Now includes screenshot for visual context.
/// Text deltas land in the sentence channel which the TTS task pulls from.
async fn run_chat(
    claude: &Claude,
    transcript: &str,
    screenshot_b64: &str,
    memory: &crate::providers::claude::MemoryStore,
    release_t: std::time::Instant,
    cancel_claude: &CancellationToken,
    sentence_tx: &tokio::sync::mpsc::UnboundedSender<String>,
) {
    let profile = memory.as_prompt_block();
    let mut helper = StreamHelper::new(sentence_tx.clone(), release_t);
    tokio::select! {
        biased;
        _ = cancel_claude.cancelled() => {
            eprintln!("[barge-in] chat aborted at {:?}", release_t.elapsed());
        }
        result = claude.chat(
            transcript, screenshot_b64, profile.as_deref(),
            |delta| helper.push_delta(delta),
        ) => {
            match result {
                Ok(final_text) => {
                    print!("{final_text}");
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                    helper.flush_tail();
                }
                Err(e) => eprintln!("[chat] failed: {e}"),
            }
        }
    }
}

/// Dispatch the Integration path. Two Claude calls (pick tool, then
/// compose summary) with `crate::integrations::dispatch` running in
/// between to actually hit the service API.
async fn run_integration(
    claude: &Claude,
    transcript: &str,
    memory: &crate::providers::claude::MemoryStore,
    release_t: std::time::Instant,
    cancel_claude: &CancellationToken,
    sentence_tx: &tokio::sync::mpsc::UnboundedSender<String>,
) {
    let profile = memory.as_prompt_block();
    let mut helper = StreamHelper::new(sentence_tx.clone(), release_t);
    let dispatch = |name: &str, input: &serde_json::Value| {
        let result = crate::integrations::dispatch(name, input);
        if result.is_none() {
            eprintln!("[integration] no handler for tool '{name}'");
        }
        result
    };
    tokio::select! {
        biased;
        _ = cancel_claude.cancelled() => {
            eprintln!("[barge-in] integration aborted at {:?}", release_t.elapsed());
        }
        result = claude.integration(
            transcript,
            crate::integrations::all_tools(),
            profile.as_deref(),
            dispatch,
            |delta| helper.push_delta(delta),
        ) => {
            match result {
                Ok(final_text) => {
                    print!("{final_text}");
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                    helper.flush_tail();
                }
                Err(e) => eprintln!("[integration] failed: {e}"),
            }
        }
    }
}

/// Dispatch the Memory path. Routes to the local JSONL store via
/// `Claude::memory`; the response is a templated reply (not a streamed
/// second Claude call) so it's the fastest path after find_action.
async fn run_memory(
    claude: &Claude,
    transcript: &str,
    memory: &crate::providers::claude::MemoryStore,
    release_t: std::time::Instant,
    cancel_claude: &CancellationToken,
    sentence_tx: &tokio::sync::mpsc::UnboundedSender<String>,
) {
    let mut helper = StreamHelper::new(sentence_tx.clone(), release_t);
    tokio::select! {
        biased;
        _ = cancel_claude.cancelled() => {
            eprintln!("[barge-in] memory aborted at {:?}", release_t.elapsed());
        }
        result = claude.memory(transcript, memory, |delta| helper.push_delta(delta)) => {
            match result {
                Ok(final_text) => {
                    print!("{final_text}");
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                    helper.flush_tail();
                }
                Err(e) => eprintln!("[memory] failed: {e}"),
            }
        }
    }
}

/// Dispatch the Agent path. Multi-step agent loop with iterative
/// screenshots between steps. Each tool call fires its action via
/// `dispatch_action`, then a fresh screenshot is captured for the next
/// step. Bounded by AGENT_MAX_STEPS in tuning.rs.
#[allow(clippy::too_many_arguments)]
async fn run_agent(
    claude: &Claude,
    transcript: &str,
    screenshot_b64: &str,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    release_t: std::time::Instant,
    early_exit: &CancellationToken,
    cancel_claude: &CancellationToken,
    sentence_tx: &tokio::sync::mpsc::UnboundedSender<String>,
) {
    let take_screenshot = move || -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let (cx, cy, cw, ch) = screenshot::active_workspace_geometry()
            .map(|g| (g.0, g.1, g.2 as i32, g.3 as i32))
            .unwrap_or((x, y, w as i32, h as i32));
        let (dw, dh) = screenshot::pick_declared_resolution(cw as i64, ch as i64);
        screenshot::capture_resized_for_claude(cx, cy, cw, ch, dw, dh)
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.to_string().into() })
    };

    let running_apps = crate::desktop::list_running_apps();
    eprintln!(
        "[agent-loop] running apps detected: {}",
        if running_apps.is_empty() {
            "(none)".to_string()
        } else {
            running_apps.join(", ")
        }
    );

    let user_email = crate::integrations::gmail::user_email();
    let early_exit_action = early_exit.clone();
    let mut helper = StreamHelper::new(sentence_tx.clone(), release_t);

    let cursor_task = claude.run_agent_loop(
        transcript,
        screenshot_b64,
        &running_apps,
        user_email.as_deref(),
        x as i64,
        y as i64,
        w as i64,
        h as i64,
        crate::integrations::all_tools(),
        early_exit.clone(),
        take_screenshot,
        move |action| {
            eprintln!(
                "[timing] ACTION FIRES → {:?}: {:?}",
                release_t.elapsed(),
                action
            );
            dispatch_action(action, &early_exit_action);
        },
        |name, input| {
            let result = crate::integrations::dispatch(name, input);
            if result.is_none() {
                eprintln!("[integration] no handler for tool '{name}'");
            }
            result
        },
        |delta: &str| {
            helper.push_delta(delta);
        },
    );

    tokio::select! {
        biased;
        _ = cancel_claude.cancelled() => {
            eprintln!("[barge-in] agent aborted at {:?}", release_t.elapsed());
        }
        result = cursor_task => {
            match result {
                Ok(final_text) => {
                    print!("{final_text}");
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                    helper.flush_tail();
                }
                Err(e) => eprintln!("[agent-loop] failed: {e}"),
            }
        }
    }
}

// ────── shared helpers ──────

/// Action dispatcher used by both find_action and the multi-step agent.
/// Fires the side effect (cursor move, click, type, open URL, launch app,
/// etc.) and trips the early-exit token so a chatty Claude doesn't keep
/// iterating once the user can see/hear results.
fn dispatch_action(action: crate::providers::claude::Action, early_exit: &CancellationToken) {
    use crate::providers::claude::Action;
    if !matches!(action, Action::Integration) {
        early_exit.cancel();
    }
    match action {
        Action::Point { x: px, y: py } => {
            set_cursor_idle();
            // Offset: +10 Y to better center on UI elements
            ai_cursor::point_at(px as i32, (py + 10) as i32);
        }
        Action::Click { x: px, y: py } => {
            set_cursor_idle();
            // Offset: +10 Y to better center on UI elements
            let adjusted_y = py + 10;
            ai_cursor::point_at(px as i32, adjusted_y as i32);
            crate::actions::click_at(px, adjusted_y);
        }
        Action::Type { text } => crate::actions::type_text(&text),
        Action::Key { key } => crate::actions::press_key(&key),
        Action::Scroll { direction, amount } => crate::actions::scroll(&direction, amount),
        Action::OpenUrl { url } => {
            set_cursor_idle();
            crate::desktop::open_url(&url);
        }
        Action::LaunchApp { app } => {
            set_cursor_idle();
            crate::desktop::launch_app(&app);
        }
        Action::SwitchToWindow { target } => {
            set_cursor_idle();
            crate::desktop::switch_to_window(&target);
        }
        Action::Integration => {}
    }
}

/// Per-turn text-delta accumulator. Buffers Claude's streamed deltas,
/// finds sentence boundaries, and pushes complete sentences into the TTS
/// channel. First flush is permissive (clause-level breaks count) for
/// faster first-audio; subsequent flushes are strict on `.!?` to keep
/// prosody natural.
struct StreamHelper {
    buf: String,
    tx: tokio::sync::mpsc::UnboundedSender<String>,
    release_t: std::time::Instant,
    first_response_logged: bool,
    first_sentence_logged: bool,
}

impl StreamHelper {
    fn new(tx: tokio::sync::mpsc::UnboundedSender<String>, release_t: std::time::Instant) -> Self {
        Self {
            buf: String::new(),
            tx,
            release_t,
            first_response_logged: false,
            first_sentence_logged: false,
        }
    }

    /// Append a streamed text fragment from Claude and emit any complete
    /// sentences to the TTS channel. Logs first-response and
    /// first-sentence timestamps the first time each fires.
    fn push_delta(&mut self, delta: &str) {
        if !self.first_response_logged {
            eprintln!(
                "[timing] claude first response → {:?}",
                self.release_t.elapsed()
            );
            self.first_response_logged = true;
        }
        self.buf.push_str(delta);
        loop {
            let end_opt = if self.first_sentence_logged {
                find_sentence_end(&self.buf)
            } else {
                find_first_flush_point(&self.buf)
            };
            let Some(end) = end_opt else { break };
            let sentence: String = self.buf.drain(..=end).collect();
            if !self.first_sentence_logged {
                eprintln!(
                    "[timing] first sentence → tts → {:?}",
                    self.release_t.elapsed()
                );
                self.first_sentence_logged = true;
            }
            let _ = self.tx.send(sentence);
        }
    }

    /// Send any leftover buffered text after Claude's stream ends. Used
    /// when the final assistant message doesn't end in `.!?`.
    fn flush_tail(&mut self) {
        let tail = self.buf.trim().to_string();
        if !tail.is_empty() {
            if !self.first_sentence_logged {
                eprintln!(
                    "[timing] first sentence → tts → {:?}",
                    self.release_t.elapsed()
                );
                self.first_sentence_logged = true;
            }
            let _ = self.tx.send(tail);
            self.buf.clear();
        }
    }
}

/// Speak a short error message and return cleanly. Used when the
/// classifier can't pick an intent, so the user hears something instead
/// of silence.
fn speak_error(
    rt: &tokio::runtime::Runtime,
    cartesia: &TtsCartesia,
    audio_out: &audio::AudioOutput,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("[orchestrator] speaking error: {message}");
    println!("aegis: {message}");
    let player = audio_out.new_player();
    let chan_nz = audio_out.channels;
    let sr_nz = audio_out.sample_rate;
    let cartesia = cartesia.clone();
    let msg = message.to_string();
    rt.block_on(async move {
        let _ = cartesia
            .synthesize_stream(&msg, |pcm_bytes| {
                let samples: Vec<f32> = pcm_bytes
                    .chunks_exact(2)
                    .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / i16::MAX as f32)
                    .collect();
                player.append(rodio::buffer::SamplesBuffer::new(chan_nz, sr_nz, samples));
            })
            .await;
        // Brief drain so the speaker actually plays the message before we
        // return to the loop and the player gets dropped.
        let drain_start = std::time::Instant::now();
        while !player.empty() && drain_start.elapsed() < Duration::from_secs(3) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    Ok(())
}

/// Find the byte index of the first sentence-ending punctuation followed by
/// whitespace or end-of-buffer. Only matches ASCII `.`, `!`, `?` which are
/// safe to slice on in UTF-8.
fn find_sentence_end(buf: &str) -> Option<usize> {
    let bytes = buf.as_bytes();
    for i in 0..bytes.len() {
        if matches!(bytes[i], b'.' | b'!' | b'?')
            && (i + 1 == bytes.len() || matches!(bytes[i + 1], b' ' | b'\n' | b'\t'))
        {
            return Some(i);
        }
    }
    None
}

/// Permissive boundary finder used only for the FIRST flush of a turn.
/// Accepts strong sentence ends (`.`, `!`, `?`) AND clause-level breaks
/// (`,`, `;`, `:`) once at least MIN bytes have accumulated.
fn find_first_flush_point(buf: &str) -> Option<usize> {
    use crate::tuning::TTS_FIRST_FLUSH_MIN_CHARS as MIN_LEN;
    let bytes = buf.as_bytes();
    for i in 0..bytes.len() {
        if matches!(bytes[i], b'.' | b'!' | b'?')
            && (i + 1 == bytes.len() || matches!(bytes[i + 1], b' ' | b'\n' | b'\t'))
        {
            return Some(i);
        }
        if i >= MIN_LEN
            && matches!(bytes[i], b',' | b';' | b':')
            && i + 1 < bytes.len()
            && matches!(bytes[i + 1], b' ' | b'\n' | b'\t')
        {
            return Some(i);
        }
    }
    None
}
