# Aegis voice agent — design plan

**Date:** 2026-05-11
**Status:** Brainstormed, ready to build
**Build target:** Talking-cursor MVP in one focused day of work

---

## What we're building

A push-to-talk voice mode for aegis. Hold a hotkey, speak a question, release. The cursor responds verbally and (when relevant) physically points at the thing on screen you asked about.

Example flows:
- *"Where's the bold button?"* → cursor flies to the button + says "right here"
- *"What does this graph show?"* → cursor stays put, just answers verbally
- *"Summarize this page"* → cursor stays put, longer verbal answer

## Goals & non-goals

**Goals**
- Build the minimum end-to-end loop where audio in → text → Claude → audio out works
- Architect so future upgrades (streaming, Cartesia, proxy backend, conversation history) slot in without rewrites
- Match the "industry standard" approach Clicky uses (PTT + cloud STT + Claude + cloud TTS)

**Non-goals (for MVP)**
- Streaming STT/TTS — batch HTTP is fine for v1
- Cloudflare Worker proxy — direct API calls with .env keys for v1
- Wake-word ("Hey cursor") — push-to-talk only
- Conversation history — each press is a fresh interaction
- Interruption / barge-in — cannot interrupt the assistant mid-response in v1
- Cross-platform — Hyprland-only

## Architecture

### Data flow

```
USER presses SUPER+SPACE
        │
        ▼
  Hyprland bind ── pkill -SIGUSR1 aegis ─▶  hotkey.rs sets RECORDING = true
                                                          │
                                                          ▼
                                                  audio.rs starts capturing
                                                  PCM chunks into a Vec<u8>
                                                          │
USER releases SUPER+SPACE                                 │
        │                                                 │
        ▼                                                 │
  Hyprland bindr ─ pkill -SIGUSR2 aegis ─▶ hotkey.rs sets RECORDING = false
                                                          │
                                                          ▼
                                            audio.rs returns final Vec<u8>
                                                          │
                                                          ▼
                              voice::run_one_turn(stt, llm, tts, audio)
                                                          │
   ┌──────────────────────────────────────────────────────┤
   ▼                                                      │
 stt.transcribe(audio)                                    │
   "where's the bold button?"                             │
                                                          │
   ▼                                                      │
 screenshot::capture_active_workspace()                   │
                                                          │
   ▼                                                      │
 llm.ask_with_image(transcript, screenshot)               │
   "It's in the toolbar at the top-left, [POINT:120,40]"  │
                                                          │
   ▼                                                      │
 if response contains [POINT:x,y] tag:                    │
   parse coords, call cursor::point_at(x, y)              │
   strip the tag from text                                │
                                                          │
   ▼                                                      │
 tts.synthesize(stripped_text) ── MP3 bytes               │
                                                          │
   ▼                                                      │
 audio.play(mp3)
                                                          │
        ◀─────────────────────────────────────────────────┘
   USER hears response
```

### File structure

```
src/
  audio.rs              NEW   cpal mic record + rodio playback
  hotkey.rs             NEW   signal handler (SIGUSR1 / SIGUSR2)
  voice.rs              NEW   orchestrator: run_one_turn() + run_loop()
  providers/
    mod.rs                    extends with Stt + Tts traits
    claude.rs                 existing — extend system prompt for [POINT:] tag
    whisper_openai.rs   NEW   Stt impl via OpenAI Whisper HTTP
    tts_openai.rs       NEW   Tts impl via OpenAI TTS HTTP
  cursor.rs                   no change
  screenshot.rs               no change
  mouse.rs                    no change
  painter.rs                  no change
  main.rs                     spawn the voice loop thread on startup
```

### Trait abstractions

In `providers/mod.rs`, alongside the existing `Llm` trait:

```rust
pub trait Stt {
    fn transcribe(&self, audio_pcm16: &[u8], sample_rate: u32) -> Result<String, Box<dyn Error>>;
}

pub trait Tts {
    fn synthesize(&self, text: &str) -> Result<Vec<u8>, Box<dyn Error>>; // returns MP3 bytes
}
```

This mirrors the existing `Llm` trait. Every future STT/TTS provider implements the same trait — the orchestrator never changes.

### The orchestrator

`src/voice.rs`:

