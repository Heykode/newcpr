-- Both account waiting paths now share the requested upstream defaults.
alter table runtime_settings
  alter column request_tuning_json set default '{"maxWaitingPerKey":10,"keyConcurrencyWaitTimeoutSeconds":30,"accountBusyWaitEnabled":true,"maxWaitingPerAccount":10,"accountConcurrencyWaitTimeoutSeconds":30}'::jsonb;

update runtime_settings
set request_tuning_json =
  (coalesce(request_tuning_json, '{}'::jsonb)
    - 'accountBusyWaitStickyMaxWaiting'
    - 'accountBusyWaitStickyTimeoutSeconds'
    - 'accountBusyWaitFallbackMaxWaiting'
    - 'accountBusyWaitFallbackTimeoutSeconds')
  || jsonb_build_object(
    'maxWaitingPerAccount', coalesce(request_tuning_json->'maxWaitingPerAccount', '10'::jsonb),
    'accountConcurrencyWaitTimeoutSeconds', coalesce(request_tuning_json->'accountConcurrencyWaitTimeoutSeconds', '30'::jsonb)
  );
