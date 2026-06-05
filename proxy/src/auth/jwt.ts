// Minimal HS256 JWT, hand-rolled on Web Crypto so the Worker stays
// dependency-free. Used to mint the peeky session token the desktop client
// stores after sign-in and replays on metered requests. Stateless: the proxy
// verifies the signature and expiry, no session table.

function b64urlEncode(bytes: Uint8Array): string {
    let bin = "";
    for (const b of bytes) bin += String.fromCharCode(b);
    return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function b64urlDecode(s: string): Uint8Array {
    const pad = s.length % 4 === 0 ? "" : "=".repeat(4 - (s.length % 4));
    const bin = atob(s.replace(/-/g, "+").replace(/_/g, "/") + pad);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
}

const enc = (s: string) => new TextEncoder().encode(s);

async function hmacKey(secret: string): Promise<CryptoKey> {
    return crypto.subtle.importKey(
        "raw",
        enc(secret),
        { name: "HMAC", hash: "SHA-256" },
        false,
        ["sign", "verify"],
    );
}

export type JwtClaims = {
    /** Our user id (uuid). */
    sub: string;
    email: string | null;
    tier: string;
    iat: number;
    exp: number;
};

/** Sign `claims` (sans iat/exp) into a compact JWS, valid for `ttlSeconds`. */
export async function signJwt(
    claims: Omit<JwtClaims, "iat" | "exp">,
    secret: string,
    ttlSeconds: number,
): Promise<string> {
    const now = Math.floor(Date.now() / 1000);
    const header = { alg: "HS256", typ: "JWT" };
    const body: JwtClaims = { ...claims, iat: now, exp: now + ttlSeconds };
    const signingInput = `${b64urlEncode(enc(JSON.stringify(header)))}.${b64urlEncode(
        enc(JSON.stringify(body)),
    )}`;
    const key = await hmacKey(secret);
    const sig = new Uint8Array(await crypto.subtle.sign("HMAC", key, enc(signingInput)));
    return `${signingInput}.${b64urlEncode(sig)}`;
}

/**
 * Verify signature + expiry. Returns the claims on success, or null on any
 * failure (bad shape, bad signature, expired). Uses crypto.subtle.verify so
 * the comparison is constant-time.
 */
export async function verifyJwt(token: string, secret: string): Promise<JwtClaims | null> {
    const parts = token.split(".");
    if (parts.length !== 3) return null;
    const [h, p, s] = parts as [string, string, string];
    const key = await hmacKey(secret);
    const ok = await crypto.subtle.verify("HMAC", key, b64urlDecode(s), enc(`${h}.${p}`));
    if (!ok) return null;
    let claims: JwtClaims;
    try {
        claims = JSON.parse(new TextDecoder().decode(b64urlDecode(p))) as JwtClaims;
    } catch {
        return null;
    }
    if (typeof claims.exp !== "number" || claims.exp <= Math.floor(Date.now() / 1000)) {
        return null;
    }
    return claims;
}
