// Run with `npm test` (node --test, native TS stripping). Free and offline.
import assert from "node:assert/strict";
import { createHmac } from "node:crypto";
import { test } from "node:test";
import { tierForSubscriptionStatus, verifyStripeSignature } from "./webhook.ts";

const SECRET = "whsec_test_secret";
const PAYLOAD = '{"id":"evt_1","type":"checkout.session.completed"}';
const NOW = 1_800_000_000;

function sign(payload: string, t: number, secret = SECRET): string {
    return createHmac("sha256", secret).update(`${t}.${payload}`).digest("hex");
}

test("accepts a fresh, correctly signed payload", async () => {
    const header = `t=${NOW},v1=${sign(PAYLOAD, NOW)}`;
    assert.equal(await verifyStripeSignature(PAYLOAD, header, SECRET, NOW, 300), true);
});

test("accepts when any v1 candidate matches (secret rotation)", async () => {
    const header = `t=${NOW},v1=${sign(PAYLOAD, NOW, "old")},v1=${sign(PAYLOAD, NOW)}`;
    assert.equal(await verifyStripeSignature(PAYLOAD, header, SECRET, NOW, 300), true);
});

test("rejects a tampered payload", async () => {
    const header = `t=${NOW},v1=${sign(PAYLOAD, NOW)}`;
    const tampered = PAYLOAD.replace("evt_1", "evt_2");
    assert.equal(await verifyStripeSignature(tampered, header, SECRET, NOW, 300), false);
});

test("rejects the wrong secret", async () => {
    const header = `t=${NOW},v1=${sign(PAYLOAD, NOW, "other")}`;
    assert.equal(await verifyStripeSignature(PAYLOAD, header, SECRET, NOW, 300), false);
});

test("rejects a stale timestamp (replay)", async () => {
    const old = NOW - 301;
    const header = `t=${old},v1=${sign(PAYLOAD, old)}`;
    assert.equal(await verifyStripeSignature(PAYLOAD, header, SECRET, NOW, 300), false);
});

test("rejects malformed headers", async () => {
    assert.equal(await verifyStripeSignature(PAYLOAD, "", SECRET, NOW, 300), false);
    assert.equal(await verifyStripeSignature(PAYLOAD, `t=${NOW}`, SECRET, NOW, 300), false);
    assert.equal(await verifyStripeSignature(PAYLOAD, `v1=abc`, SECRET, NOW, 300), false);
    assert.equal(await verifyStripeSignature(PAYLOAD, `t=${NOW},v1=zz`, SECRET, NOW, 300), false);
});

test("subscription status maps to tier", () => {
    assert.equal(tierForSubscriptionStatus("active"), "pro");
    assert.equal(tierForSubscriptionStatus("trialing"), "pro");
    for (const s of ["past_due", "canceled", "unpaid", "incomplete", "incomplete_expired", "paused", ""]) {
        assert.equal(tierForSubscriptionStatus(s), "free", s);
    }
});
