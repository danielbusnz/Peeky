// Pure helpers for the Stripe webhook: signature verification and the
// subscription-status -> tier rule. No imports on purpose, so `node --test`
// can load this file directly (see webhook.test.ts) without a bundler.

const encoder = new TextEncoder();

function hexToBytes(hex: string): Uint8Array | null {
    if (hex.length % 2 !== 0 || !/^[0-9a-f]*$/i.test(hex)) return null;
    const out = new Uint8Array(hex.length / 2);
    for (let i = 0; i < out.length; i++) {
        out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
    }
    return out;
}

/**
 * Verify a `Stripe-Signature` header against the raw request body. The header
 * is `t=<unix>,v1=<hex>[,v1=<hex>...]`; the signed string is `<t>.<payload>`
 * under HMAC-SHA256 with the endpoint's signing secret. Any matching v1 wins
 * (Stripe sends several during a secret rotation). Events whose timestamp is
 * more than `toleranceSeconds` from `nowSeconds` are rejected as replays.
 * Comparison goes through crypto.subtle.verify, which is constant-time.
 */
export async function verifyStripeSignature(
    payload: string,
    header: string,
    secret: string,
    nowSeconds: number,
    toleranceSeconds: number,
): Promise<boolean> {
    let timestamp: number | null = null;
    const candidates: Uint8Array[] = [];
    for (const part of header.split(",")) {
        const eq = part.indexOf("=");
        if (eq < 0) continue;
        const key = part.slice(0, eq).trim();
        const value = part.slice(eq + 1).trim();
        if (key === "t") {
            const t = Number(value);
            if (Number.isFinite(t)) timestamp = t;
        } else if (key === "v1") {
            const bytes = hexToBytes(value);
            if (bytes) candidates.push(bytes);
        }
    }
    if (timestamp === null || candidates.length === 0) return false;
    if (Math.abs(nowSeconds - timestamp) > toleranceSeconds) return false;

    const key = await crypto.subtle.importKey(
        "raw",
        encoder.encode(secret),
        { name: "HMAC", hash: "SHA-256" },
        false,
        ["verify"],
    );
    const signed = encoder.encode(`${timestamp}.${payload}`);
    for (const candidate of candidates) {
        if (await crypto.subtle.verify("HMAC", key, candidate, signed)) return true;
    }
    return false;
}

/**
 * The tier a user gets for a Stripe subscription status. `trialing` counts as
 * paid so the 7-day trial is the full product; everything that isn't live
 * (past_due, canceled, unpaid, incomplete, paused) drops back to free.
 */
export function tierForSubscriptionStatus(status: string): "free" | "pro" {
    return status === "active" || status === "trialing" ? "pro" : "free";
}
