-- Links a user to their Stripe customer. Null until the user's first checkout,
-- when we create the Stripe customer and store the cus_... id here. One user maps
-- to one Stripe customer, set once and never changed.

ALTER TABLE users ADD COLUMN stripe_customer_id TEXT;
