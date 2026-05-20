//! Central registry of tunable knobs. Change a number, recompile, see
//! the effect on latency or correctness. Structural constants (URLs,
//! model IDs, header names) live near their code; only behavior dials
//! live here. All times in milliseconds unless noted.

// ────── audio capture ──────

/// Audio captured before press is buffered in this ring; flushed on
/// press so the first syllable isn't missed.
/// ↑ catches more leading audio (good for fast talkers). costs memory.
/// ↓ less leading audio captured. 0 = nothing buffered before press.
pub const AUDIO_PREROLL_MS: u64 = 0;

/// How long to keep forwarding audio to Deepgram after release.
/// ↑ more reliable last-syllable capture. adds latency.
/// ↓ faster EOS to Deepgram. risks clipping the final word.
pub const AUDIO_POST_RELEASE_GRACE_MS: u64 = 800;

// ────── STT ──────

/// After Deepgram sends a non-empty FINAL, wait this long for any
/// additional FINALs before returning.
/// ↑ catches multi-segment utterances (pauses, "Hello. My name is X").
/// ↓ faster transcript return. risks truncating split utterances.
pub const STT_QUIESCENCE_MS: u64 = 0;

// ────── TTS first-flush ──────

/// Min chars before the eager flush accepts a comma/semicolon/colon
/// as a flush point (instead of waiting for . ! ?).
/// ↑ only flushes on longer opening clauses (smoother prosody).
/// ↓ catches shorter clauses like "Hi there," (faster first audio).
pub const TTS_FIRST_FLUSH_MIN_CHARS: usize = 12;

// ────── Claude agent loop ──────

/// Hard cap on agent loop iterations per turn.
/// ↑ allows longer multi-step plans. risks runaway token burn.
/// ↓ tighter cost ceiling. may truncate legitimate chains.
pub const AGENT_MAX_STEPS: usize = 10;

/// Wait between firing a tool action and capturing the next screenshot.
/// Lets the UI repaint, animations settle.
/// ↑ more reliable screenshots after UI changes. step latency tax.
/// ↓ faster step-to-step. risks capturing pre-animation state.
pub const AGENT_SETTLE_MS: u64 = 600;

/// Max screenshots kept inline in messages history. Older ones get
/// their image bytes stripped.
/// ↑ more visual context across steps. bigger requests.
/// ↓ tighter request bodies. less long-range visual memory.
pub const AGENT_KEEP_RECENT_SCREENSHOTS: usize = 3;
