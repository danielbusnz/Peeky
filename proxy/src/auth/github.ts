// GitHub sign-in, Worker-mediated so the OAuth client secret never ships in
// the desktop binary. Flow (poll-by-state, no deep link / loopback needed):
//
//   1. client picks a random `state` (uuid) and opens the system browser to
//      /auth/github/start?state=...
//   2. start stashes the state in KV (pending) and 302s to GitHub
//   3. user approves -> GitHub hits /auth/github/callback?code=&state=
//   4. callback exchanges the code (secret stays here), reads the identity,
//      upserts the user in D1, mints an peeky JWT, and parks it in KV under
//      the state
//   5. client polls /auth/github/session?state=... and collects the JWT
//
// We only read identity (id, login, email). GitHub's access token is used once
// here and discarded; it is never stored. Integrations stay local.

import { UUID_RE } from "../constants";
import { jsonResponse } from "../http";
import type { Env } from "../types";
import { signJwt } from "./jwt";

const AUTHORIZE_URL = "https://github.com/login/oauth/authorize";
const TOKEN_URL = "https://github.com/login/oauth/access_token";
const USER_URL = "https://api.github.com/user";
const EMAILS_URL = "https://api.github.com/user/emails";
// Identity only. No repo/gist scope: this login is unrelated to the local
// GitHub integration (which uses the `gh` CLI on the device).
const SCOPE = "read:user user:email";
// GitHub rejects API calls without a User-Agent.
const UA = "aegis-proxy";

// State lives in the shared KV under its own prefix. Short TTL: the user has
// to finish the browser dance promptly.
const statePrefix = (state: string) => `oauthstate:${state}`;
const STATE_TTL_SECONDS = 600;
// Session token lifetime. Long-ish for a desktop app; the client re-signs in
// when it expires.
const SESSION_TTL_SECONDS = 30 * 24 * 60 * 60;

type PendingState = { status: "pending" };
type DoneState = { status: "done"; token: string; email: string | null; name: string | null };

/** GET /auth/github/start?state=<uuid> -> 302 to GitHub. */
export async function handleGithubStart(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    const state = url.searchParams.get("state") ?? "";
    if (!UUID_RE.test(state)) {
        return jsonResponse(400, { error: "missing or invalid state" });
    }

    const pending: PendingState = { status: "pending" };
    await env.USAGE_KV.put(statePrefix(state), JSON.stringify(pending), {
        expirationTtl: STATE_TTL_SECONDS,
    });

    const authorize = new URL(AUTHORIZE_URL);
    authorize.searchParams.set("client_id", env.GITHUB_CLIENT_ID);
    authorize.searchParams.set("redirect_uri", `${url.origin}/auth/github/callback`);
    authorize.searchParams.set("scope", SCOPE);
    authorize.searchParams.set("state", state);
    return Response.redirect(authorize.toString(), 302);
}

/** GET /auth/github/callback?code=&state= -> exchange, upsert, park JWT. */
export async function handleGithubCallback(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    const code = url.searchParams.get("code") ?? "";
    const state = url.searchParams.get("state") ?? "";

    if (!UUID_RE.test(state) || !code) {
        return closePage("Sign-in failed: bad request. You can close this window.");
    }
    // The state must be one we issued and still pending.
    const parked = await env.USAGE_KV.get(statePrefix(state));
    if (!parked) {
        return closePage("Sign-in link expired. Close this and try again.");
    }

    const accessToken = await exchangeCode(env, code, `${url.origin}/auth/github/callback`);
    if (!accessToken) {
        return closePage("Sign-in failed: could not reach GitHub. You can close this window.");
    }

    const profile = await fetchIdentity(accessToken);
    if (!profile) {
        return closePage("Sign-in failed: could not read your GitHub profile.");
    }

    const user = await upsertUser(env, profile);
    const token = await signJwt(
        { sub: user.id, email: profile.email, tier: user.tier },
        env.JWT_SECRET,
        SESSION_TTL_SECONDS,
    );

    const done: DoneState = {
        status: "done",
        token,
        email: profile.email,
        name: profile.name,
    };
    await env.USAGE_KV.put(statePrefix(state), JSON.stringify(done), {
        expirationTtl: STATE_TTL_SECONDS,
    });

    return closePage("Signed in to Peeky. You can close this window and return to the app.");
}

