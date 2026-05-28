// Per-device usage metering backed by KV. Both tiers run on one mechanism: a
// lifetime call counter, keyed per device (trial) or per code+device (demo).
// An invite code just raises the cap; it does not change how metering works.

import { TURN_KV_TTL_SECONDS } from "./constants";
import type { Tier, TurnUsage } from "./types";

export function trialUsageKey(deviceId: string): string {
    return `usage:trial:${deviceId}`;
}

export function demoUsageKey(code: string, deviceId: string): string {
    return `usage:demo:${code}:${deviceId}`;
}

/** The counter key for whichever tier this request resolved to. */
export function usageKey(tier: Tier, deviceId: string): string {
    return tier.kind === "trial"
        ? trialUsageKey(deviceId)
        : demoUsageKey(tier.code, deviceId);
}

export function utcDateKey(d: Date): string {
    return d.toISOString().slice(0, 10);
}

export function parseCap(value: string | undefined, fallback: number): number {
    const n = parseInt(value ?? "", 10);
    return Number.isFinite(n) && n > 0 ? n : fallback;
}

async function readTurnUsage(kv: KVNamespace, key: string): Promise<TurnUsage> {
    const raw = await kv.get(key);
    if (!raw) return { turns: 0 };
    try {
        const parsed = JSON.parse(raw) as Partial<TurnUsage>;
        return { turns: typeof parsed.turns === "number" ? parsed.turns : 0 };
    } catch {
        return { turns: 0 };
    }
}

/**
 * Read-modify-write: returns true and bumps the counter if the device has
 * calls left under `cap`, false otherwise. The write refreshes the TTL, so an
 * active device's count persists while an idle one's expires (soft reset).
 * Small race window where two concurrent requests both succeed at the cap
 * boundary is acceptable.
 */
export async function consumeTurn(
    kv: KVNamespace,
    key: string,
    cap: number,
): Promise<boolean> {
    const existing = await readTurnUsage(kv, key);
    if (existing.turns >= cap) return false;
    existing.turns += 1;
    await kv.put(key, JSON.stringify(existing), { expirationTtl: TURN_KV_TTL_SECONDS });
    return true;
}