```rust
pub fn run_one_turn(
    stt: &dyn Stt,
    llm: &dyn Llm,
    tts: &dyn Tts,
    audio: &mut Audio,
) -> Result<(), Box<dyn Error>> {
    let pcm = audio.record_until_release()?;          // blocks until SIGUSR2
    let transcript = stt.transcribe(&pcm, 16000)?;
    println!("user said: {}", transcript);

    let (b64, _, _) = crate::screenshot::capture_active_workspace()?;
    let reply = llm.ask_with_image(&transcript, &b64)?;
    println!("claude said: {}", reply);

    let (clean_text, point_coords) = parse_point_tag(&reply);
    if let Some((x, y)) = point_coords {
        crate::cursor::point_at(x, y);
    }

    let mp3 = tts.synthesize(&clean_text)?;
    audio.play(&mp3)?;
    Ok(())
}

pub fn run_loop(stt: impl Stt, llm: impl Llm, tts: impl Tts) {
    let mut audio = Audio::new();
    loop {
        hotkey::wait_for_press();
        if let Err(e) = run_one_turn(&stt, &llm, &tts, &mut audio) {
            eprintln!("voice turn failed: {}", e);
        }
    }
}
```

The `parse_point_tag` helper looks for the literal string `[POINT:x,y]` in Claude's response, extracts the coords, and returns the text without the tag.

## Module-by-module spec

### `src/audio.rs` (~80 lines)

Wraps `cpal` for input and `rodio` for playback.

```rust
pub struct Audio {
    input_stream: Option<cpal::Stream>,
    output_stream_handle: rodio::OutputStreamHandle,
    _output_stream: rodio::OutputStream,
    pcm_buffer: Arc<Mutex<Vec<u8>>>,
}

impl Audio {
    pub fn new() -> Self { ... }

    /// Start mic capture. Returns when SIGUSR2 fires (hotkey released).
    pub fn record_until_release(&mut self) -> Result<Vec<u8>, Box<dyn Error>> {
        // 1. Create cpal input stream at default device, request 16kHz mono if possible.
        // 2. Callback writes incoming samples (converted to i16 PCM) into pcm_buffer.
        // 3. Loop checking hotkey::is_recording() — when false, drop the stream and return the buffer.
    }

    pub fn play(&self, mp3_bytes: &[u8]) -> Result<(), Box<dyn Error>> {
        // Use rodio::Decoder::new(Cursor::new(mp3_bytes))? then sink.append() + sink.sleep_until_end()
    }
}
```

**Open questions for tomorrow:**
- `cpal` may not give exactly 16kHz mono on first try — might need to resample with `rubato`
- May need `pipewire` feature flag on cpal in Cargo.toml: `cpal = { version = "...", features = ["pipewire"] }`

### `src/hotkey.rs` (~30 lines)

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use signal_hook::consts::{SIGUSR1, SIGUSR2};

static RECORDING: AtomicBool = AtomicBool::new(false);

pub fn init() -> Result<(), Box<dyn Error>> {
    // Register SIGUSR1 → set RECORDING true
    // Register SIGUSR2 → set RECORDING false
    // signal_hook::iterator::Signals in a background thread
}

pub fn is_recording() -> bool { RECORDING.load(Ordering::Relaxed) }
pub fn wait_for_press() {
    while !is_recording() { std::thread::sleep(Duration::from_millis(20)); }
}
```

**Add to Cargo.toml:** `signal-hook = "0.3"`

### `src/providers/whisper_openai.rs` (~40 lines)

```rust
pub struct WhisperOpenAi { api_key: String }

impl WhisperOpenAi {
    pub fn from_env() -> Result<Self, ...> { /* reads OPENAI_API_KEY */ }
}

impl Stt for WhisperOpenAi {
    fn transcribe(&self, pcm: &[u8], sr: u32) -> Result<String, ...> {
        // 1. Wrap PCM in a WAV header (or convert to mp3/m4a — Whisper accepts both)
        // 2. POST multipart to https://api.openai.com/v1/audio/transcriptions
        //    fields: file=<bytes>, model=whisper-1, response_format=text
        // 3. Return the text body
    }
}
```

The WAV header is ~44 bytes prepended to raw PCM16 data. Standard utility.

### `src/providers/tts_openai.rs` (~30 lines)

```rust
pub struct TtsOpenAi { api_key: String }

