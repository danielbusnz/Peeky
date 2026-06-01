// Compile-time constants: validation patterns, upstream URLs, and TTLs.

export const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
// The five intent labels, identical on both sides (Rust Intent::as_str and the
// routelet head.json labels). Anything else is rejected.
export const INTENT_LABELS = ["find_action", "integration", "chat", "memory", "agent"];
// Cap stored text so a misbehaving client can't write huge objects.
export const SAMPLE_MAX_TEXT_CHARS = 2000;
export const CODE_RE = /^[A-Z0-9][A-Z0-9-]{6,62}[A-Z0-9]$/;

export const ANTHROPIC_URL = "https://api.anthropic.com/v1/messages";
export const DEEPGRAM_TOKEN_URL = "https://api.deepgram.com/v1/auth/grant";
export const CARTESIA_TOKEN_URL = "https://api.cartesia.ai/access-token";
export const CARTESIA_API_VERSION = "2026-03-01";

// Daily usage entries are keyed by UTC date and only need to outlive the day
// they track, plus slack for clock skew at the boundary. After that the entry
// expires on its own, which is the daily reset.
export const DAILY_USAGE_TTL_SECONDS = 2 * 24 * 60 * 60;

// Default per-day budget for trial devices (no invite code). Mirrors the
// invite-record fields so trial and demo run on one metering model.
export const TRIAL_DAILY_BUDGET = {
    input_tokens: 60_000,
    output_tokens: 6_000,
    deepgram: 10,
    cartesia: 10,
};

// Per-turn token estimates charged against the Anthropic budget. We charge a
// flat estimate rather than parsing real usage from the stream (simpler, at the
// cost of some drift). Sized so the sample demo budget (600k in / 60k out) is
// roughly 100 turns/day. Switch to parsing the SSE usage if drift matters.
export const EST_INPUT_TOKENS_PER_TURN = 6_000;
export const EST_OUTPUT_TOKENS_PER_TURN = 600;

// How long upstream tokens live. The client caches and reuses a token for its
// lifetime (minting at startup, refreshing before expiry), so one token covers
// a session instead of one per turn/sentence. 3600s is each provider's max.
// These are narrowly scoped (STT-only / TTS-only) client tokens, so an hour is
// an acceptable blast radius if one leaks.
export const DEEPGRAM_TOKEN_TTL_SECONDS = 3600;
export const CARTESIA_TOKEN_TTL_SECONDS = 3600;
