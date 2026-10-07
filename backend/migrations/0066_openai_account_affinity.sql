alter table runtime_settings
    add column openai_account_affinity text not null default 'strict'
    check (openai_account_affinity in ('relaxed', 'strict'));
