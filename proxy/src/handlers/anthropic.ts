import {
    ANTHROPIC_URL,
    EST_INPUT_TOKENS_PER_TURN,
    EST_OUTPUT_TOKENS_PER_TURN,
} from "../constants";
import { cors, jsonResponse, passthroughHeaders, requireDeviceId } from "../http";
import { resolveTier } from "../tiers";
import type { Env } from "../types";
import { anthropicExhausted, dailyUsageKey, readDailyUsage, recordUsage, utcDateKey } from "../usage";

/**
 * Full HTTP proxy for Anthropic's Messages API. Checks the device's daily token
 * budget, then streams the SSE response through untouched. Charges a flat
 * per-turn token estimate against the budget, recorded off the hot path.
 */
export async function handleAnthropic(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
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

    const key = dailyUsageKey(tier, deviceId, utcDateKey(new Date()));
    const usage = await readDailyUsage(env.USAGE_KV, key);
    if (anthropicExhausted(usage, tier.budget)) {
        return cors(
            jsonResponse(429, {
                error: tier.kind === "trial" ? "trial_exhausted" : "code_exhausted",
                message:
                    tier.kind === "trial"
                        ? "Free trial spent for today. Use your own API keys (BYOK) or contact us for an invite code."
                        : "This invite code's daily budget is spent. It resets at 00:00 UTC.",
                provider: "anthropic",
                tier: tier.kind,
            }),
        );
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

    const upstream = await fetch(ANTHROPIC_URL, {
        method: "POST",
        headers: upstreamHeaders,
        body: rawBody,
    });

    return cors(
        new Response(upstream.body, {
            status: upstream.ok ? 200 : upstream.status,
            headers: passthroughHeaders(upstream.headers),
        }),
    );
}
