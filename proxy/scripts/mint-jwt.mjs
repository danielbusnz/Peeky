// Mint a local aegis session JWT for testing authed endpoints against
// `wrangler dev`. Reads JWT_SECRET from ../.dev.vars and signs an HS256 token
// matching the format verifyJwt expects in src/auth/jwt.ts.
//
// Usage:  node scripts/mint-jwt.mjs [sub] [tier]
//   sub  = user id (must exist in the local users table). Default test-user-1.
//   tier = free | pro. Default free.

import crypto from "node:crypto";
import fs from "node:fs";

const b64url = (buf) => Buffer.from(buf).toString("base64url");

const devVars = fs.readFileSync(new URL("../.dev.vars", import.meta.url), "utf8");
const secret = devVars.match(/^JWT_SECRET=(.*)$/m)?.[1];
if (!secret) {
    console.error("JWT_SECRET not found in .dev.vars");
    process.exit(1);
}

const sub = process.argv[2] ?? "test-user-1";
const tier = process.argv[3] ?? "free";
const now = Math.floor(Date.now() / 1000);

const header = { alg: "HS256", typ: "JWT" };
const payload = { sub, email: null, tier, iat: now, exp: now + 3600 };

const signingInput = `${b64url(JSON.stringify(header))}.${b64url(JSON.stringify(payload))}`;
const sig = crypto.createHmac("sha256", secret).update(signingInput).digest();

console.log(`${signingInput}.${b64url(sig)}`);