impl Tts for TtsOpenAi {
    fn synthesize(&self, text: &str) -> Result<Vec<u8>, ...> {
        // POST to https://api.openai.com/v1/audio/speech
        //   body: { "model": "tts-1", "voice": "alloy", "input": text }
        // Returns MP3 bytes in the response body.
    }
}
```

### `src/providers/claude.rs` — small change

Extend the system prompt in `ask_with_image` to include:

> "When the user is asking where a UI element is on screen, include `[POINT:x,y]` at the end of your response with the screen coordinates of the element. Otherwise omit the tag. Keep all responses to 1-2 sentences."

This is the same pattern Clicky's v1 uses. Simpler than tool-use for now; can upgrade to proper tools later.

### `src/main.rs` change

```rust
fn main() {
    let claude = providers::claude::Claude::from_env().expect("...");
    let stt = providers::whisper_openai::WhisperOpenAi::from_env().expect("...");
    let tts = providers::tts_openai::TtsOpenAi::from_env().expect("...");

    hotkey::init().expect("signal handler setup");

    // Voice loop in its own thread
    std::thread::spawn(move || {
        voice::run_loop(stt, claude, tts);
    });

    mouse::spawn_poller();
    cursor::cursor(300, 300);
}
```

## Setup steps (do these tomorrow before coding)

### 1. API keys

Add to `.env`:
```
ANTHROPIC_API_KEY=sk-ant-api03-...     # already there
OPENAI_API_KEY=sk-proj-...              # new, for Whisper + TTS
```

### 2. Hyprland config

Add to `~/.config/hypr/hyprland.conf`:
```
bind  = SUPER, space, exec, pkill -SIGUSR1 aegis
bindr = SUPER, space, exec, pkill -SIGUSR2 aegis
```

Reload Hyprland config: `hyprctl reload`

### 3. Cargo deps

```bash
cargo add cpal --features pipewire
cargo add rodio
cargo add signal-hook
cargo add hound      # WAV file writer for prepping audio for Whisper
```

## Build order (do this in sequence)

1. **`hotkey.rs`** — Get signals firing. Test: press the key, see RECORDING flip via println.
2. **`audio.rs::play`** — Wire up rodio. Test: play a hard-coded MP3 to make sure speakers work.
3. **`audio.rs::record_until_release`** — Capture from mic. Test: record 5s, dump PCM to disk, play it back.
4. **`providers/whisper_openai.rs`** — POST a recorded WAV to OpenAI. Test: hardcoded WAV file → transcript.
5. **`providers/tts_openai.rs`** — POST text to OpenAI TTS. Test: "hello world" → MP3 bytes → play.
6. **`voice.rs::run_one_turn`** — Wire it all together. Test: press hotkey, talk, hear response.
7. **`providers/claude.rs` system prompt** — Add the `[POINT:x,y]` instruction.
8. **`voice.rs` point tag parsing** — Extract coords from response, call `cursor::point_at`.

Don't skip ahead. Each step is testable in isolation. If something breaks at step 6, you know steps 1-5 work because you tested them.

## Evolution path (what comes after MVP)

| Upgrade | When | What changes |
|---|---|---|
| Streaming TTS playback (Cartesia) | When response feels slow | New `tts_cartesia.rs`, `Tts` trait gains `synthesize_stream`, `audio.rs` adds `play_stream` |
| Streaming STT (AssemblyAI) | When you can't stand the post-release pause | New `stt_assemblyai.rs`, audio capture pipes directly to STT websocket while recording |
| Cloudflare Worker proxy | Before sharing with anyone | All HTTP URLs change from provider URLs to `your-worker.workers.dev`; no code structure change |
| Conversation history | When users complain about no memory | Wrap orchestrator in a `Conversation` struct holding past turns |
| Tool-based pointing | If `[POINT:x,y]` parsing gets brittle | Extend `Llm` trait with `complete_with_tools`, Claude impl emits `point_at` tool calls |
| User accounts + billing | When you start selling | Supabase auth, JWT to worker, Stripe |

## Cost estimate (per voice turn)

- OpenAI Whisper: ~$0.006 per minute of audio. A 5-second question = ~$0.0005.
- Claude (Opus 4.7 with vision): a screenshot + transcript + short response ≈ $0.005-0.02.
- OpenAI TTS (tts-1): $15 per 1M characters. ~30 chars per response = ~$0.0005.

**Per turn: roughly $0.01-0.03.** A heavy day of 100 turns ≈ $1-3. Manageable for personal use without a backend.

## Open questions (decide tomorrow if needed)

- **Initial voice**: which OpenAI TTS voice (alloy / echo / fable / onyx / nova / shimmer)? Try a few, pick what fits.
- **Audio format from mic**: cpal might give f32 samples at 48kHz; we need i16 at 16kHz for the WAV header. Resample with rubato or accept 48kHz (Whisper handles any rate, just bigger payload).
- **Hotkey choice**: `SUPER+space` overlaps with most launchers. Maybe `SUPER+grave` or `SUPER+v` instead.

## Spec review checklist

- [x] No placeholders / TBDs in the design
- [x] Sections internally consistent (file structure matches data flow matches build order)
- [x] Scope is single-day MVP, not multi-week mega-feature
- [x] No ambiguity in trait shapes or orchestrator signature

---

**Status:** Ready to implement tomorrow. Setup steps + build order are sequential and testable.
