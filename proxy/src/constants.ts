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

// Usage counters soft-reset after 30 days of inactivity: each use refreshes
// the TTL, so an active device's cap holds while an idle one's expires.
export const TURN_KV_TTL_SECONDS = 30 * 24 * 60 * 60;

// How long upstream tokens live. Long enough for the client to open a WS,
// short enough that a stolen token is useless quickly.
export const DEEPGRAM_TOKEN_TTL_SECONDS = 60;
export const CARTESIA_TOKEN_TTL_SECONDS = 60;
