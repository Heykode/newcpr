-- Rebuildable monitor projections, separate from account state and Plan learning.
create table account_group_monitor_snapshots (
  group_id text primary key references account_groups(id) on delete cascade,
  config_revision bigint not null check (config_revision > 0),
  sampled_at timestamptz not null,
  total_accounts bigint not null check (total_accounts >= 0),
  eligible_accounts bigint not null check (eligible_accounts >= 0),
  estimated_accounts bigint not null check (estimated_accounts >= 0),
  used_slots bigint check (used_slots >= 0),
  total_slots bigint not null check (total_slots >= 0),
  remaining_usd double precision,
  remaining_status text not null,
  expected_expiry_usd double precision,
  expiry_status text not null,
  consume_usd_per_minute double precision,
  quota_consume_usd_per_minute double precision,
  eta_minutes double precision,
  eta_status text not null,
  low_sample boolean not null,
  earliest_reset_at timestamptz
);
