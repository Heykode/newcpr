alter table provider_accounts
    add column relogin_count bigint not null default 0 check (relogin_count >= 0),
    add column last_relogin_at timestamptz;

create table account_relogin_successes (
    operation_id text primary key check (length(operation_id) > 0),
    account_id text not null references provider_accounts(id) on delete cascade,
    credential_revision bigint not null check (credential_revision > 0),
    succeeded_at timestamptz not null default now(),
    unique (account_id, credential_revision)
);
