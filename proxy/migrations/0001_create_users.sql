-- Accounts for peeky. A user is one identity from a social provider (github,
-- google). Integration tokens are NOT stored here: they stay on the device.
-- This table backs login, and later the subscription tier the proxy meters
-- against and a synced settings blob.

CREATE TABLE IF NOT EXISTS users (
    id                TEXT PRIMARY KEY,            -- uuid v4, our own id
    provider          TEXT NOT NULL,               -- 'github' | 'google'
    provider_uid      TEXT NOT NULL,               -- the provider's stable user id
    email             TEXT,                        -- may be null if the provider hides it
    name              TEXT,
    avatar_url        TEXT,
    created_at        INTEGER NOT NULL,            -- unix seconds
    subscription_tier TEXT NOT NULL DEFAULT 'free',
    UNIQUE (provider, provider_uid)
);

CREATE INDEX IF NOT EXISTS idx_users_email ON users (email);
