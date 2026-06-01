// Per-device usage metering backed by KV. Both tiers run on one model: a daily
// budget (Anthropic tokens, Deepgram/Cartesia mint counts) keyed per UTC day,
// per device (trial) or per code+device (demo). The date in the key is the
// reset boundary; a short TTL lets each day's entry expire on its own.

import { DAILY_USAGE_TTL_SECONDS } from "./constants";
import type { DailyBudget, DailyUsage, Tier } from "./types";

export function utcDateKey(d: Date): string {
    return d.toISOString().slice(0, 10);
}

/** The usage key for this (tier, device) on a given UTC day. */
export function dailyUsageKey(tier: Tier, deviceId: string, date: string): string {
    return tier.kind === "trial"
        ? `usage:trial:${deviceId}:${date}`
        : `usage:demo:${tier.code}:${deviceId}:${date}`;
}

function numOr0(v: unknown): number {
    return typeof v === "number" && Number.isFinite(v) ? v : 0;
}

export async function readDailyUsage(kv: KVNamespace, key: string): Promise<DailyUsage> {
    const raw = await kv.get(key);
    const p = (() => {
        if (!raw) return {};
        try {
            return JSON.parse(raw) as Partial<DailyUsage>;
        } catch {
            return {};
        }
    })();
    return {
        input_tokens: numOr0(p.input_tokens),
        output_tokens: numOr0(p.output_tokens),
        deepgram: numOr0(p.deepgram),
        cartesia: numOr0(p.cartesia),
    };
}

/**
 * Whether the device has any Anthropic budget left today. A turn's real cost
 * isn't known before the call, so this only rejects once a budget is already
 * met; the in-flight turn that crosses the line is allowed, then recorded. A
 * small overshoot at the boundary is acceptable.
 */
export function anthropicExhausted(usage: DailyUsage, budget: DailyBudget): boolean {
    return usage.input_tokens >= budget.input_tokens || usage.output_tokens >= budget.output_tokens;
}

/** Whether the device has any Deepgram mint budget left today. */
export function deepgramExhausted(usage: DailyUsage, budget: DailyBudget): boolean {
    return usage.deepgram >= budget.deepgram;
}

/** Whether the device has any Cartesia mint budget left today. */
export function cartesiaExhausted(usage: DailyUsage, budget: DailyBudget): boolean {
    return usage.cartesia >= budget.cartesia;
}

/**
 * Add `delta` to today's usage and persist. Read-modify-write with a small race
 * window: two concurrent turns can both read the pre-write value, so a device
 * may slip slightly past budget. Meant for ctx.waitUntil so the write stays off
 * the hot path.
 */
export async function recordUsage(
    kv: KVNamespace,
    key: string,
    delta: Partial<DailyUsage>,
): Promise<void> {
    const cur = await readDailyUsage(kv, key);
    const next: DailyUsage = {
        input_tokens: cur.input_tokens + (delta.input_tokens ?? 0),
        output_tokens: cur.output_tokens + (delta.output_tokens ?? 0),
        deepgram: cur.deepgram + (delta.deepgram ?? 0),
        cartesia: cur.cartesia + (delta.cartesia ?? 0),
    };
    await kv.put(key, JSON.stringify(next), { expirationTtl: DAILY_USAGE_TTL_SECONDS });
}
