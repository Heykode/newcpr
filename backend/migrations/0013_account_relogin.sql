-- Separate login material from managed accounts; deletion never cascades into the pool.
CREATE TABLE account_relogin_entries (
    id text PRIMARY KEY,
    email text NOT NULL,
    revision bigint NOT NULL CHECK (revision > 0),
    material jsonb NOT NULL CHECK (jsonb_typeof(material) = 'object')
);
CREATE UNIQUE INDEX account_relogin_email ON account_relogin_entries (lower(email));

CREATE TABLE account_relogin_settings (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    concurrency integer NOT NULL DEFAULT 1 CHECK (concurrency BETWEEN 1 AND 8),
    paused boolean NOT NULL DEFAULT false
);
INSERT INTO account_relogin_settings (singleton) VALUES (true);
