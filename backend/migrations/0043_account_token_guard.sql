create table account_token_guard_config (
    singleton boolean primary key default true check (singleton),
    config jsonb not null
);

insert into account_token_guard_config (config) values (
    '{"enabled":false,"groupIds":[],"model":"gpt-6-astra","intervalSeconds":300,"timeoutSeconds":60,"concurrency":6,"maxPerCycle":12}'
);

create table account_token_guard_events (
    id bigint generated always as identity primary key,
    account_id text not null references provider_accounts(id) on delete cascade,
    observed_at timestamptz not null,
    event jsonb not null
);
create index account_token_guard_events_account_time on account_token_guard_events (account_id, observed_at desc);
create index account_token_guard_events_time on account_token_guard_events (observed_at desc);
