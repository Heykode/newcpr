-- Excel credentials and observations never mutate the Codex credential owner.
ALTER TABLE provider_accounts DROP CONSTRAINT provider_accounts_responses_upstream_ck;
ALTER TABLE provider_accounts ADD CONSTRAINT provider_accounts_responses_upstream_ck CHECK (
    responses_upstream IN ('codex','excel') AND
    (responses_upstream='codex' OR (provider_kind='openai' AND authentication_kind IN ('oauth','none')))
);

CREATE TABLE openai_excel_credentials (
    account_id text PRIMARY KEY REFERENCES provider_accounts(id) ON DELETE CASCADE,
    credential_revision bigint NOT NULL DEFAULT 1 CHECK (credential_revision > 0),
    upstream_user_id text NOT NULL CHECK (length(upstream_user_id) BETWEEN 1 AND 512),
    upstream_account_id text,
    email text,
    plan_type text,
    credential_json jsonb NOT NULL CHECK (jsonb_typeof(credential_json) = 'object'
        AND octet_length(credential_json::text) <= 262144),
    credential_state text NOT NULL DEFAULT 'ready'
        CHECK (credential_state IN ('unknown', 'ready', 'expired', 'banned', 'invalid')),
    credential_observed_at timestamptz NOT NULL,
    access_token_expires_at timestamptz,
    has_refresh_token boolean NOT NULL DEFAULT false,
    next_refresh_at timestamptz,
    last_error_reason text,
    last_error_message text,
    suspended boolean NOT NULL DEFAULT false,
    quality_pause_owner text,
    refresh_attempt_revision bigint,
    refresh_attempt_started_at timestamptz,
    quota_json jsonb,
    quota_access_state text NOT NULL DEFAULT 'unknown'
        CHECK (quota_access_state IN ('unknown', 'allowed', 'exhausted')),
    quota_evidence text,
    quota_access_observed_at timestamptz,
    quota_reset_at timestamptz,
    quota_observed_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (has_refresh_token OR next_refresh_at IS NULL),
    CHECK ((refresh_attempt_revision IS NULL) = (refresh_attempt_started_at IS NULL)),
    CHECK (refresh_attempt_revision IS NULL OR refresh_attempt_revision = credential_revision),
    CHECK (credential_observed_at <= updated_at),
    CHECK (created_at <= updated_at),
    CHECK (quota_access_observed_at IS NULL OR quota_access_observed_at <= updated_at),
    CHECK (quota_observed_at IS NULL OR quota_observed_at <= updated_at)
);

CREATE INDEX openai_excel_refresh_due ON openai_excel_credentials(next_refresh_at)
    WHERE has_refresh_token AND NOT suspended;
