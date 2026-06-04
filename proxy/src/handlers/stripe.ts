import { verifyJwt } from "../auth/jwt";
import { cors, jsonResponse } from "../http";
import type { Env } from "../types";


async function getOrCreateStripeCustomer(env: Env, userId: string): Promise<string> {
    const row = await env.DB
        .prepare("SELECT stripe_customer_id FROM users WHERE id = ?")
        .bind(userId)
        .first<{ stripe_customer_id: string | null }>();

    if (row === null) {
        throw new Error(`No user found id ${userId}`);
    }
    if (row.stripe_customer_id !== null) {
        return row.stripe_customer_id;
    }

    // Stripe's API takes form-encoded bodies, not JSON.
    const res = await fetch("https://api.stripe.com/v1/customers", {
        method: "POST",
        headers: {
            Authorization: `Bearer ${env.STRIPE_SECRET_KEY}`,
            "Content-Type": "application/x-www-form-urlencoded",
        },
        body: new URLSearchParams({
            // Back-reference so subscription webhooks can resolve our user from the customer.
            "metadata[user_id]": userId,
        }),
    });
    if (!res.ok) {
        throw new Error(`stripe customer create failed: ${res.status}`);
    }
    const customer = await res.json<{ id: string }>();


    await env.DB
        .prepare("UPDATE users SET stripe_customer_id = ? WHERE id = ?")
        .bind(customer.id, userId)
        .run();

    return customer.id;


}

export async function handleCheckout(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const auth = request.headers.get("Authorization");
    if (!auth?.startsWith("Bearer ")) {
        return cors(new Response("missing token", { status: 401 }));
    }
    const token = auth.slice(7);

    const claims = await verifyJwt(token, env.JWT_SECRET);
    if (claims === null) {
        return cors(new Response("invalid token", { status: 401 }));
    }

    const customerId = await getOrCreateStripeCustomer(env, claims.sub);

    // Same form-encoded POST as the customer call, different endpoint. Stripe's
    // array syntax is line_items[0][price], the response carries the hosted url.
    const res = await fetch("https://api.stripe.com/v1/checkout/sessions", {
        method: "POST",
        headers: {
            Authorization: `Bearer ${env.STRIPE_SECRET_KEY}`,
            "Content-Type": "application/x-www-form-urlencoded",
        },
        body: new URLSearchParams({
            mode: "subscription",
            customer: customerId,
            "line_items[0][price]": env.STRIPE_PRICE_ID,
            "line_items[0][quantity]": "1",
            success_url: "https://aegis.dev/upgrade/success",
            cancel_url: "https://aegis.dev/upgrade/cancel",
        }),
    });
    if (!res.ok) {
        return cors(jsonResponse(502, { error: "stripe checkout create failed" }));
    }

    const session = await res.json<{ url: string }>();
    return cors(jsonResponse(200, { url: session.url }));
}
