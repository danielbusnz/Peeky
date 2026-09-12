// Stripe billing: Checkout to start a subscription, the customer portal to
// manage it, and the webhook that flips a user's plan when Stripe says the
// subscription changed. Stripe's API is form-encoded, so every call goes
// through stripeRequest rather than a JSON client.
//
// Two ways a user ends up on pro:
//   in-app:  Settings -> Checkout. We know the user, so the Stripe customer
//            carries metadata[user_id] and the session carries
//            client_reference_id, and the webhook matches on that.
//   landing: the public payment link on getpeeky.ai. No user exists yet, so
//            the webhook matches by email, and if that finds nobody the
//            subscription is claimed at first sign-in (claimSubscriptionByEmail).

import { sessionFromRequest } from "../auth/jwt";
import { tierForSubscriptionStatus, verifyStripeSignature } from "../billing/webhook";
import {
    SITE_URL,
    STRIPE_API_URL,
    STRIPE_SIGNATURE_TOLERANCE_SECONDS,
    STRIPE_TRIAL_DAYS,
} from "../constants";
import { cors, jsonResponse } from "../http";
import type { Env } from "../types";

type UserBilling = {
    id: string;
    email: string | null;
    subscription_tier: string;
    stripe_customer_id: string | null;
};

const USER_COLUMNS = "id, email, subscription_tier, stripe_customer_id";

async function userWhere(env: Env, clause: string, value: string): Promise<UserBilling | null> {
    return env.DB.prepare(`SELECT ${USER_COLUMNS} FROM users WHERE ${clause} = ?`)
        .bind(value)
        .first<UserBilling>();
}

/** Set the plan, and link the Stripe customer when we learn it. */
async function setPlan(env: Env, userId: string, plan: "free" | "pro", customerId: string | null): Promise<void> {
    await env.DB.prepare(
        "UPDATE users SET subscription_tier = ?, stripe_customer_id = COALESCE(?, stripe_customer_id) WHERE id = ?",
    )
        .bind(plan, customerId, userId)
        .run();
}

async function stripeRequest<T>(
    env: Env,
    method: "GET" | "POST",
    path: string,
    params: Record<string, string> = {},
): Promise<T> {
    const form = new URLSearchParams(params);
    const url = method === "GET" ? `${STRIPE_API_URL}${path}?${form}` : `${STRIPE_API_URL}${path}`;
    const res = await fetch(url, {
        method,
        headers: {
            Authorization: `Bearer ${env.STRIPE_SECRET_KEY}`,
            ...(method === "POST" ? { "Content-Type": "application/x-www-form-urlencoded" } : {}),
        },
        body: method === "POST" ? form : undefined,
    });
    if (!res.ok) {
        throw new Error(`stripe ${method} ${path} failed: ${res.status}`);
    }
    return res.json<T>();
}

/** The Stripe customer for a user, creating one on first checkout. */
async function ensureCustomer(env: Env, user: UserBilling): Promise<string> {
    if (user.stripe_customer_id !== null) return user.stripe_customer_id;
    const params: Record<string, string> = {
        // Back-reference so a human reading the Stripe dashboard can find us.
        "metadata[user_id]": user.id,
    };
    if (user.email) params.email = user.email;
    const customer = await stripeRequest<{ id: string }>(env, "POST", "/customers", params);
    await env.DB.prepare("UPDATE users SET stripe_customer_id = ? WHERE id = ?")
        .bind(customer.id, user.id)
        .run();
    return customer.id;
}

/** The signed-in user behind the request, or the 401 to return. */
async function requireUser(request: Request, env: Env): Promise<UserBilling | Response> {
    const claims = await sessionFromRequest(request, env.JWT_SECRET);
    if (!claims) return cors(jsonResponse(401, { error: "invalid token" }));
    const user = await userWhere(env, "id", claims.sub);
    if (!user) return cors(jsonResponse(401, { error: "unknown user" }));
    return user;
}

/** POST /v1/billing/checkout -> { url } of a Stripe Checkout Session. */
export async function handleCheckout(request: Request, env: Env): Promise<Response> {
    const user = await requireUser(request, env);
    if (user instanceof Response) return user;
    if (user.subscription_tier === "pro") {
        return cors(jsonResponse(409, { error: "already subscribed" }));
    }
    try {
        const customer = await ensureCustomer(env, user);
        // Stripe's array syntax is line_items[0][price]; the response carries
        // the hosted URL the client opens in the browser.
        const session = await stripeRequest<{ url: string }>(env, "POST", "/checkout/sessions", {
            mode: "subscription",
            customer,
            client_reference_id: user.id,
            "line_items[0][price]": env.STRIPE_PRICE_ID,
            "line_items[0][quantity]": "1",
            "subscription_data[trial_period_days]": String(STRIPE_TRIAL_DAYS),
            allow_promotion_codes: "true",
            success_url: `${SITE_URL}/?subscribed=1`,
            cancel_url: `${SITE_URL}/`,
        });
        return cors(jsonResponse(200, { url: session.url }));
    } catch (err) {
        console.error("[billing] checkout failed:", err);
        return cors(jsonResponse(502, { error: "checkout failed" }));
    }
}

