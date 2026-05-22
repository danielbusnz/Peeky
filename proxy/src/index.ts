// aegis-proxy: Cloudflare Worker that fronts Anthropic, Deepgram, and Cartesia
// for the aegis desktop client.
//
// Why it exists:
//   The desktop app ships without API keys so non-technical users can install
//   it and have it just work. The Worker holds all three secret keys, caps
//   per-device usage from a KV store, and streams/forwards responses.
//
// Two tiers, selected per request:
//   trial — no invite code. Lifetime turn counter, capped by TRIAL_TURNS_CAP.
//           Default 18 turns = 6 voice queries at 3 calls/query (STT, Claude,
//           TTS). Cap is per-device, soft-resets after 30 days of inactivity
//           when the KV entry expires.
//   demo  — request carries `x-aegis-invite-code`. Code's KV payload supplies
//           per-day token caps and a max-devices binding. Used for recruiter
//           demos and anyone we hand-grant extended access.
//
// Routes:
//   POST /v1/anthropic/messages   HTTP SSE proxy to Claude Messages API
//   POST /v1/deepgram/token       mint short-lived Deepgram JWT for STT WS
//   POST /v1/cartesia/token       mint short-lived Cartesia access token for TTS WS
//
// Why mixed patterns:
//   Anthropic is HTTP request/response with streaming SSE. We forward bytes
//   through and parse the stream for token accounting. Cheap.
//
//   Deepgram + Cartesia are WebSocket-only for streaming. Proxying WebSockets
//   through Workers is hairy (bidirectional forwarding, mid-stream cap
//   enforcement, idle timeouts). Instead, both providers offer short-lived
//   token APIs intended exactly for this client-direct-to-provider pattern.
//   The Worker just mints a token; the client connects directly. ~20 lines per
//   provider, no per-message proxying.

export interface Env {
    /** Anthropic API key. Set via `wrangler secret put ANTHROPIC_API_KEY`. */
    ANTHROPIC_API_KEY: string;
    /** Deepgram API key. Set via `wrangler secret put DEEPGRAM_API_KEY`. */
    DEEPGRAM_API_KEY: string;
    /** Cartesia API key. Set via `wrangler secret put CARTESIA_API_KEY`. */
    CARTESIA_API_KEY: string;
    /**
     * Shared namespace for both usage counters and invite codes. Keys:
     *   usage:trial:{deviceId}                  -> TrialUsage (30d TTL)
     *   usage:demo:{code}:{deviceId}:{date}     -> Usage (48h TTL)
     *   invite:{CODE}                           -> InviteCode (no TTL, managed by hand)
     */
    USAGE_KV: KVNamespace;
    /** Lifetime turn cap for trial-tier devices. Decimal string. */
    TRIAL_TURNS_CAP: string;
    /** Daily caps for demo-tier devices when the invite code omits a field. */
    DAILY_INPUT_TOKEN_CAP: string;
    DAILY_OUTPUT_TOKEN_CAP: string;
    DAILY_DEEPGRAM_TOKEN_CAP: string;
    DAILY_CARTESIA_TOKEN_CAP: string;
}

type Usage = {
    /** Anthropic input tokens consumed today. */
    input: number;
    /** Anthropic output tokens consumed today. */
    output: number;
    /** Deepgram tokens minted today (each gates one streaming session). */
    deepgram_tokens: number;
    /** Cartesia tokens minted today (each gates one or more TTS sessions). */
    cartesia_tokens: number;
};

type TrialUsage = {
    /** Any-endpoint calls this device has made. Compared to TRIAL_TURNS_CAP. */
    turns: number;
};

type DemoCaps = {
    daily_input_tokens: number;
    daily_output_tokens: number;
    daily_deepgram_tokens: number;
    daily_cartesia_tokens: number;
};

type InviteCode = DemoCaps & {
    /** ISO 8601. Codes past this date are rejected. */
    expires_at: string;
    /** Hard ceiling on `devices_seen.length`. */
    max_devices: number;
    /** Device IDs that have used this code. Append-only. */
    devices_seen: string[];
};

type Tier =
    | { kind: "trial"; turnsCap: number }
    | { kind: "demo"; code: string; caps: DemoCaps };

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const CODE_RE = /^[A-Z0-9][A-Z0-9-]{6,62}[A-Z0-9]$/;
const ANTHROPIC_URL = "https://api.anthropic.com/v1/messages";
const DEEPGRAM_TOKEN_URL = "https://api.deepgram.com/v1/auth/grant";
const CARTESIA_TOKEN_URL = "https://api.cartesia.ai/access-token";
const CARTESIA_API_VERSION = "2026-03-01";
const DEMO_KV_TTL_SECONDS = 48 * 60 * 60;
const TRIAL_KV_TTL_SECONDS = 30 * 24 * 60 * 60;

