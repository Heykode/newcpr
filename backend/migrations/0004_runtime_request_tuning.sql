alter table runtime_settings
  add column request_tuning_json jsonb;

alter table runtime_settings
  add constraint runtime_settings_request_tuning_ck check (
    request_tuning_json is null
    or (
      jsonb_typeof(request_tuning_json) = 'object'
      and octet_length(request_tuning_json::text) <= 16384
    )
  );
