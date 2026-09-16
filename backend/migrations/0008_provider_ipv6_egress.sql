create table provider_egress_settings (
    id integer primary key check (id = 1),
    revision bigint not null default 1 check (revision > 0),
    default_mode text not null default 'unchanged'
        check (default_mode in ('unchanged', 'fixed_ipv6_reuse', 'random_ipv6_reuse',
                               'fixed_ipv6_fresh', 'random_ipv6_fresh')),
    rotation_cursor bigint not null default 0 check (rotation_cursor >= 0)
);
insert into provider_egress_settings (id) values (1);

create table provider_egress_addresses (
    id text primary key,
    address text not null unique check (family(address::inet) = 6),
    enabled boolean not null default false,
    position integer not null check (position >= 0)
);
create index provider_egress_addresses_order_idx
    on provider_egress_addresses (position, id);

create table provider_egress_account_overrides (
    provider_account_id text primary key references provider_accounts (id) on delete cascade,
    mode text not null
        check (mode in ('unchanged', 'fixed_ipv6_reuse', 'random_ipv6_reuse',
                       'fixed_ipv6_fresh', 'random_ipv6_fresh'))
);

-- No foreign key: fixed source history survives account deletion and pool edits.
-- Empty identity components are normalized deliberately, without case folding.
create table provider_egress_fixed_affinity (
    provider_kind text not null,
    upstream_account_key text not null,
    upstream_user_key text not null,
    address text not null check (family(address::inet) = 6),
    created_at timestamptz not null default now(),
    primary key (provider_kind, upstream_account_key, upstream_user_key),
    check (upstream_account_key <> '' and upstream_user_key <> '')
);
