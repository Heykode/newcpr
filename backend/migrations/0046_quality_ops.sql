create table quality_rules (
    id text primary key,
    account_id text not null unique references provider_accounts(id) on delete cascade,
    revision bigint not null default 1,
    config jsonb not null,
    enabled boolean not null,
    next_run_at timestamptz not null,
    pending boolean not null default false,
    lease_token text,
    lease_until timestamptz,
    last_status text,
    last_run_at timestamptz,
    updated_at timestamptz not null default now()
);
create index quality_rules_due on quality_rules(next_run_at) where enabled;

create table quality_runs (
    id text primary key,
    rule_id text not null references quality_rules(id) on delete cascade,
    rule_revision bigint not null,
    account_id text not null,
    model text not null,
    config jsonb not null,
    status text not null default 'running',
    started_at timestamptz not null default now(),
    finished_at timestamptz,
    correct integer not null default 0,
    incorrect integer not null default 0,
    unknown integer not null default 0,
    request_errors integer not null default 0,
    answers jsonb not null default '[]'
);
create index quality_runs_history on quality_runs(rule_id, started_at desc);
