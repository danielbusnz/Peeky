// Invite-code validation and tier resolution. Decides whether a request runs
// as trial (no code) or demo (valid code), and serves the read-only verify
// endpoint the onboarding UI calls.

import { CODE_RE } from "./constants";
import { cors, jsonResponse, requireDeviceId } from "./http";
import type { Env, InviteCode, InviteLookup, Tier } from "./types";
import { parseCap } from "./usage";

/**
 * Read-only validation of an invite code: format, existence, and device-slot
 * accounting. Performs NO writes (no device binding), so it's safe for both
 * the metered request path and the pre-flight verify endpoint. Returns the
 * parsed code on success or an error Response to early-return.
 */
export async function lookupInvite(
    env: Env,
    rawCode: string,
    deviceId: string,
): Promise<InviteLookup | Response> {
    const normalized = rawCode.trim().toUpperCase();
    if (!CODE_RE.test(normalized)) {
        return cors(jsonResponse(400, { error: "invalid invite code format" }));
    }

    const raw = await env.USAGE_KV.get(`invite:${normalized}`);
    if (!raw) {
        return cors(jsonResponse(403, { error: "invite_code_unknown" }));
    }

    let invite: InviteCode;
    try {
        invite = JSON.parse(raw) as InviteCode;
    } catch {
        console.error(`[invite] malformed payload for ${normalized}`);
        return cors(jsonResponse(500, { error: "invite_code_corrupt" }));
    }

    return {
        normalized,
        invite,
        deviceKnown: invite.devices_seen.includes(deviceId),
        hasRoom: invite.devices_seen.length < invite.max_devices,
    };
}

/**
 * Inspects the request for `x-aegis-invite-code`. If absent, returns the trial
 * tier with the configured cap. If present, validates the code, binds the
 * device, and returns the demo tier with the code's call cap. Returns an error
 * Response on any validation failure so callers can early-return.
 */
export async function resolveTier(
    request: Request,
    env: Env,
    deviceId: string,
): Promise<Tier | Response> {
    const code = request.headers.get("x-aegis-invite-code");
    if (!code) {
        return { kind: "trial", turnsCap: parseCap(env.TRIAL_TURNS_CAP, 18) };
    }

    const lookup = await lookupInvite(env, code, deviceId);
    if (lookup instanceof Response) return lookup;
    const { normalized, invite, deviceKnown, hasRoom } = lookup;

    // Bind device-id to code on first sighting. RMW with a small race window
    // that may briefly let one extra device through; acceptable for the trust
    // level of invite codes.
    if (!deviceKnown) {
        if (!hasRoom) {
            return cors(
                jsonResponse(403, {
                    error: "invite_code_device_limit",
                    message: `This code is limited to ${invite.max_devices} device(s).`,
                }),
            );
        }
        invite.devices_seen.push(deviceId);
        await env.USAGE_KV.put(`invite:${normalized}`, JSON.stringify(invite));
    }

    return { kind: "demo", code: normalized, turnsCap: invite.turns_cap };
}

/**
 * Pre-flight check the onboarding UI calls before the user commits a code.
 * Validates the code read-only (no device binding, no usage charged) so the
 * user can see green/red feedback without burning a device slot. A code at
 * its device limit that this device isn't already bound to is reported as
 * unusable, since committing it later would fail.
 */
export async function handleInviteVerify(request: Request, env: Env): Promise<Response> {
    const deviceId = requireDeviceId(request);
    if (deviceId instanceof Response) return deviceId;

    const code = request.headers.get("x-aegis-invite-code");
    if (!code) {
        return cors(jsonResponse(400, { error: "missing invite code" }));
    }

    const lookup = await lookupInvite(env, code, deviceId);
    if (lookup instanceof Response) return lookup;

    if (!lookup.deviceKnown && !lookup.hasRoom) {
        return cors(
            jsonResponse(403, {
                error: "invite_code_device_limit",
                message: `This code is limited to ${lookup.invite.max_devices} device(s).`,
            }),
        );
    }

    return cors(
        jsonResponse(200, {
            ok: true,
            turns_cap: lookup.invite.turns_cap,
            max_devices: lookup.invite.max_devices,
        }),
    );
}
