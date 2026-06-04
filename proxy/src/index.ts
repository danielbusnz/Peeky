// aegis-proxy: Cloudflare Worker that fronts Anthropic, Deepgram, and Cartesia
// for the aegis desktop client.
//
// Why it exists:
//   The desktop app ships without API keys so non-technical users can install
//   it and have it just work. The Worker holds all three secret keys, caps
//   per-device usage from a KV store, and streams/forwards responses.
//
// Two tiers, selected per request. Both meter against per-UTC-day budgets
// (Anthropic tokens, Deepgram/Cartesia mint counts) that reset at midnight UTC.
//   trial: no invite code. Uses TRIAL_DAILY_BUDGET from constants.ts. Per
//          device, keyed by UTC day.
//   demo:  request carries `x-aegis-invite-code`. The code's KV payload sets
//          the daily budgets, an expiry, and a max-devices binding. Used for
//          recruiter demos and anyone we hand-grant extended access.
//
// Routes:
//   POST /v1/anthropic/messages   HTTP SSE proxy to Claude Messages API
//   POST /v1/deepgram/token       mint short-lived Deepgram JWT for STT WS
//   POST /v1/cartesia/token       mint short-lived Cartesia access token for TTS WS
//   POST /v1/invite/verify        read-only check that an invite code is usable
//   POST /v1/routelet/sample      store one redacted classification sample in R2
//   POST /v1/billing/checkout     create a Stripe subscription Checkout Session
//
// Why mixed patterns:
//   Anthropic is HTTP request/response with streaming SSE. We forward bytes
//   through untouched and charge a flat per-turn token estimate. Cheap.
//
//   Deepgram + Cartesia are WebSocket-only for streaming. Proxying WebSockets
//   through Workers is hairy (bidirectional forwarding, mid-stream cap
//   enforcement, idle timeouts). Instead, both providers offer short-lived
//   token APIs intended exactly for this client-direct-to-provider pattern.
//   The Worker just mints a token; the client connects directly. ~20 lines per
//   provider, no per-message proxying.
//
// This file is the router only. Handlers live in handlers/, the metering layer
// in usage.ts, tier resolution in tiers.ts, shared plumbing in http.ts.

import { handleGithubCallback, handleGithubSession, handleGithubStart } from "./auth/github";
import { handleAnthropic } from "./handlers/anthropic";
import { handleRouteletSample } from "./handlers/routelet";
import { handleCheckout } from "./handlers/stripe";
import { handleCartesiaToken, handleDeepgramToken } from "./handlers/tokens";
import { cors } from "./http";
import { handleInviteVerify } from "./tiers";
import type { Env } from "./types";

export type { Env };

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
            if (url.pathname === "/v1/invite/verify") {
                return handleInviteVerify(request, env);
            }
            if (url.pathname === "/v1/routelet/sample") {
                return handleRouteletSample(request, env);
            }
            if (url.pathname === "/v1/billing/checkout") {
                return handleCheckout(request, env, ctx);
            }
        }

        // GitHub sign-in. start + callback are browser redirects; session is
        // polled by the desktop client (via reqwest, so no CORS needed).
        if (request.method === "GET") {
            if (url.pathname === "/auth/github/start") {
                return handleGithubStart(request, env);
            }
            if (url.pathname === "/auth/github/callback") {
                return handleGithubCallback(request, env);
            }
            if (url.pathname === "/auth/github/session") {
                return handleGithubSession(request, env);
            }
        }

        return cors(new Response("Not found", { status: 404 }));
    },
} satisfies ExportedHandler<Env>;
