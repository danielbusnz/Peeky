// Short-lived token minting for Deepgram (STT) and Cartesia (TTS). Both follow
// the same shape: charge one call against the device's lifetime counter, ask
// the provider for a scoped token, hand it back. The client then streams
// directly to the provider over a WebSocket, so the Worker never sits on the
// audio data path.

import {
    CARTESIA_API_VERSION,
    CARTESIA_TOKEN_TTL_SECONDS,
    CARTESIA_TOKEN_URL,
    DEEPGRAM_TOKEN_TTL_SECONDS,
    DEEPGRAM_TOKEN_URL,
} from "../constants";
import { cors, jsonResponse, requireDeviceId } from "../http";
import { resolveTier } from "../tiers";
import type { Env, Tier } from "../types";
import {
    cartesiaExhausted,
    dailyUsageKey,
    deepgramExhausted,
    exhaustionBody,
    readDailyUsage,
    recordUsage,
    utcDateKey,
} from "../usage";

/** 429 body for a tier that has spent its daily budget. */
function exhausted(tier: Tier, provider: string): Response {
    return cors(jsonResponse(429, exhaustionBody(tier, provider)));
}

/**
 * Mints a short-lived Deepgram JWT scoped to one streaming session. Client
 * uses the token to open a WS directly with Deepgram, bypassing the Worker.
 */
export async function handleDeepgramToken(
    request: Request,
    env: Env,
    ctx: ExecutionContext,
): Promise<Response> {
    const deviceId = requireDeviceId(request);
    if (deviceId instanceof Response) return deviceId;

    const tier = await resolveTier(request, env, deviceId);
    if (tier instanceof Response) return tier;

    const key = dailyUsageKey(tier, deviceId, utcDateKey(new Date()));
    const usage = await readDailyUsage(env.USAGE_KV, key);
    if (deepgramExhausted(usage, tier.budget)) return exhausted(tier, "deepgram");
    ctx.waitUntil(recordUsage(env.USAGE_KV, key, { deepgram: 1 }));

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
export async function handleCartesiaToken(
    request: Request,
    env: Env,
    ctx: ExecutionContext,
): Promise<Response> {
    const deviceId = requireDeviceId(request);
    if (deviceId instanceof Response) return deviceId;

    const tier = await resolveTier(request, env, deviceId);
    if (tier instanceof Response) return tier;

    const key = dailyUsageKey(tier, deviceId, utcDateKey(new Date()));
    const usage = await readDailyUsage(env.USAGE_KV, key);
    if (cartesiaExhausted(usage, tier.budget)) return exhausted(tier, "cartesia");
    ctx.waitUntil(recordUsage(env.USAGE_KV, key, { cartesia: 1 }));

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

    return cors(
        jsonResponse(200, {
            token: grant.token,
            expires_in: CARTESIA_TOKEN_TTL_SECONDS,
        }),
    );
}
