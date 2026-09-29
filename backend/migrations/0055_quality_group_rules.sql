create table quality_group_rules (
  id text primary key,
  revision bigint not null default 1 check (revision > 0),
  name text not null check (length(name) between 1 and 128),
  filter jsonb not null check (jsonb_typeof(filter) = 'object'),
  config jsonb not null check (jsonb_typeof(config) = 'object'),
  last_synced_at timestamptz,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

-- A NULL rule keeps the administrator's exclusion until the group rule is deleted.
create table quality_group_members (
  group_rule_id text not null references quality_group_rules(id) on delete cascade,
  account_id text not null references provider_accounts(id) on delete cascade,
  rule_id text unique references quality_rules(id) on delete set null,
  applied_revision bigint not null check (applied_revision > 0),
  primary key (group_rule_id, account_id)
);
create index quality_group_members_account_idx on quality_group_members(account_id);
