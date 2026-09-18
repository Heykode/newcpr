-- Optional OpenAI turn-state maintenance. The feature is disabled globally and per account by default.
alter table runtime_settings
  add column turn_state_injection_enabled boolean not null default false,
  add column turn_state_models text[] not null default array[
    'gpt-6-astra',
    'gpt-5.6-sol',
    'gpt-5.6-terra'
  ]::text[];

alter table runtime_settings
  add constraint runtime_settings_turn_state_models_ck check (
    cardinality(turn_state_models) between 1 and 64
    and array_position(turn_state_models, null) is null
  );

alter table provider_accounts
  add column turn_state_injection_enabled boolean not null default false;

create table provider_turn_states (
  provider_account_id text not null,
  upstream_model text not null,
  normal_length smallint not null,
  active_state text,
  active_issued_at timestamptz,
  active_expires_at timestamptz,
  standby_state text,
  standby_issued_at timestamptz,
  standby_expires_at timestamptz,
  state_version bigint not null default 0,
  refresh_status text not null default 'missing',
  last_observed_length smallint,
  last_probe_at timestamptz,
  last_success_at timestamptz,
  updated_at timestamptz not null default now(),
  primary key (provider_account_id, upstream_model),
  constraint provider_turn_states_account_fk foreign key (provider_account_id)
    references provider_accounts (id)
    on update restrict
    on delete cascade,
  constraint provider_turn_states_model_ck check (
    octet_length(upstream_model) between 1 and 256
    and upstream_model = btrim(upstream_model)
    and upstream_model !~ '[[:cntrl:]]'
  ),
  constraint provider_turn_states_length_ck check (
    normal_length in (292, 332)
    and (last_observed_length is null or last_observed_length between 1 and 4096)
  ),
  constraint provider_turn_states_active_ck check (
    (active_state is null and active_issued_at is null and active_expires_at is null)
    or (
      active_state is not null
      and octet_length(active_state) between 1 and 4096
      and active_issued_at is not null
      and active_expires_at is not null
      and active_issued_at < active_expires_at
    )
  ),
  constraint provider_turn_states_standby_ck check (
    (standby_state is null and standby_issued_at is null and standby_expires_at is null)
    or (
      standby_state is not null
      and octet_length(standby_state) between 1 and 4096
      and standby_issued_at is not null
      and standby_expires_at is not null
      and standby_issued_at < standby_expires_at
    )
  ),
  constraint provider_turn_states_version_ck check (state_version >= 0),
  constraint provider_turn_states_refresh_status_ck check (
    refresh_status in ('missing', 'ready', 'refreshing', 'failed')
  )
);

create index provider_turn_states_refresh_idx
  on provider_turn_states (refresh_status, active_expires_at, provider_account_id, upstream_model);
