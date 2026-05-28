import { ANTHROPIC_URL } from "../constants";
import { cors, jsonResponse, passthroughHeaders, requireDeviceId } from "../http";
import { resolveTier } from "../tiers";
import type { Env } from "../types";
import { consumeTurn, usageKey } from "../usage";

/**
 * Full HTTP proxy for Anthropic's Messages API. Charges one call against the
 * device's lifetime counter up front, then streams the SSE response through
 * untouched.
 */
export async function handleAnthropic(request: Request, env: Env): Promise<Response> {
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

    const consumed = await consumeTurn(env.USAGE_KV, usageKey(tier, deviceId), tier.turnsCap);
    if (!consumed) {
        return cors(
            jsonResponse(429, {
                error: tier.kind === "trial" ? "trial_exhausted" : "code_exhausted",
                message:
                    tier.kind === "trial"
                        ? "Free trial spent. Use your own API keys (BYOK) or contact us for an invite code."
                        : "This invite code's uses are spent.",
                provider: "anthropic",
                tier: tier.kind,
            }),
        );
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

    return cors(
        new Response(upstream.body, {
            status: upstream.ok ? 200 : upstream.status,
            headers: passthroughHeaders(upstream.headers),
        }),
    );
}
