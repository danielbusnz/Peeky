import {
    ANTHROPIC_URL,
    EST_INPUT_TOKENS_PER_TURN,
    EST_OUTPUT_TOKENS_PER_TURN,
} from "../constants";
import { cors, jsonResponse, passthroughHeaders, requireDeviceId } from "../http";
import { resolveTier } from "../tiers";
import type { Env } from "../types";
import {
    anthropicExhausted,
    dailyUsageKey,
    exhaustionBody,
    readDailyUsage,
    recordUsage,
    utcDateKey,
} from "../usage";

/**
 * Full HTTP proxy for Anthropic's Messages API. Checks the device's daily token
 * budget, then streams the request body up and the SSE response back through
 * untouched. Charges a flat per-turn token estimate, recorded off the hot path.
 */
export async function handleAnthropic(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const deviceId = requireDeviceId(request);
    if (deviceId instanceof Response) return deviceId;

    const tier = await resolveTier(request, env, deviceId);
    if (tier instanceof Response) return tier;

    const key = dailyUsageKey(tier, deviceId, utcDateKey(new Date()));
    // Dev-only test hook: force the budget wall with no upstream call so the
    // client upgrade flow can be exercised without spending tokens. Gated on
    // DEV_HEADERS so it is inert in production. Mirrors FORCE_EXHAUSTED_HEADER
    // in peeky/src/providers/proxy_contract.rs.
    if (env.DEV_HEADERS === "1" && request.headers.get("x-peeky-force-exhausted") === "1") {
        return cors(jsonResponse(429, exhaustionBody(tier, "anthropic")));
    }

    const usage = await readDailyUsage(env.USAGE_KV, key);
    if (anthropicExhausted(usage, tier.budget)) {
        return cors(jsonResponse(429, exhaustionBody(tier, "anthropic")));
    }

    // Charge a flat per-turn estimate off the hot path. The write runs after we
    // return below, via ctx.waitUntil, so it never blocks time-to-first-token.
    // Charged on attempt, not on success.
    ctx.waitUntil(
        recordUsage(env.USAGE_KV, key, {
            input_tokens: EST_INPUT_TOKENS_PER_TURN,
            output_tokens: EST_OUTPUT_TOKENS_PER_TURN,
        }),
    );

    const upstreamHeaders: Record<string, string> = {
        "x-api-key": env.ANTHROPIC_API_KEY,
        "anthropic-version": request.headers.get("anthropic-version") ?? "2023-06-01",
        "content-type": "application/json",
    };
    const beta = request.headers.get("anthropic-beta");
    if (beta) upstreamHeaders["anthropic-beta"] = beta;

    // Stream the request body straight through instead of buffering it. The
    // peeky client always sends stream:true, so we skip parsing the body just
    // to check that. For screenshot turns this overlaps the client->edge and
    // edge->Anthropic uploads instead of doing them back to back. `duplex` is
    // required when the body is a stream and isn't in the DOM RequestInit type.
    const upstream = await fetch(ANTHROPIC_URL, {
        method: "POST",
        headers: upstreamHeaders,
        body: request.body,
        duplex: "half",
    } as RequestInit & { duplex: "half" });

    return cors(
        new Response(upstream.body, {
            status: upstream.ok ? 200 : upstream.status,
            headers: passthroughHeaders(upstream.headers),
        }),
    );
}
