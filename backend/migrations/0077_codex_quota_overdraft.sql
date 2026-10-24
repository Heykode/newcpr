ALTER TABLE runtime_settings
    ADD COLUMN codex_quota_overdraft_enabled boolean NOT NULL DEFAULT false,
    ADD COLUMN codex_quota_overdraft_probe_model text NOT NULL DEFAULT 'gpt-6-astra'
        CHECK (length(btrim(codex_quota_overdraft_probe_model)) BETWEEN 1 AND 128
            AND codex_quota_overdraft_probe_model = btrim(codex_quota_overdraft_probe_model)
            AND codex_quota_overdraft_probe_model !~ '[[:cntrl:]]');

CREATE TABLE codex_quota_overdraft_states (
    account_id text PRIMARY KEY REFERENCES provider_accounts(id) ON DELETE CASCADE,
    credential_revision bigint NOT NULL CHECK (credential_revision > 0),
    account_subject text NOT NULL CHECK (account_subject <> ''),
    config_revision bigint NOT NULL CHECK (config_revision > 0),
    status text NOT NULL CHECK (status IN ('pending', 'passed', 'inconclusive', 'failed', 'recovered')),
    quota_window text NOT NULL CHECK (quota_window IN ('5h', '7d', 'multiple')),
    cycle_key text NOT NULL CHECK (cycle_key <> ''),
    generation bigint NOT NULL CHECK (generation > 0),
    probe_model text NOT NULL CHECK (length(probe_model) BETWEEN 1 AND 128 AND probe_model !~ '[[:cntrl:]]'),
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 5),
    attempt_limit integer NOT NULL DEFAULT 5 CHECK (attempt_limit BETWEEN 1 AND 5),
    reason_code text,
    started_at timestamptz NOT NULL,
    tested_at timestamptz,
    retry_at timestamptz,
    recover_at timestamptz,
    five_hour_started_at timestamptz,
    five_hour_recover_at timestamptz,
    seven_day_started_at timestamptz,
    seven_day_recover_at timestamptz,
    owner_token text,
    lease_until timestamptz,
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (account_id, cycle_key, generation)
);

CREATE INDEX idx_codex_quota_overdraft_due
    ON codex_quota_overdraft_states (status, retry_at, lease_until, updated_at);
