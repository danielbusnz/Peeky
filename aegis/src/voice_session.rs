//! One-time resources held across every voice turn. Owns the tokio runtime,
//! the persistent cpal mic stream, the audio output sink, and the provider
//! clients. Built once at startup by `VoiceSession::start`, then borrowed by each
//! turn. Must be constructed on the same thread that will drive the loop
//! because `cpal::Stream` is `!Send`.

use crate::audio;
use crate::providers::claude::{Claude, MemoryStore};
use crate::providers::stt_deepgram::SttDeepgram;
use crate::providers::tts_cartesia::TtsCartesia;
use crate::routelet::Routelet;

pub struct VoiceSession {
    pub rt: tokio::runtime::Runtime,
    pub mic: audio::LiveMic,
    pub audio_out: audio::AudioOutput,
    pub stt: SttDeepgram,
    pub claude: Claude,
    pub cartesia: TtsCartesia,
    pub memory: MemoryStore,
    pub routelet: Routelet,
}

impl VoiceSession {
    /// Build the per-process session: create the tokio runtime, warm HTTP
    /// pools to all three providers in parallel, start the persistent cpal
    /// mic stream, and open the audio output sink.
    pub fn start(
        mic: audio::Mic,
        stt: SttDeepgram,
        claude: Claude,
        cartesia: TtsCartesia,
        routelet: Routelet,
    ) -> Self {
        // Tokio runtime owned by this thread. Streaming providers (Deepgram WS,
        // Claude SSE, Cartesia SSE) all run via `rt.block_on(...)`.
        let rt = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");

        // Pre-open HTTPS pools to Claude, Deepgram, and Cartesia so the first
        // voice turn doesn't pay TCP+TLS handshake cost. Each warm() fires a
        // fast-failing request on this runtime; the connection stays in the
        // pool for the real call.
        let t_warm = std::time::Instant::now();
        rt.block_on(async {
            let _ = tokio::join!(claude.warm(), stt.warm(), cartesia.warm());
        });
        eprintln!("[warmup] HTTP pools primed in {:?}", t_warm.elapsed());

        // Warm up cpal once on this thread (cpal::Stream is !Send). The stream
        // runs forever; per-turn we just install a sender to start forwarding.
        let running_mic = mic.start();

        // Open the audio output sink ONCE at startup. Per-turn we just hand
        // out a fresh Player against this sink (~free).
        let audio_out = audio::AudioOutput::init().expect("could not open audio output");

        // Load persistent memory facts (name, preferences, etc.). Empty
        // store on first launch; subsequent launches re-load from
        // ~/.config/aegis/memory.jsonl.
        let memory = MemoryStore::open_default().expect("could not open aegis memory store");

        VoiceSession {
            rt,
            mic: running_mic,
            audio_out,
            stt,
            claude,
            cartesia,
            memory,
            routelet,
        }
    }
}
