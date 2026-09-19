alter table provider_turn_states
  add column active_captured_at timestamptz,
  add column standby_captured_at timestamptz,
  add column suspect_count smallint not null default 0 check (suspect_count between 0 and 2),
  add column probe_attempts bigint not null default 0 check (probe_attempts >= 0),
  add column last_probe_reason text,
  add column successful_probe_attempt bigint check (successful_probe_attempt > 0);

alter table provider_turn_states
  drop constraint provider_turn_states_refresh_status_ck,
  add constraint provider_turn_states_refresh_status_ck check (
    refresh_status in ('missing', 'ready', 'queued', 'refreshing', 'cooldown', 'failed')
  );

-- A stopped old collector must not remain displayed as running after upgrade.
update provider_turn_states set refresh_status = 'missing'
where refresh_status = 'refreshing';
