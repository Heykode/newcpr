alter table runtime_settings
  add column account_warmup_enabled boolean not null default false,
  add column account_warmup_schedule_time text not null default '08:00',
  add column account_warmup_model text,
  add column account_warmup_cursor timestamptz,
  add constraint runtime_settings_warmup_schedule_ck
    check (account_warmup_schedule_time ~ '^([01][0-9]|2[0-3]):[0-5][0-9](,([01][0-9]|2[0-3]):[0-5][0-9])*$'),
  add constraint runtime_settings_warmup_model_ck
    check (account_warmup_model is null or (length(account_warmup_model) between 1 and 128 and account_warmup_model = btrim(account_warmup_model)));