// How long upstream tokens live. Long enough for the client to open a WS,
// short enough that a stolen token is useless quickly.
const DEEPGRAM_TOKEN_TTL_SECONDS = 60;
const CARTESIA_TOKEN_TTL_SECONDS = 60;

export default {
    async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
        const url = new URL(request.url);

        if (request.method === "OPTIONS") return cors(new Response(null, { status: 204 }));

        if (request.method === "POST") {
            if (url.pathname === "/v1/anthropic/messages") {
                return handleAnthropic(request, env, ctx);
            }
            if (url.pathname === "/v1/deepgram/token") {
                return handleDeepgramToken(request, env, ctx);
            }
            if (url.pathname === "/v1/cartesia/token") {
                return handleCartesiaToken(request, env, ctx);
            }
        }

        return cors(new Response("Not found", { status: 404 }));
    },
} satisfies ExportedHandler<Env>;

// ────────────────────────────────────────────────────────────────────────────
// Route handlers
// ────────────────────────────────────────────────────────────────────────────

/**
 * Full HTTP proxy for Anthropic's Messages API. Trial tier bumps the turn
 * counter pre-flight; demo tier tallies token usage from a teed SSE copy.
 */
async function handleAnthropic(
    request: Request,
    env: Env,
    ctx: ExecutionContext,
): Promise<Response> {
    const deviceId = requireDeviceId(request);
    if (deviceId instanceof Response) return deviceId;

    const tier = await resolveTier(request, env, deviceId);
    if (tier instanceof Response) return tier;

    const rawBody = await request.text();
    let parsed: { stream?: boolean };
    try {
        parsed = JSON.parse(rawBody);
    } catch {
        return cors(jsonResponse(400, { error: "request body must be JSON" }));
    }
    if (parsed.stream !== true) {
        return cors(jsonResponse(400, { error: "stream: true required" }));
    }

    // Pre-flight cap check. Trial pays a turn up front; demo pays tokens after
    // the SSE stream reports usage.
    if (tier.kind === "trial") {
        const consumed = await consumeTrialTurn(env.USAGE_KV, deviceId, tier.turnsCap);
        if (!consumed) {
            return cors(
                jsonResponse(429, {
                    error: "trial_exhausted",
                    message:
                        "Free trial spent. Use your own API keys (BYOK) or contact us for an invite code.",
                    provider: "anthropic",
                    tier: "trial",
                }),
            );
        }
    } else {
        const kvKey = demoUsageKey(tier.code, deviceId);
        const usage = await readUsage(env.USAGE_KV, kvKey);
        if (
            usage.input >= tier.caps.daily_input_tokens ||
            usage.output >= tier.caps.daily_output_tokens
        ) {
            return cors(
                jsonResponse(429, {
                    error: "daily_cap_exceeded",
                    message: "Daily cap for this invite code reached. Try again tomorrow.",
                    provider: "anthropic",
                    tier: "demo",
                    usage,
                    caps: {
                        input: tier.caps.daily_input_tokens,
                        output: tier.caps.daily_output_tokens,
                    },
                }),
            );
        }
    }

    const upstreamHeaders: Record<string, string> = {
        "x-api-key": env.ANTHROPIC_API_KEY,
        "anthropic-version": request.headers.get("anthropic-version") ?? "2023-06-01",
        "content-type": "application/json",
    };
    const beta = request.headers.get("anthropic-beta");
    if (beta) upstreamHeaders["anthropic-beta"] = beta;

    const upstream = await fetch(ANTHROPIC_URL, {
        method: "POST",
        headers: upstreamHeaders,
        body: rawBody,
    });

    if (!upstream.ok || !upstream.body) {
        return cors(
            new Response(upstream.body, {
                status: upstream.status,
                headers: passthroughHeaders(upstream.headers),
            }),
        );
    }

    // Only demo tier needs token accounting. Trial already paid its turn.
    if (tier.kind === "demo") {
        const [toClient, toTally] = upstream.body.tee();
        ctx.waitUntil(
            tallyAnthropicUsage(toTally, env.USAGE_KV, demoUsageKey(tier.code, deviceId)),
        );
        return cors(
            new Response(toClient, {
                status: 200,
                headers: passthroughHeaders(upstream.headers),
            }),
        );
    }

    return cors(
        new Response(upstream.body, {
            status: 200,
            headers: passthroughHeaders(upstream.headers),
        }),
    );
}

