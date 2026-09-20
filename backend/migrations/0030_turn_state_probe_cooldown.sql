alter table provider_turn_states
  add column probe_total_attempts bigint not null default 0 check (probe_total_attempts >= 0),
  add column probe_cooldown_until timestamptz,
  add column probe_retry_from_upstream boolean,
  add column probe_http_status smallint,
  add column probe_error_code text,
  add column probe_returned_length integer;

-- Historical counters only cover the current round; do not invent earlier totals.
update provider_turn_states set probe_total_attempts = probe_attempts;
