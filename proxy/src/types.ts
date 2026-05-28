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
     *   usage:trial:{deviceId}            -> TurnUsage (30d TTL, refreshed on use)
     *   usage:demo:{code}:{deviceId}      -> TurnUsage (30d TTL, refreshed on use)
     *   invite:{CODE}                     -> InviteCode (no TTL, managed by hand)
     */
    USAGE_KV: KVNamespace;
    /**
     * Object store for routelet distillation samples. One object per sample:
     *   samples/{date}/{deviceId}/{ts}-{uuid}.json -> RouteletSample
     * Opt-in on the client; the bucket only ever sees redacted text.
     */
    ROUTELET_R2: R2Bucket;
    /** Lifetime call cap for trial-tier devices. Decimal string. One voice query = 3 calls (STT, Claude, TTS). */
    TRIAL_TURNS_CAP: string;
}

/** Lifetime call counter, shared by both tiers. One per (tier, device). */
export type TurnUsage = {
    /** Metered calls this device has made (STT/Claude/TTS each count one). */
    turns: number;
};

export type InviteCode = {
    /** Lifetime call cap for any device using this code. 10 uses = 30 calls. */
    turns_cap: number;
    /** Hard ceiling on `devices_seen.length`. */
    max_devices: number;
    /** Device IDs that have used this code. Append-only. */
    devices_seen: string[];
};

export type Tier =
    | { kind: "trial"; turnsCap: number }
    | { kind: "demo"; code: string; turnsCap: number };

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