/**
 * Mints a short-lived Deepgram JWT scoped to one streaming session. Client
 * uses the token to open a WS directly with Deepgram, bypassing the Worker.
 */
async function handleDeepgramToken(
    request: Request,
    env: Env,
    ctx: ExecutionContext,
): Promise<Response> {
    const deviceId = requireDeviceId(request);
    if (deviceId instanceof Response) return deviceId;

    const tier = await resolveTier(request, env, deviceId);
    if (tier instanceof Response) return tier;

    if (tier.kind === "trial") {
        const consumed = await consumeTrialTurn(env.USAGE_KV, deviceId, tier.turnsCap);
        if (!consumed) {
            return cors(
                jsonResponse(429, {
                    error: "trial_exhausted",
                    message:
                        "Free trial spent. Use your own API keys (BYOK) or contact us for an invite code.",
                    provider: "deepgram",
                    tier: "trial",
                }),
            );
        }
    } else {
        const kvKey = demoUsageKey(tier.code, deviceId);
        const usage = await readUsage(env.USAGE_KV, kvKey);
        if (usage.deepgram_tokens >= tier.caps.daily_deepgram_tokens) {
            return cors(
                jsonResponse(429, {
                    error: "daily_cap_exceeded",
                    message: "Daily STT session cap reached. Try again tomorrow.",
                    provider: "deepgram",
                    tier: "demo",
                    usage: { used: usage.deepgram_tokens, cap: tier.caps.daily_deepgram_tokens },
                }),
            );
        }
    }

    const upstream = await fetch(DEEPGRAM_TOKEN_URL, {
        method: "POST",
        headers: {
            authorization: `Token ${env.DEEPGRAM_API_KEY}`,
            "content-type": "application/json",
        },
        body: JSON.stringify({ ttl_seconds: DEEPGRAM_TOKEN_TTL_SECONDS }),
    });

    if (!upstream.ok) {
        const body = await upstream.text();
        console.error(`[deepgram/token] upstream ${upstream.status}: ${body}`);
        return cors(
            new Response(body, {
                status: upstream.status,
                headers: { "content-type": "application/json" },
            }),
        );
    }

    const grant = (await upstream.json()) as { access_token: string; expires_in: number };

    if (tier.kind === "demo") {
        ctx.waitUntil(
            bumpCounter(env.USAGE_KV, demoUsageKey(tier.code, deviceId), "deepgram_tokens"),
        );
    }

    return cors(
        jsonResponse(200, {
            token: grant.access_token,
            expires_in: grant.expires_in,
        }),
    );
}

/**
 * Mints a short-lived Cartesia access token scoped to TTS use. Same pattern
 * as Deepgram: client uses the returned token directly against Cartesia's
 * WebSocket, Worker isn't on the data path.
 */
