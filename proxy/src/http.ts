// Request/response plumbing shared by every handler: CORS, JSON responses,
// device-id extraction, header passthrough, and the two input validators.

import { INTENT_LABELS, UUID_RE } from "./constants";

export function jsonResponse(status: number, body: unknown): Response {
    return new Response(JSON.stringify(body), {
        status,
        headers: { "content-type": "application/json" },
    });
}

export function cors(res: Response): Response {
    const headers = new Headers(res.headers);
    headers.set("access-control-allow-origin", "*");
    headers.set("access-control-allow-methods", "POST, OPTIONS");
    headers.set(
        "access-control-allow-headers",
        "content-type, authorization, anthropic-version, anthropic-beta, x-aegis-device-id, x-aegis-invite-code",
    );
    return new Response(res.body, { status: res.status, headers });
}

export function passthroughHeaders(upstream: Headers): Headers {
    const out = new Headers();
    const ct = upstream.get("content-type");
    if (ct) out.set("content-type", ct);
    const cc = upstream.get("cache-control");
    if (cc) out.set("cache-control", cc);
    return out;
}

/**
 * Reads + validates the device id from the request. Returns the id on success,
 * or a ready-to-return error Response on failure. Caller pattern:
 *
 *   const deviceId = requireDeviceId(request);
 *   if (deviceId instanceof Response) return deviceId;
 *   // ...use deviceId as a string
 */
export function requireDeviceId(request: Request): string | Response {
    const deviceId = request.headers.get("x-aegis-device-id");
    if (!deviceId || !UUID_RE.test(deviceId)) {
        return cors(jsonResponse(401, { error: "missing or invalid X-Aegis-Device-Id" }));
    }
    return deviceId;
}

/**
 * Returns the label if it is one of the five known intents, null if absent,
 * or a 400 Response if present but unrecognized. Keeps the stored vocabulary
 * closed so a stale client can't poison the dataset with junk labels.
 */
export function validLabelOrNull(value: unknown): string | null | Response {
    if (value === undefined || value === null) return null;
    if (typeof value === "string" && INTENT_LABELS.includes(value)) return value;
    return cors(jsonResponse(400, { error: `unknown intent label: ${String(value)}` }));
}

/**
 * Server-side redaction backstop. The client already redacts; this masks the
 * highest-risk leaks (emails, 4+ digit runs like PINs/cards/phones) a second
 * time so nothing raw lands in the bucket even if the client misses it.
 */
export function scrubText(text: string): string {
    return text
        .replace(/[\w.+-]+@[\w-]+\.[\w.-]+/g, "<EMAIL>")
        .replace(/\d{4,}/g, "<NUM>");
}
