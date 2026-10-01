CREATE TABLE account_reset_inventories (
    account_id text PRIMARY KEY REFERENCES provider_accounts(id) ON DELETE CASCADE,
    checked_at timestamptz NOT NULL,
    document jsonb NOT NULL
);

CREATE TABLE account_reset_batches (
    id uuid PRIMARY KEY,
    created_at timestamptz NOT NULL DEFAULT now(),
    document jsonb NOT NULL,
    actor jsonb,
    identities jsonb NOT NULL
);

CREATE TABLE account_reset_consumptions (
    request_id uuid PRIMARY KEY,
    account_id text NOT NULL,
    credit_id text,
    identity jsonb NOT NULL,
    status text NOT NULL CHECK (status IN ('running', 'unknown', 'completed')),
    claim_id uuid NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    result jsonb
);
CREATE UNIQUE INDEX account_reset_consumptions_active
    ON account_reset_consumptions(account_id) WHERE status IN ('running', 'unknown');
CREATE INDEX account_reset_consumptions_credit
    ON account_reset_consumptions(account_id, credit_id);
CREATE INDEX account_reset_batches_created ON account_reset_batches(created_at DESC);
CREATE INDEX account_reset_batches_items ON account_reset_batches USING gin ((document->'items'));
