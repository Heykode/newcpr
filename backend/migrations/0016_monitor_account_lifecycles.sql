-- Monitor-only observations. Account creation time is copied from its existing owner.
-- No account FK: confirmed failures must survive deletion and same-identity reimport.
create table monitor_account_lifecycles (
    identity_key text primary key,
    provider_kind text not null,
    plan_type text,
    account_created_at timestamptz not null,
    failure_started_at timestamptz,
    failure_reason text,
    dead_at timestamptz,
    sampled_at timestamptz not null
);

create index monitor_account_lifecycles_samples_idx
    on monitor_account_lifecycles (provider_kind, lower(plan_type), dead_at desc)
    where dead_at is not null;
