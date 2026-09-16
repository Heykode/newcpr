-- Durable provider device facts survive account row deletion. Secrets,
-- cookies, request identifiers and transport keys are intentionally absent.
create table if not exists provider_device_identities (
    provider_kind text not null,
    upstream_user_id text not null,
    upstream_account_id text not null default '',
    installation_id text not null,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    primary key (provider_kind, upstream_user_id, upstream_account_id)
);

create unique index if not exists provider_device_identities_installation_uq
    on provider_device_identities (installation_id);

alter table provider_device_identities
    add constraint provider_device_identities_provider_ck
    check (length(provider_kind) between 1 and 128);

alter table provider_device_identities
    add constraint provider_device_identities_user_ck
    check (length(upstream_user_id) between 1 and 512);
