alter table runtime_settings
    drop constraint runtime_settings_openai_account_affinity_check,
    add constraint runtime_settings_openai_account_affinity_check
        check (openai_account_affinity in ('relaxed', 'preferred', 'strict'));
