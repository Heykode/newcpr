-- Match upstream initial policy without changing saved or customized settings.
-- NewCPR stores queue settings in request_tuning_json rather than SQL columns.
alter table runtime_settings
  alter column max_concurrent_per_account set default 3,
  alter column openai_guardian_reserved_concurrency set default 5,
  alter column request_tuning_json set default '{"maxWaitingPerKey":10,"keyConcurrencyWaitTimeoutSeconds":30,"accountBusyWaitEnabled":true,"accountBusyWaitStickyMaxWaiting":10,"accountBusyWaitStickyTimeoutSeconds":30,"accountBusyWaitFallbackMaxWaiting":10,"accountBusyWaitFallbackTimeoutSeconds":30}'::jsonb;

update runtime_settings
set max_concurrent_per_account = 3,
    openai_guardian_reserved_concurrency = 5,
    request_tuning_json = coalesce(request_tuning_json, '{}'::jsonb) ||
      '{"maxWaitingPerKey":10,"keyConcurrencyWaitTimeoutSeconds":30,"accountBusyWaitEnabled":true,"accountBusyWaitStickyMaxWaiting":10,"accountBusyWaitStickyTimeoutSeconds":30,"accountBusyWaitFallbackMaxWaiting":10,"accountBusyWaitFallbackTimeoutSeconds":30}'::jsonb
where current_setting('cpr.fresh_install', true) = 'true'
   or (config_revision = 1
  and max_concurrent_per_account = 5
  and openai_guardian_reserved_concurrency = 0
  and coalesce(request_tuning_json->>'maxWaitingPerKey', '0') = '0'
  and coalesce(request_tuning_json->>'keyConcurrencyWaitTimeoutSeconds', '30') = '30'
  and coalesce(request_tuning_json->>'accountBusyWaitEnabled', 'false') = 'false'
  and coalesce(request_tuning_json->>'accountBusyWaitStickyMaxWaiting', '3') = '3'
  and coalesce(request_tuning_json->>'accountBusyWaitStickyTimeoutSeconds', '120') = '120'
  and coalesce(request_tuning_json->>'accountBusyWaitFallbackMaxWaiting', '100') = '100'
  and coalesce(request_tuning_json->>'accountBusyWaitFallbackTimeoutSeconds', '30') = '30');
