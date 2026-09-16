create table quota_learning_accounts (
  provider_account_id text not null references provider_accounts(id) on delete cascade,
  provider_kind text not null,
  plan_type text not null,
  window_key text not null,
  window_minutes integer not null,
  baseline_cost_usd double precision,
  baseline_percent double precision not null default 0,
  baseline_reset_at timestamptz,
  bound_usd double precision,
  bound_at timestamptz,
  last_percent double precision not null default 0,
  last_reset_at timestamptz,
  last_observed_at timestamptz not null,
  updated_at timestamptz not null default now(),
  primary key (provider_account_id, window_key, window_minutes)
);

create index quota_learning_accounts_plan_idx
  on quota_learning_accounts (provider_kind, lower(plan_type), window_key, window_minutes);

create table quota_learning_plan_samples (
  id bigserial primary key,
  provider_kind text not null,
  plan_type text not null,
  window_key text not null,
  window_minutes integer not null,
  bound_usd double precision not null,
  source_account_id text not null,
  bound_at timestamptz not null,
  unique (provider_kind, source_account_id, window_key, window_minutes)
);

create index quota_learning_plan_samples_lookup_idx
  on quota_learning_plan_samples (
    provider_kind,
    lower(plan_type),
    window_key,
    window_minutes,
    bound_at desc,
    id desc
  );
