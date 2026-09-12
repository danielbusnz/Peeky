// Signed-in account facts the console reads on demand. Served from D1 rather
// than the JWT so a plan change lands without a re-sign-in.

import { sessionFromRequest } from "../auth/jwt";
import { cors, jsonResponse } from "../http";
import type { Env } from "../types";

/** GET /v1/account/me -> { tier, email }. */
export async function handleAccountMe(request: Request, env: Env): Promise<Response> {
    const claims = await sessionFromRequest(request, env.JWT_SECRET);
    if (!claims) return cors(jsonResponse(401, { error: "invalid token" }));
    const row = await env.DB.prepare("SELECT email, subscription_tier FROM users WHERE id = ?")
        .bind(claims.sub)
        .first<{ email: string | null; subscription_tier: string }>();
    if (!row) return cors(jsonResponse(401, { error: "unknown user" }));
    return cors(jsonResponse(200, { tier: row.subscription_tier, email: row.email }));
}
