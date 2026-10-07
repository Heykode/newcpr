CREATE TABLE account_auto_reset_policies (
    account_id text PRIMARY KEY REFERENCES provider_accounts(id) ON DELETE CASCADE,
    revision bigint NOT NULL CHECK (revision > 0),
    config jsonb NOT NULL,
    identity jsonb NOT NULL,
    next_check_at timestamptz NOT NULL DEFAULT now(),
    check_claim uuid,
    check_until timestamptz,
    checked_at timestamptz,
    message text,
    last_trigger jsonb NOT NULL DEFAULT '[]'::jsonb
);
CREATE INDEX account_auto_reset_due ON account_auto_reset_policies(next_check_at)
    WHERE config->>'enabled' = 'true';

CREATE TABLE account_auto_reset_jobs (
    request_id uuid PRIMARY KEY,
    batch_id uuid NOT NULL REFERENCES account_reset_batches(id),
    account_id text NOT NULL,
    policy_revision bigint NOT NULL,
    observation jsonb NOT NULL
);
CREATE INDEX account_auto_reset_jobs_account ON account_auto_reset_jobs(account_id);
