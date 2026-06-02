// Shared types for the aegis proxy Worker. The runtime bindings (Env) and the
// shapes persisted to KV and R2 live here so handlers and the metering layer
// agree on one definition.

export interface Env {
    /** Anthropic API key. Set via `wrangler secret put ANTHROPIC_API_KEY`. */
    ANTHROPIC_API_KEY: string;
    /** Deepgram API key. Set via `wrangler secret put DEEPGRAM_API_KEY`. */
    DEEPGRAM_API_KEY: string;
    /** Cartesia API key. Set via `wrangler secret put CARTESIA_API_KEY`. */
    CARTESIA_API_KEY: string;
    /**
     * Shared namespace for usage counters and invite codes. Keys:
     *   usage:trial:{deviceId}:{utcDate}        -> DailyUsage (~2d TTL)
     *   usage:demo:{code}:{deviceId}:{utcDate}  -> DailyUsage (~2d TTL)
     *   invite:{CODE}                           -> InviteCode (no TTL, by hand)
     * Usage is per UTC day: the date in the key is the reset boundary, and the
     * short TTL lets yesterday's entry expire on its own.
     */
    USAGE_KV: KVNamespace;
    /**
     * Object store for routelet distillation samples. One object per sample:
     *   samples/{date}/{deviceId}/{ts}-{uuid}.json -> RouteletSample
     * Opt-in on the client; the bucket only ever sees redacted text.
     */
    ROUTELET_R2: R2Bucket;
    /**
     * Accounts database. Holds users (social identities), and later the synced
     * settings blob and subscription tier. Bound via [[d1_databases]] in
     * wrangler.toml. See migrations/.
     */
    DB: D1Database;
    /** GitHub OAuth app client id. `wrangler secret put GITHUB_CLIENT_ID`. */
    GITHUB_CLIENT_ID: string;
    /** GitHub OAuth app client secret. `wrangler secret put GITHUB_CLIENT_SECRET`. */
    GITHUB_CLIENT_SECRET: string;
    /** HS256 signing secret for aegis session JWTs. `wrangler secret put JWT_SECRET`. */
    JWT_SECRET: string;
}

/**
 * Per-day usage caps. Anthropic is metered in estimated tokens; Deepgram and
 * Cartesia in token-mint count (one mint covers a whole session thanks to
 * client-side caching).
 */
export type DailyBudget = {
    input_tokens: number;
    output_tokens: number;
    deepgram: number;
    cartesia: number;
};

/** Running usage for one (tier, device, UTC day). Same fields as DailyBudget. */
export type DailyUsage = {
    input_tokens: number;
    output_tokens: number;
    deepgram: number;
    cartesia: number;
};

export type InviteCode = {
    /** Per-day Anthropic input-token budget for any device using this code. */
    daily_input_tokens: number;
    /** Per-day Anthropic output-token budget. */
    daily_output_tokens: number;
    /** Per-day Deepgram token-mint budget (mints, not audio seconds). */
    daily_deepgram_tokens: number;
    /** Per-day Cartesia token-mint budget. */
    daily_cartesia_tokens: number;
    /** Hard ceiling on `devices_seen.length`. */
    max_devices: number;
    /** ISO 8601 instant after which the code is rejected. */
    expires_at: string;
    /** Device IDs that have used this code. Append-only. */
    devices_seen: string[];
};

export type Tier =
    | { kind: "trial"; budget: DailyBudget }
    | { kind: "demo"; code: string; budget: DailyBudget };

/** Successful read-only resolution of an invite code against KV. */
export type InviteLookup = {
    normalized: string;
    invite: InviteCode;
    /** Whether this device is already bound to the code. */
    deviceKnown: boolean;
    /** Whether the code has an unused device slot (ignoring `deviceKnown`). */
    hasRoom: boolean;
};

/** One distillation sample as stored in R2. */
export type RouteletSample = {
    /** Redacted on-device, scrubbed again here. Maps to the `text` field the routelet trainer reads. */
    text: string;
    /** What routelet predicted on-device, or null if it abstained. */
    routelet_pred: string | null;
    /** Routelet softmax confidence in [0,1], or null when it abstained. */
    routelet_conf: number | null;
    /**
     * Ground-truth label. Reserved for the server-attached Claude label once
     * the fallback path feeds this endpoint; null until then.
     */
    claude_label: string | null;
    /** The `x-aegis-device-id` that produced the sample. */
    device: string;
    /** Unix seconds, server clock (not the client's). */
    ts: number;
};
