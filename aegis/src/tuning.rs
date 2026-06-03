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
pub const AUDIO_POST_RELEASE_GRACE_MS: u64 = 200;

// ────── STT ──────

/// After Deepgram sends a non-empty FINAL, wait this long for any
/// additional FINALs before returning.
/// ↑ catches multi-segment utterances (pauses, "Hello. My name is X").
/// ↓ faster transcript return. risks truncating split utterances.
pub const STT_QUIESCENCE_MS: u64 = 150;

// ────── TTS first-flush ──────

/// Min chars before the eager flush accepts a comma/semicolon/colon
/// as a flush point (instead of waiting for . ! ?).
/// ↑ only flushes on longer opening clauses (smoother prosody).
/// ↓ catches shorter clauses like "Hi there," (faster first audio).
pub const TTS_FIRST_FLUSH_MIN_CHARS: usize = 12;

// ────── routelet classifier ──────

/// Minimum routelet confidence required to accept its prediction on-device.
/// Below this the turn falls back to the Claude classifier for a second opinion.
///
/// Set high on purpose. routelet's max-softmax saturates near 0.98 for almost
/// everything, including garbled/out-of-distribution input, so a low gate never
/// fired (see the deferral analysis in routelet/report). At 0.55 it deferred ~0%
/// of OOD probes; at 0.95 it catches a meaningful share of them while deferring
/// only ~1 to 2% of real in-distribution commands.
/// ↑ defers more turns to Claude (catches more OOD, adds latency and cost).
/// ↓ keeps more turns on-device (faster, but the gate stops catching OOD).
pub const ROUTELET_CONFIDENCE_THRESHOLD: f32 = 0.95;

/// Max distillation samples drained and POSTed in one uploader wakeup.
/// ↑ fewer wakeups under bursty use. larger transient batch if the proxy is slow.
/// ↓ steadier trickle of small requests. more task wakeups.
pub const ROUTELET_UPLOAD_BATCH_MAX: usize = 32;

/// Seconds to retire a proxy-minted STT/TTS token before its real TTL, so a
/// turn never opens a stream with a token that expires mid-flight. The proxy
/// mints 3600s tokens, so the cache effectively lasts a session minus this.
/// ↑ re-mints sooner (safer against clock skew and long turns, more mints).
/// ↓ squeezes more reuse out of each token (fewer mints, tighter expiry race).
pub const PROXY_TOKEN_REFRESH_MARGIN_SECS: u64 = 120;

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

// ────── Working context (Tier 0) ──────

/// Recent voice turns kept verbatim in the live working context and replayed
/// into chat/agent requests.
/// ↑ better in-conversation recall. more tokens + latency per turn.
/// ↓ leaner requests. shorter conversational memory.
pub const WORKING_CONTEXT_RECENT_TURNS: usize = 6;

/// Turn count that triggers compaction: once `recent` exceeds this, the turns
/// older than RECENT_TURNS are folded into the running summary off the hot
/// path. Kept above RECENT_TURNS for hysteresis, so it does not compact every
/// turn.
/// ↑ compact less often. larger peak request before it kicks in.
/// ↓ compact sooner. more frequent summarizer calls.
pub const WORKING_CONTEXT_COMPACT_AT: usize = 10;