/** POST /v1/billing/portal -> { url } of a customer portal session. */
export async function handlePortal(request: Request, env: Env): Promise<Response> {
    const user = await requireUser(request, env);
    if (user instanceof Response) return user;
    if (user.stripe_customer_id === null) {
        return cors(jsonResponse(409, { error: "no subscription" }));
    }
    try {
        const session = await stripeRequest<{ url: string }>(env, "POST", "/billing_portal/sessions", {
            customer: user.stripe_customer_id,
            return_url: `${SITE_URL}/`,
        });
        return cors(jsonResponse(200, { url: session.url }));
    } catch (err) {
        console.error("[billing] portal failed:", err);
        return cors(jsonResponse(502, { error: "portal failed" }));
    }
}

type StripeEvent = { id: string; type: string; data: { object: Record<string, unknown> } };

/**
 * POST /v1/billing/webhook. Stripe retries on any non-2xx, so a failed D1
 * write returns 500 and comes back later; a bad signature is a 400 and stays
 * dead. Every branch is idempotent (it sets state, never increments), so
 * duplicate deliveries are harmless.
 */
export async function handleWebhook(request: Request, env: Env): Promise<Response> {
    const payload = await request.text();
    const header = request.headers.get("stripe-signature") ?? "";
    const valid = await verifyStripeSignature(
        payload,
        header,
        env.STRIPE_WEBHOOK_SECRET,
        Math.floor(Date.now() / 1000),
        STRIPE_SIGNATURE_TOLERANCE_SECONDS,
    );
    if (!valid) return jsonResponse(400, { error: "bad signature" });

    let event: StripeEvent;
    try {
        event = JSON.parse(payload) as StripeEvent;
    } catch {
        return jsonResponse(400, { error: "bad json" });
    }
    try {
        await applyEvent(env, event);
    } catch (err) {
        console.error(`[billing] webhook ${event.type} ${event.id} failed:`, err);
        return jsonResponse(500, { error: "apply failed" });
    }
    return jsonResponse(200, { received: true });
}

async function applyEvent(env: Env, event: StripeEvent): Promise<void> {
    const obj = event.data.object;
    const customerId = typeof obj.customer === "string" ? obj.customer : null;

    switch (event.type) {
        case "checkout.session.completed": {
            if (obj.mode !== "subscription") return;
            const userId = typeof obj.client_reference_id === "string" ? obj.client_reference_id : null;
            const details = obj.customer_details as { email?: string | null } | undefined;
            const email = details?.email ?? null;
            const user =
                (userId ? await userWhere(env, "id", userId) : null) ??
                (email ? await userWhere(env, "email", email) : null);
            if (!user) {
                // Landing-page buyer with no account yet. Claimed at sign-in.
                console.warn(`[billing] checkout ${event.id} matched no user (${email ?? "no email"})`);
                return;
            }
            await setPlan(env, user.id, "pro", customerId);
            return;
        }
        case "customer.subscription.updated":
        case "customer.subscription.deleted": {
            if (!customerId) return;
            const user = await userWhere(env, "stripe_customer_id", customerId);
            if (!user) return;
            await setPlan(env, user.id, tierForSubscriptionStatus(String(obj.status)), null);
            return;
        }
        default:
            return;
    }
}

/**
 * Called at sign-in for users with no linked Stripe customer: if a customer
 * with this email holds a subscription (they bought through the landing page
 * before ever signing in), link it and set the plan. Returns the resulting
 * plan. Throws on Stripe errors; the caller treats that as "leave as is".
 */
export async function claimSubscriptionByEmail(
    env: Env,
    userId: string,
    email: string,
): Promise<"free" | "pro"> {
    const customers = await stripeRequest<{ data: { id: string }[] }>(env, "GET", "/customers", {
        email,
        limit: "1",
    });
    const customer = customers.data[0];
    if (!customer) return "free";
    const subs = await stripeRequest<{ data: { status: string }[] }>(env, "GET", "/subscriptions", {
        customer: customer.id,
        status: "all",
        limit: "1",
    });
    const plan = tierForSubscriptionStatus(subs.data[0]?.status ?? "none");
    // Link the customer even on free so the portal can show past invoices.
    await setPlan(env, userId, plan, customer.id);
    return plan;
}