/**
 * GET /auth/github/session?state= -> { status }.
 * pending while the browser dance is in flight; done (with token) once the
 * callback lands. The done entry is single-use: read it once, then delete.
 */
export async function handleGithubSession(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    const state = url.searchParams.get("state") ?? "";
    if (!UUID_RE.test(state)) {
        return jsonResponse(400, { error: "missing or invalid state" });
    }

    const raw = await env.USAGE_KV.get(statePrefix(state));
    if (!raw) {
        return jsonResponse(404, { status: "expired" });
    }
    const parsed = JSON.parse(raw) as PendingState | DoneState;
    if (parsed.status === "pending") {
        return jsonResponse(200, { status: "pending" });
    }
    // Done: hand it over once, then burn it.
    await env.USAGE_KV.delete(statePrefix(state));
    return jsonResponse(200, parsed);
}

type GithubProfile = {
    provider_uid: string;
    email: string | null;
    name: string | null;
    avatar_url: string | null;
};

async function exchangeCode(
    env: Env,
    code: string,
    redirectUri: string,
): Promise<string | null> {
    const resp = await fetch(TOKEN_URL, {
        method: "POST",
        headers: { "content-type": "application/json", accept: "application/json", "user-agent": UA },
        body: JSON.stringify({
            client_id: env.GITHUB_CLIENT_ID,
            client_secret: env.GITHUB_CLIENT_SECRET,
            code,
            redirect_uri: redirectUri,
        }),
    });
    if (!resp.ok) return null;
    const data = (await resp.json()) as { access_token?: string };
    return data.access_token ?? null;
}

async function fetchIdentity(accessToken: string): Promise<GithubProfile | null> {
    const headers = {
        authorization: `Bearer ${accessToken}`,
        accept: "application/vnd.github+json",
        "user-agent": UA,
    };
    const userResp = await fetch(USER_URL, { headers });
    if (!userResp.ok) return null;
    const u = (await userResp.json()) as {
        id: number;
        login: string;
        name: string | null;
        email: string | null;
        avatar_url: string | null;
    };

    let email = u.email;
    if (!email) {
        // Public email is often null; pull the primary verified one.
        const emailsResp = await fetch(EMAILS_URL, { headers });
        if (emailsResp.ok) {
            const emails = (await emailsResp.json()) as {
                email: string;
                primary: boolean;
                verified: boolean;
            }[];
            email = emails.find((e) => e.primary && e.verified)?.email ?? null;
        }
    }

    return {
        provider_uid: String(u.id),
        email,
        name: u.name ?? u.login,
        avatar_url: u.avatar_url,
    };
}

/** Insert the user on first sign-in, or refresh email/name on return. */
async function upsertUser(
    env: Env,
    profile: GithubProfile,
): Promise<{ id: string; tier: string }> {
    const existing = await env.DB.prepare(
        "SELECT id, subscription_tier FROM users WHERE provider = ? AND provider_uid = ?",
    )
        .bind("github", profile.provider_uid)
        .first<{ id: string; subscription_tier: string }>();

    if (existing) {
        await env.DB.prepare("UPDATE users SET email = ?, name = ?, avatar_url = ? WHERE id = ?")
            .bind(profile.email, profile.name, profile.avatar_url, existing.id)
            .run();
        return { id: existing.id, tier: existing.subscription_tier };
    }

    const id = crypto.randomUUID();
    await env.DB.prepare(
        "INSERT INTO users (id, provider, provider_uid, email, name, avatar_url, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
        .bind(
            id,
            "github",
            profile.provider_uid,
            profile.email,
            profile.name,
            profile.avatar_url,
            Math.floor(Date.now() / 1000),
        )
        .run();
    return { id, tier: "free" };
}

/** A tiny HTML page shown in the browser tab after the redirect dance. */
function closePage(message: string): Response {
    return new Response(
        `<!doctype html><html><head><meta charset="utf-8"><title>Peeky</title>
<style>body{background:#1a1a1a;color:#eee;font-family:system-ui,sans-serif;display:grid;place-items:center;height:100vh;margin:0}p{font-size:1.1rem}</style>
</head><body><p>${message}</p></body></html>`,
        { status: 200, headers: { "content-type": "text/html; charset=utf-8" } },
    );
}