async function handleCartesiaToken(
    request: Request,
    env: Env,
    ctx: ExecutionContext,
): Promise<Response> {
    const deviceId = requireDeviceId(request);
    if (deviceId instanceof Response) return deviceId;

    const tier = await resolveTier(request, env, deviceId);
    if (tier instanceof Response) return tier;

    if (tier.kind === "trial") {
        const consumed = await consumeTrialTurn(env.USAGE_KV, deviceId, tier.turnsCap);
        if (!consumed) {
            return cors(
                jsonResponse(429, {
                    error: "trial_exhausted",
                    message:
                        "Free trial spent. Use your own API keys (BYOK) or contact us for an invite code.",
                    provider: "cartesia",
                    tier: "trial",
                }),
            );
        }
    } else {
        const kvKey = demoUsageKey(tier.code, deviceId);
        const usage = await readUsage(env.USAGE_KV, kvKey);
        if (usage.cartesia_tokens >= tier.caps.daily_cartesia_tokens) {
            return cors(
                jsonResponse(429, {
                    error: "daily_cap_exceeded",
                    message: "Daily TTS session cap reached. Try again tomorrow.",
                    provider: "cartesia",
                    tier: "demo",
                    usage: { used: usage.cartesia_tokens, cap: tier.caps.daily_cartesia_tokens },
                }),
            );
        }
    }

    const upstream = await fetch(CARTESIA_TOKEN_URL, {
        method: "POST",
        headers: {
            authorization: `Bearer ${env.CARTESIA_API_KEY}`,
            "cartesia-version": CARTESIA_API_VERSION,
            "content-type": "application/json",
        },
        body: JSON.stringify({
            grants: { tts: true },
            expires_in: CARTESIA_TOKEN_TTL_SECONDS,
        }),
    });

    if (!upstream.ok) {
        const body = await upstream.text();
        console.error(`[cartesia/token] upstream ${upstream.status}: ${body}`);
        return cors(
            new Response(body, {
                status: upstream.status,
                headers: { "content-type": "application/json" },
            }),
        );
    }

    const grant = (await upstream.json()) as { token: string };

    if (tier.kind === "demo") {
        ctx.waitUntil(
            bumpCounter(env.USAGE_KV, demoUsageKey(tier.code, deviceId), "cartesia_tokens"),
        );
    }

    return cors(
        jsonResponse(200, {
            token: grant.token,
            expires_in: CARTESIA_TOKEN_TTL_SECONDS,
        }),
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Tier resolution + invite codes
// ────────────────────────────────────────────────────────────────────────────

/**
 * Inspects the request for `x-aegis-invite-code`. If absent, returns the trial
 * tier with the configured cap. If present, validates the code, enforces
 * expiry + device binding, and returns the demo tier. Returns an error
 * Response on any validation failure so callers can early-return.
 */
async function resolveTier(
    request: Request,
    env: Env,
    deviceId: string,
): Promise<Tier | Response> {
    const code = request.headers.get("x-aegis-invite-code");
    if (!code) {
        return { kind: "trial", turnsCap: parseCap(env.TRIAL_TURNS_CAP, 18) };
    }

    const normalized = code.trim().toUpperCase();
    if (!CODE_RE.test(normalized)) {
        return cors(jsonResponse(400, { error: "invalid invite code format" }));
    }

    const raw = await env.USAGE_KV.get(`invite:${normalized}`);
    if (!raw) {
        return cors(jsonResponse(403, { error: "invite_code_unknown" }));
    }

    let invite: InviteCode;
    try {
        invite = JSON.parse(raw) as InviteCode;
    } catch {
        console.error(`[invite] malformed payload for ${normalized}`);
        return cors(jsonResponse(500, { error: "invite_code_corrupt" }));
    }

    if (Date.parse(invite.expires_at) <= Date.now()) {
        return cors(jsonResponse(403, { error: "invite_code_expired" }));
    }

    // Bind device-id to code on first sighting. RMW with a small race window
    // that may briefly let one extra device through; acceptable for the trust
    // level of invite codes.
    if (!invite.devices_seen.includes(deviceId)) {
        if (invite.devices_seen.length >= invite.max_devices) {
            return cors(
                jsonResponse(403, {
                    error: "invite_code_device_limit",
                    message: `This code is limited to ${invite.max_devices} device(s).`,
                }),
            );
        }
        invite.devices_seen.push(deviceId);
        await env.USAGE_KV.put(`invite:${normalized}`, JSON.stringify(invite));
    }

    return {
        kind: "demo",
        code: normalized,
        caps: {
            daily_input_tokens: invite.daily_input_tokens,
            daily_output_tokens: invite.daily_output_tokens,
            daily_deepgram_tokens: invite.daily_deepgram_tokens,
            daily_cartesia_tokens: invite.daily_cartesia_tokens,
        },
    };
}

/**
 * Trial-tier read-modify-write: returns true and bumps the counter if the
 * device has turns left, false otherwise. Small race window where two
 * concurrent requests both succeed at the same cap boundary is acceptable.
 */
async function consumeTrialTurn(
    kv: KVNamespace,
    deviceId: string,
    cap: number,
): Promise<boolean> {
    const key = trialUsageKey(deviceId);
    const existing = await readTrialUsage(kv, key);
    if (existing.turns >= cap) return false;
    existing.turns += 1;
    await kv.put(key, JSON.stringify(existing), { expirationTtl: TRIAL_KV_TTL_SECONDS });
    return true;
}

// ────────────────────────────────────────────────────────────────────────────
// Shared helpers
// ────────────────────────────────────────────────────────────────────────────

/**
 * Reads + validates the device id from the request. Returns the id on success,
 * or a ready-to-return error Response on failure. Caller pattern:
 *
 *   const deviceId = requireDeviceId(request);
 *   if (deviceId instanceof Response) return deviceId;
 *   // ...use deviceId as a string
 */
function requireDeviceId(request: Request): string | Response {
    const deviceId = request.headers.get("x-aegis-device-id");
    if (!deviceId || !UUID_RE.test(deviceId)) {
        return cors(jsonResponse(401, { error: "missing or invalid X-Aegis-Device-Id" }));
    }
    return deviceId;
}

function demoUsageKey(code: string, deviceId: string): string {
    return `usage:demo:${code}:${deviceId}:${utcDateKey(new Date())}`;
}

function trialUsageKey(deviceId: string): string {
    return `usage:trial:${deviceId}`;
}

function utcDateKey(d: Date): string {
    return d.toISOString().slice(0, 10);
}

function parseCap(value: string | undefined, fallback: number): number {
    const n = parseInt(value ?? "", 10);
    return Number.isFinite(n) && n > 0 ? n : fallback;
}

async function readUsage(kv: KVNamespace, key: string): Promise<Usage> {
    const raw = await kv.get(key);
    if (!raw) return emptyUsage();
    try {
        const parsed = JSON.parse(raw) as Partial<Usage>;
        return {
            input: typeof parsed.input === "number" ? parsed.input : 0,
            output: typeof parsed.output === "number" ? parsed.output : 0,
            deepgram_tokens:
                typeof parsed.deepgram_tokens === "number" ? parsed.deepgram_tokens : 0,
            cartesia_tokens:
                typeof parsed.cartesia_tokens === "number" ? parsed.cartesia_tokens : 0,
        };
    } catch {
        return emptyUsage();
    }
}

async function readTrialUsage(kv: KVNamespace, key: string): Promise<TrialUsage> {
    const raw = await kv.get(key);
    if (!raw) return { turns: 0 };
    try {
        const parsed = JSON.parse(raw) as Partial<TrialUsage>;
        return { turns: typeof parsed.turns === "number" ? parsed.turns : 0 };
    } catch {
        return { turns: 0 };
    }
}

function emptyUsage(): Usage {
    return { input: 0, output: 0, deepgram_tokens: 0, cartesia_tokens: 0 };
}

async function bumpCounter(
    kv: KVNamespace,
    key: string,
    field: "deepgram_tokens" | "cartesia_tokens",
): Promise<void> {
    const existing = await readUsage(kv, key);
    existing[field] += 1;
    await kv.put(key, JSON.stringify(existing), { expirationTtl: DEMO_KV_TTL_SECONDS });
}

function jsonResponse(status: number, body: unknown): Response {
    return new Response(JSON.stringify(body), {
        status,
        headers: { "content-type": "application/json" },
    });
}

function cors(res: Response): Response {
    const headers = new Headers(res.headers);
    headers.set("access-control-allow-origin", "*");
    headers.set("access-control-allow-methods", "POST, OPTIONS");
    headers.set(
        "access-control-allow-headers",
        "content-type, anthropic-version, anthropic-beta, x-aegis-device-id, x-aegis-invite-code",
    );
    return new Response(res.body, { status: res.status, headers });
}

function passthroughHeaders(upstream: Headers): Headers {
    const out = new Headers();
    const ct = upstream.get("content-type");
    if (ct) out.set("content-type", ct);
    const cc = upstream.get("cache-control");
    if (cc) out.set("cache-control", cc);
    return out;
}

/**
 * Walk the Anthropic SSE response stream, sum input/output tokens, then add
 * them into the day's KV entry. See block comment in handleAnthropic for the
 * shape of Anthropic's usage events.
 */
async function tallyAnthropicUsage(
    stream: ReadableStream<Uint8Array>,
    kv: KVNamespace,
    kvKey: string,
): Promise<void> {
    let input = 0;
    let output = 0;
    const reader = stream.getReader();
    const decoder = new TextDecoder();
    let buf = "";

    try {
        while (true) {
            const { done, value } = await reader.read();
            if (done) break;
            buf += decoder.decode(value, { stream: true });
            const events = buf.split("\n\n");
            buf = events.pop() ?? "";

            for (const evt of events) {
                const dataLine = evt.split("\n").find((l) => l.startsWith("data: "));
                if (!dataLine) continue;
                const payload = dataLine.slice(6);
                if (payload === "[DONE]") continue;
                try {
                    const obj = JSON.parse(payload);
                    if (obj.type === "message_start" && obj.message?.usage?.input_tokens != null) {
                        input = obj.message.usage.input_tokens;
                    } else if (
                        obj.type === "message_delta" &&
                        obj.usage?.output_tokens != null
                    ) {
                        output = obj.usage.output_tokens;
                    }
                } catch {
                    // skip non-JSON / partial
                }
            }
        }
    } catch (err) {
        console.error("tally read error:", err);
    }

    if (input === 0 && output === 0) return;

    const existing = await readUsage(kv, kvKey);
    const total = {
        ...existing,
        input: existing.input + input,
        output: existing.output + output,
    };
    await kv.put(kvKey, JSON.stringify(total), { expirationTtl: DEMO_KV_TTL_SECONDS });
}
