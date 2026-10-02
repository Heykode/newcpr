alter table runtime_settings
  add column openai_guardian_reserved_concurrency bigint not null default 0
    constraint runtime_settings_guardian_reserved_concurrency_ck
      check (openai_guardian_reserved_concurrency between 0 and 4294967295);
