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

    const res = await fetch("https://api.stripe.com/v1/customers", {
        method: "POST",
        headers: {
            Authorization: `Bearer ${env.STRIPE_SECRET_KEY}`,
            "Content-Type": "application/x-www-form-urlencoded",
        },
        body: new URLSearchParams({
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
